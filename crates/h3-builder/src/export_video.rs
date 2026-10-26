//! Export of the finished video: copy a stitched video to a place the user chose. Port of `builder/export_video.py`.
//!
//! The Export button of the timeline stitches every scene into `EXPORT_VIDEO.mp4` in the project folder (the stitch route),
//! asks for a file name with the `save_video` native dialog (`files::pick_path`) and then calls `POST
//! /vrgdg/music_builder/export_final_video` to put the video there.

use std::fs;
use std::path::{Path, PathBuf};

use h3_core::pyval::{truthy, Obj};
use serde_json::{json, Value};

use crate::payload::path_text;

type Reply = Result<Value, String>;

/// The stitch the Export button makes is named with this prefix (`EXPORT_VIDEO.mp4`, `EXPORT_VIDEO_2.mp4` ...). Keep it equal
/// to `EXPORT_STITCH_PREFIX` in `web/music_video_builder/export_video.mjs`.
pub const EXPORT_STITCH_PREFIX: &str = "EXPORT_VIDEO";
const EXPORT_SUFFIX: &str = ".mp4";
const SOURCE_EXTENSIONS: [&str; 6] = ["mp4", "mov", "mkv", "webm", "avi", "m4v"];
const NAME_FORBIDDEN: &str = "<>:\"/\\|?*";

/// `_export_file_name`: the name a save dialog starts with, safe on every system and ending in `.mp4`.
pub fn export_file_name(name: &str) -> String {
    let cleaned: String = name.chars().map(|c| if NAME_FORBIDDEN.contains(c) || (c as u32) < 32 { '_' } else { c }).collect();
    let text = cleaned.trim().trim_end_matches(['.', ' ']);
    let cut = text.len().saturating_sub(EXPORT_SUFFIX.len());
    let stem = if text.len() >= EXPORT_SUFFIX.len() && text.is_char_boundary(cut) && text[cut..].eq_ignore_ascii_case(EXPORT_SUFFIX) { &text[..cut] } else { text };
    let stem = stem.trim().trim_end_matches(['.', ' ']);
    format!("{}{EXPORT_SUFFIX}", if stem.is_empty() { "video" } else { stem })
}

fn absolute(raw: &str) -> PathBuf {
    h3_core::abspath::absolute(raw).unwrap_or_else(|_| PathBuf::from(raw)).components().collect()
}

/// `_same_place`: the same path, ignoring case where the file system does (Windows).
fn same_place(first: &Path, second: &Path) -> bool {
    if cfg!(windows) {
        first.to_string_lossy().to_lowercase() == second.to_string_lossy().to_lowercase()
    } else {
        first == second
    }
}

/// `_copy_atomically`: copy to a hidden file beside the destination and replace it in one step, so a full disk or a dropped
/// network share leaves any video that was already at the destination as it was.
fn copy_atomically(source: &Path, destination: &Path) -> std::io::Result<()> {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or_default();
    let partial = destination.with_file_name(format!(".export_{}_{nanos}.partial", std::process::id()));
    let result = fs::copy(source, &partial).and_then(|_| fs::rename(&partial, destination));
    if result.is_err() {
        let _ = fs::remove_file(&partial);
    }
    result
}

/// `POST /vrgdg/music_builder/export_final_video`: copy the stitched video `source_path` to `destination_path` and report where
/// it went. The destination is always an `.mp4` file in a folder that exists (a name without an extension gets `.mp4`). With
/// `remove_source` the stitch is deleted after the copy, but only a file the Export button made: directly in `project_folder`
/// and named with [`EXPORT_STITCH_PREFIX`]. Any other source stays.
pub fn export_final_video(payload: &Obj) -> Reply {
    let raw_source = path_text(payload, "source_path");
    if raw_source.is_empty() {
        return Err("The stitched video path is empty.".into());
    }
    let source = absolute(&raw_source);
    if !source.is_file() {
        return Err(format!("The stitched video was not found: {}", source.display()));
    }
    let source_extension = source.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if !SOURCE_EXTENSIONS.contains(&source_extension.as_str()) {
        return Err(format!("The stitched file is not a supported video file: {}", source.display()));
    }

    let raw_destination = path_text(payload, "destination_path");
    if raw_destination.is_empty() {
        return Err("Choose where to save the video.".into());
    }
    let mut destination = absolute(&raw_destination);
    match destination.extension().map(|e| e.to_string_lossy().to_lowercase()) {
        None => {
            let mut named = destination.into_os_string();
            named.push(EXPORT_SUFFIX);
            destination = PathBuf::from(named);
        }
        Some(extension) if format!(".{extension}") != EXPORT_SUFFIX => {
            return Err(format!("The exported video is an {EXPORT_SUFFIX} file. Use a file name that ends in {EXPORT_SUFFIX}."));
        }
        Some(_) => {}
    }
    let folder = destination.parent().map(Path::to_path_buf).unwrap_or_default();
    if !folder.is_dir() {
        return Err(format!("The folder to save in does not exist: {}", folder.display()));
    }
    if same_place(&source, &destination) {
        return Err("Choose a place other than the stitched video itself.".into());
    }

    copy_atomically(&source, &destination).map_err(|e| e.to_string())?;

    let project_text = path_text(payload, "project_folder");
    let name_is_export = source.file_name().is_some_and(|n| n.to_string_lossy().starts_with(EXPORT_STITCH_PREFIX));
    let removable = payload.get("remove_source").is_some_and(truthy)
        && !project_text.is_empty()
        && name_is_export
        && source.parent().is_some_and(|parent| same_place(parent, &absolute(&project_text)));
    // The video is saved either way. A stitch left in the project folder costs nothing but space.
    let removed = removable && fs::remove_file(&source).is_ok();
    let size = fs::metadata(&destination).map(|m| m.len()).unwrap_or_default();
    Ok(json!({"destination_path": destination.display().to_string(), "size_bytes": size, "removed_source": removed}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(value: Value) -> Obj {
        value.as_object().cloned().unwrap()
    }

    fn text(path: &Path) -> String {
        path.display().to_string()
    }

    fn write(path: &Path, data: &str) -> PathBuf {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, data).unwrap();
        path.to_path_buf()
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        project: PathBuf,
        out: PathBuf,
        stitch: PathBuf,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        let out = dir.path().join("out");
        fs::create_dir_all(&out).unwrap();
        let stitch = write(&project.join("EXPORT_VIDEO.mp4"), "stitched video");
        Fixture { _dir: dir, project, out, stitch }
    }

    impl Fixture {
        fn export(&self, source: &Path, destination: &Path, project: &str, remove: bool) -> Reply {
            export_final_video(&obj(json!({
                "source_path": text(source), "destination_path": text(destination), "project_folder": project, "remove_source": remove,
            })))
        }

        fn export_stitch(&self, name: &str) -> Reply {
            self.export(&self.stitch, &self.out.join(name), &text(&self.project), true)
        }
    }

    #[test]
    fn names_are_safe_and_end_in_mp4() {
        assert_eq!(export_file_name("My Song"), "My Song.mp4");
        assert_eq!(export_file_name("My Song.mp4"), "My Song.mp4");
        assert_eq!(export_file_name("My Song.MP4"), "My Song.mp4");
        assert_eq!(export_file_name("a<b>:c\"d/e\\f|g?h*i"), "a_b__c_d_e_f_g_h_i.mp4");
        assert_eq!(export_file_name("night. "), "night.mp4");
        for empty in ["", "   ", "...", ".mp4"] {
            assert_eq!(export_file_name(empty), "video.mp4", "{empty:?}");
        }
        assert_eq!(export_file_name("Café ☕"), "Café ☕.mp4");
    }

    #[test]
    fn the_video_is_copied_and_the_stitch_removed() {
        let f = fixture();
        let reply = f.export_stitch("song.mp4").unwrap();
        assert_eq!(reply["destination_path"], text(&f.out.join("song.mp4")));
        assert_eq!(reply["size_bytes"], "stitched video".len());
        assert_eq!(reply["removed_source"], true);
        assert_eq!(fs::read_to_string(f.out.join("song.mp4")).unwrap(), "stitched video");
        assert!(!f.stitch.exists());
        let left: Vec<_> = fs::read_dir(&f.out).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(left.len(), 1, "no partial file is left behind: {left:?}");
    }

    #[test]
    fn the_stitch_stays_unless_it_is_asked_to_go() {
        let f = fixture();
        let reply = f.export(&f.stitch, &f.out.join("song.mp4"), &text(&f.project), false).unwrap();
        assert_eq!(reply["removed_source"], false);
        assert!(f.stitch.is_file());
    }

    #[test]
    fn only_the_stitch_of_the_export_button_is_ever_removed() {
        let f = fixture();
        let project = text(&f.project);
        let final_video = write(&f.project.join("FINAL_VIDEO.mp4"), "x");
        assert_eq!(f.export(&final_video, &f.out.join("a.mp4"), &project, true).unwrap()["removed_source"], false);
        assert!(final_video.is_file());
        let nested = write(&f.project.join("rendered_scene_videos").join("EXPORT_VIDEO.mp4"), "x");
        assert_eq!(f.export(&nested, &f.out.join("b.mp4"), &project, true).unwrap()["removed_source"], false);
        assert!(nested.is_file());
        let elsewhere = write(&f.out.join("other").join("EXPORT_VIDEO.mp4"), "x");
        assert_eq!(f.export(&elsewhere, &f.out.join("c.mp4"), &project, true).unwrap()["removed_source"], false);
        assert!(elsewhere.is_file());
        assert_eq!(f.export(&f.stitch, &f.out.join("d.mp4"), "", true).unwrap()["removed_source"], false);
        assert!(f.stitch.is_file());
    }

    #[test]
    fn a_name_without_an_extension_gets_mp4() {
        let f = fixture();
        let reply = f.export_stitch("no extension").unwrap();
        assert_eq!(reply["destination_path"], text(&f.out.join("no extension.mp4")));
        assert!(f.out.join("no extension.mp4").is_file());
    }

    #[test]
    fn only_an_mp4_name_is_accepted() {
        let f = fixture();
        for name in ["run.exe", "clip.mov", "notes.txt", "x.py"] {
            assert!(f.export_stitch(name).unwrap_err().contains("ends in .mp4"), "{name}");
        }
        assert_eq!(fs::read_dir(&f.out).unwrap().count(), 0);
        assert!(f.stitch.is_file());
    }

    #[test]
    fn a_missing_folder_source_or_empty_path_is_refused() {
        let f = fixture();
        assert!(f.export_stitch("missing/song.mp4").unwrap_err().contains("folder to save in does not exist"));
        let gone = f.project.join("none.mp4");
        assert!(f.export(&gone, &f.out.join("a.mp4"), &text(&f.project), true).unwrap_err().contains("stitched video was not found"));
        let json_file = write(&f.project.join("vrgdg_builder_session.json"), "{}");
        assert!(f.export(&json_file, &f.out.join("a.mp4"), &text(&f.project), true).unwrap_err().contains("not a supported video"));
        assert!(export_final_video(&obj(json!({"source_path": "  "}))).unwrap_err().contains("path is empty"));
        assert!(export_final_video(&obj(json!({"source_path": text(&f.stitch), "destination_path": ""}))).unwrap_err().contains("Choose where to save"));
        assert!(f.export(&f.stitch, &f.stitch, &text(&f.project), true).unwrap_err().contains("other than the stitched video"));
        assert!(f.stitch.is_file());
    }

    #[test]
    fn a_video_already_there_is_replaced() {
        let f = fixture();
        let destination = write(&f.out.join("song.mp4"), "old video");
        f.export_stitch("song.mp4").unwrap();
        assert_eq!(fs::read_to_string(destination).unwrap(), "stitched video");
    }

    #[test]
    fn a_folder_named_like_the_video_leaves_nothing_behind_and_keeps_the_stitch() {
        let f = fixture();
        fs::create_dir(f.out.join("song.mp4")).unwrap();
        assert!(f.export_stitch("song.mp4").is_err());
        let left: Vec<_> = fs::read_dir(&f.out).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect();
        assert_eq!(left, ["song.mp4"], "the half written file is removed");
        assert!(f.stitch.is_file(), "the stitch is kept when the copy failed");
    }
}
