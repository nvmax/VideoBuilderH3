//! The scene scratch folder under ComfyUI's output folder: find the stage clips a render left, back them up into the
//! project and clear the folder. Port of `_find_minimax_h3_stage_outputs`, `_collect_minimax_h3_stage_backup` and
//! `_cleanup_minimax_h3_output_folder`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use h3_graph::inputs::output_location_in;
use h3_media::video::{create_thumbnail, thumbnail_path_for};
use h3_media::Tools;
use serde_json::{json, Value};

use crate::video_ops::VideoOpError;

fn mtime(path: &Path) -> Option<f64> {
    fs::metadata(path).ok()?.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs_f64())
}

fn files_below(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(read) = fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = read.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            files_below(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// The newest `stage1`, `stage2` and `stage3` clip (`*-audio.mp4`) in the folder, as `stageN_path` (empty when none).
pub fn find_stage_outputs(output_folder: &Path, min_mtime: f64) -> Value {
    let mut found: [Option<(f64, String)>; 3] = [None, None, None];
    if output_folder.is_dir() {
        let mut files = Vec::new();
        files_below(output_folder, &mut files);
        for path in files {
            let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
            if !name.ends_with("-audio.mp4") {
                continue;
            }
            let Some(stage) = ["stage1", "stage2", "stage3"].iter().position(|s| name.contains(s)) else { continue };
            let (Some(modified), Ok(meta)) = (mtime(&path), fs::metadata(&path)) else { continue };
            if modified + 1.0 < min_mtime || meta.len() == 0 {
                continue;
            }
            if found[stage].as_ref().is_none_or(|(prev, _)| modified > *prev) {
                found[stage] = Some((modified, h3_core::abspath::absolute(&path).unwrap_or(path).display().to_string()));
            }
        }
    }
    let path_of = |i: usize| found[i].as_ref().map(|(_, p)| p.clone()).unwrap_or_default();
    json!({"stage1_path": path_of(0), "stage2_path": path_of(1), "stage3_path": path_of(2)})
}

fn norm(path: &Path) -> String {
    path.display().to_string().replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

fn inside(parent: &Path, child: &Path) -> bool {
    let (parent, child) = (norm(parent), norm(child));
    child == parent || child.starts_with(&format!("{parent}\\"))
}

/// Copy a stage clip into `rendered_scene_videos_backup/scene_NNNN/` of the project. The source must be outside it.
pub fn backup_stage(tools: &Tools, project: &Path, source: &Path, stage: &str, scene: u32, stamp: &str) -> Result<Value, VideoOpError> {
    let source = h3_core::abspath::absolute(source).map_err(|e| VideoOpError::Invalid(e.to_string()))?;
    if !source.is_file() {
        return Err(VideoOpError::Invalid(format!("MiniMax H3 stage video was not found: {}", source.display())));
    }
    let project = h3_core::abspath::absolute(project).map_err(|e| VideoOpError::Invalid(e.to_string()))?;
    if inside(&project, &source) {
        return Err(VideoOpError::Invalid("MiniMax H3 stage backup source must be outside the project folder.".into()));
    }
    if !matches!(stage, "stage1" | "stage2") {
        return Err(VideoOpError::Invalid("MiniMax H3 stage backup must be stage1 or stage2.".into()));
    }
    let dir = project.join("rendered_scene_videos_backup").join(format!("scene_{scene:04}"));
    fs::create_dir_all(&dir).map_err(|e| VideoOpError::Invalid(e.to_string()))?;
    let mut target = dir.join(format!("video_{scene:04}-{stage}_{stamp}.mp4"));
    let mut index = 2;
    while target.exists() {
        target = dir.join(format!("video_{scene:04}-{stage}_{stamp}_{index:02}.mp4"));
        index += 1;
    }
    fs::copy(&source, &target).map_err(|e| VideoOpError::Invalid(format!("Copying MiniMax H3 {stage} backup failed: {e}")))?;
    let thumbnail = create_thumbnail(tools, &target, None).map(|p| p.display().to_string()).unwrap_or_default();
    Ok(json!({"backup_path": target.display().to_string(), "backup_thumbnail_path": thumbnail, "stage": stage, "scene_number": scene}))
}

fn linked(path: &Path, stop_at: &Path) -> bool {
    let mut current = Some(path);
    while let Some(p) = current {
        if fs::symlink_metadata(p).map(|m| m.file_type().is_symlink()).unwrap_or(false) {
            return true;
        }
        if p == stop_at {
            break;
        }
        current = p.parent();
    }
    false
}

/// Remove the scene's scratch folder. Only that exact folder under `VRGDG_MiniMaxH3` is allowed, with no links in the way.
pub fn cleanup_scratch(output_dir: &str, project: &Path, output_folder: &str, scene: u32) -> Result<Value, VideoOpError> {
    let given = output_folder.trim().trim_matches('"');
    if given.is_empty() || project.as_os_str().is_empty() {
        return Err(VideoOpError::Invalid("Output folder and project folder are required.".into()));
    }
    if !(1..=999_999).contains(&scene) {
        return Err(VideoOpError::Invalid("A valid scene number is required.".into()));
    }
    let project = h3_core::abspath::absolute(project).map_err(|e| VideoOpError::Invalid(e.to_string()))?;
    let (expected, _) = output_location_in(output_dir, &project.display().to_string(), scene);
    let expected = h3_core::abspath::absolute(&expected).map_err(|e| VideoOpError::Invalid(e.to_string()))?;
    let folder = h3_core::abspath::absolute(given).map_err(|e| VideoOpError::Invalid(e.to_string()))?;
    let root = h3_core::abspath::absolute(PathBuf::from(output_dir).join("VRGDG_MiniMaxH3")).map_err(|e| VideoOpError::Invalid(e.to_string()))?;
    if norm(&folder) != norm(&expected) || !inside(&root, &folder) || norm(&folder) == norm(&root) || linked(&folder, &root) {
        return Err(VideoOpError::Invalid("Output folder must be the scene's MiniMax H3 scratch directory without links.".into()));
    }
    if !folder.is_dir() {
        return Ok(json!({"removed": false}));
    }
    fs::remove_dir_all(&folder).map_err(|e| VideoOpError::Invalid(e.to_string()))?;
    Ok(json!({"removed": true, "output_folder": folder.display().to_string()}))
}

/// Digits after `prefix`, then `rest_ok` of what follows. `None` when the name does not fit.
fn number_after(name: &str, prefix: &str) -> Option<(u32, usize)> {
    let tail = name.strip_prefix(prefix)?;
    let digits = tail.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    Some((tail[..digits].parse().ok()?, prefix.len() + digits))
}

fn ensure_thumbnail(tools: &Tools, video: &Path) -> String {
    let target = thumbnail_path_for(video);
    if target.is_file() {
        return target.display().to_string();
    }
    create_thumbnail(tools, video, None).map(|p| p.display().to_string()).unwrap_or_default()
}

/// The `scene_NN` or `scene-NN` number named by a folder in the path, searching from the file outward.
fn scene_from_path(path: &Path) -> Option<u32> {
    for part in path.components().rev() {
        let text = part.as_os_str().to_string_lossy().to_lowercase();
        let mut from = 0;
        while let Some(at) = text[from..].find("scene") {
            let start = from + at + 5;
            if matches!(text.as_bytes().get(start), Some(b'_' | b'-')) {
                if let Some((n, _)) = number_after(&text[start + 1..], "") {
                    return Some(n);
                }
            }
            from = start;
        }
    }
    None
}

fn walk_mp4(dir: &Path, out: &mut Vec<PathBuf>) {
    files_below(dir, out);
    out.retain(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("mp4")));
}

/// Index the project's scene clips and backups, and copy finished clips found in the Builder's old scratch folders into
/// `rendered_scene_videos`. Port of `_scan_builder_scene_videos`.
pub fn scan_scene_videos(tools: &Tools, project: &Path) -> Result<Value, VideoOpError> {
    let folder = h3_core::abspath::absolute(project).map_err(|e| VideoOpError::Invalid(e.to_string()))?;
    let video_folder = folder.join("rendered_scene_videos");
    let backup_root = folder.join("rendered_scene_videos_backup");
    let list = |dir: &Path| -> Vec<(String, PathBuf)> {
        let mut names: Vec<(String, PathBuf)> = fs::read_dir(dir).map(|d| d.flatten().map(|e| (e.file_name().to_string_lossy().into_owned(), e.path())).collect()).unwrap_or_default();
        names.sort();
        names
    };
    let mut srt_times: Vec<(u32, f64)> = Vec::new();
    for (name, path) in list(&folder.join("scene_srt")) {
        let lower = name.to_lowercase();
        if let Some((n, end)) = number_after(&lower, "scene_") {
            if &lower[end..] == ".srt" && path.is_file() {
                if let Some(t) = mtime(&path) {
                    srt_times.push((n, t));
                }
            }
        }
    }
    srt_times.sort_by(|a, b| a.1.total_cmp(&b.1));

    fs::create_dir_all(&video_folder).map_err(|e| VideoOpError::Invalid(e.to_string()))?;
    let mut videos: BTreeMap<u32, String> = BTreeMap::new();
    let mut thumbnails: BTreeMap<u32, String> = BTreeMap::new();
    for (name, path) in list(&video_folder) {
        let lower = name.to_lowercase();
        let Some((n, end)) = number_after(&lower, "video_") else { continue };
        if &lower[end..] != "-audio.mp4" || !path.is_file() {
            continue;
        }
        let legacy = h3_media::video::legacy_thumbnail_path(&path);
        if legacy.is_file() {
            let _ = fs::remove_file(&legacy);
        }
        videos.insert(n, path.display().to_string());
        let thumb = ensure_thumbnail(tools, &path);
        if !thumb.is_empty() {
            thumbnails.insert(n, thumb);
        }
    }

    // Finished clips left in the Builder's old scratch folders.
    let mut candidates: BTreeMap<u32, (i32, f64, PathBuf)> = BTreeMap::new();
    for (name, dir) in list(&folder) {
        if !dir.is_dir() || !["image_to_video_clips", "text_to_video_clips", "reference_to_video_clips", "ingredients_to_video_clips"].iter().any(|p| name == *p || name.starts_with(&format!("{p}_"))) {
            continue;
        }
        let mut files = Vec::new();
        walk_mp4(&dir, &mut files);
        for path in files {
            let file_name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
            let Some((raw, end)) = number_after(&file_name, "video_") else { continue };
            let rest = &file_name[end..];
            if !(rest == ".mp4" || rest.starts_with('-') || rest.starts_with('_')) {
                continue;
            }
            let (Some(modified), Ok(meta)) = (mtime(&path), fs::metadata(&path)) else { continue };
            if meta.len() == 0 {
                continue;
            }
            let path = h3_core::abspath::absolute(&path).unwrap_or(path);
            let key = match scene_from_path(&path) {
                Some(n) => n,
                None if raw != 1 && !videos.contains_key(&raw) => raw,
                None => srt_times.iter().filter(|(k, t)| *t <= modified + 2.0 && !videos.contains_key(k)).max_by(|a, b| a.1.total_cmp(&b.1)).map_or(raw, |(k, _)| *k),
            };
            let score = if file_name.ends_with("-audio.mp4") { 100 } else { 0 } + if file_name.contains("-audio") { 10 } else { 0 };
            if candidates.get(&key).is_none_or(|(s, m, _)| (score, modified) > (*s, *m)) {
                candidates.insert(key, (score, modified, path));
            }
        }
    }
    let mut recovered: BTreeMap<u32, String> = BTreeMap::new();
    for (scene, (_, _, source)) in candidates {
        if videos.contains_key(&scene) {
            continue;
        }
        let target = video_folder.join(format!("video_{scene:04}-audio.mp4"));
        match fs::copy(&source, &target) {
            Ok(_) => {
                videos.insert(scene, target.display().to_string());
                recovered.insert(scene, source.display().to_string());
                let thumb = ensure_thumbnail(tools, &target);
                if !thumb.is_empty() {
                    thumbnails.insert(scene, thumb);
                }
            }
            Err(e) => eprintln!("[VRGDG-RS Video] Could not recover scene video '{}': {e}", source.display()),
        }
    }

    // Backups: the newest twelve per scene, oldest first.
    let mut backups: BTreeMap<u32, Vec<(String, f64)>> = BTreeMap::new();
    if backup_root.is_dir() {
        let mut files = Vec::new();
        walk_mp4(&backup_root, &mut files);
        for path in files {
            let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
            let Some((n, end)) = number_after(&name, "video_") else { continue };
            if !name[end..].starts_with("-audio_") || !path.is_file() {
                continue;
            }
            backups.entry(n).or_default().push((path.display().to_string(), mtime(&path).unwrap_or(0.0)));
        }
    }
    let mut backup_out = serde_json::Map::new();
    let mut backup_thumbs = serde_json::Map::new();
    for (scene, mut pairs) in backups {
        pairs.sort_by(|a, b| b.1.total_cmp(&a.1));
        pairs.truncate(12);
        pairs.reverse();
        backup_thumbs.insert(scene.to_string(), json!(pairs.iter().map(|(p, _)| ensure_thumbnail(tools, Path::new(p))).collect::<Vec<_>>()));
        backup_out.insert(scene.to_string(), json!(pairs.iter().map(|(p, _)| p.clone()).collect::<Vec<_>>()));
    }
    let keyed = |m: &BTreeMap<u32, String>| Value::Object(m.iter().map(|(k, v)| (k.to_string(), json!(v))).collect());
    Ok(json!({
        "project_folder": folder.display().to_string(),
        "video_folder": video_folder.display().to_string(),
        "videos": keyed(&videos),
        "video_thumbnails": keyed(&thumbnails),
        "video_backups": Value::Object(backup_out),
        "video_backup_thumbnails": Value::Object(backup_thumbs),
        "recovered_from_scratch": keyed(&recovered),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn stage_clips_are_found_by_name_and_age() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("a_stage1_00001-audio.mp4"), b"x");
        touch(&dir.path().join("sub/b_stage2_00001-audio.mp4"), b"x");
        touch(&dir.path().join("c_stage3_00001-audio.mp4"), b"");
        touch(&dir.path().join("d_stage1.mp4"), b"x");
        let found = find_stage_outputs(dir.path(), 0.0);
        assert!(found["stage1_path"].as_str().unwrap().ends_with("a_stage1_00001-audio.mp4"));
        assert!(found["stage2_path"].as_str().unwrap().contains("sub"));
        assert_eq!(found["stage3_path"], "");
        assert_eq!(find_stage_outputs(dir.path(), 4_000_000_000.0)["stage1_path"], "");
        assert_eq!(find_stage_outputs(&dir.path().join("none"), 0.0)["stage2_path"], "");
    }

    #[test]
    fn only_the_scenes_own_scratch_folder_is_removed() {
        let out = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let output_dir = out.path().display().to_string();
        let (scratch, _) = output_location_in(&output_dir, &project.path().display().to_string(), 3);
        touch(&PathBuf::from(&scratch).join("x.mp4"), b"x");
        let other = out.path().join("VRGDG_MiniMaxH3").join("other");
        touch(&other.join("keep.mp4"), b"x");
        assert!(cleanup_scratch(&output_dir, project.path(), &other.display().to_string(), 3).is_err());
        assert!(cleanup_scratch(&output_dir, project.path(), &out.path().join("VRGDG_MiniMaxH3").display().to_string(), 3).is_err());
        assert!(cleanup_scratch(&output_dir, project.path(), "", 3).is_err());
        assert!(cleanup_scratch(&output_dir, project.path(), &scratch, 0).is_err());
        assert!(other.join("keep.mp4").is_file());
        let done = cleanup_scratch(&output_dir, project.path(), &scratch, 3).unwrap();
        assert_eq!(done["removed"], true);
        assert!(!PathBuf::from(&scratch).exists());
        assert_eq!(cleanup_scratch(&output_dir, project.path(), &scratch, 3).unwrap()["removed"], false);
    }
}
