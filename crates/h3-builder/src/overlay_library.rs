//! The overlay library: the video clips (dust, particles, smoke, light leaks, snow) a person downloaded to lay over their videos. They live in an
//! `overlay` folder next to the app's config file, one sub folder per category. Importing copies a clip in, checks that it is a video ffmpeg
//! can read, and makes a still and a two second loop to show in the library. A clip is copied again into `<project>/overlay/` when a project
//! uses it, so the project can be moved to another computer with its clips.
//!
//! FX Elements' RAW downloads (`.R3D` in an `.RDC` folder) and `.exr` frames cannot be read by ffmpeg, so they are refused with the
//! words that say which download to take instead (ProRes).

use std::fs;
use std::path::{Component, Path, PathBuf};

use h3_core::pyval::{py_str, Obj};
use serde_json::{json, Value};

use crate::payload::path_text;
use crate::Env;

type Reply = Result<Value, String>;

pub const CATEGORIES: [&str; 7] = ["dust", "particles", "smoke", "flares", "light_leaks", "snow", "other"];
const VIDEO_EXTENSIONS: [&str; 6] = [".mov", ".mp4", ".webm", ".mkv", ".avi", ".m4v"];

impl Env {
    /// Where the overlays and fonts are kept: the folder the app was given (`overlay_dir`, normally `models/overlay` of ComfyUI), else an `overlay`
    /// folder next to the config file, beside the `luts` folder.
    pub fn overlay_library(&self) -> PathBuf {
        self.overlay_dir.clone().unwrap_or_else(|| self.luts.parent().map(Path::to_path_buf).unwrap_or_default().join("overlay"))
    }
}

fn extension(path: &Path) -> String {
    path.extension().map(|e| format!(".{}", e.to_string_lossy().to_lowercase())).unwrap_or_default()
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Why a file that is not a video this app can use is refused, in words that say what to do. `None` for a video.
pub fn refusal(path: &Path) -> Option<String> {
    let ext = extension(path);
    if VIDEO_EXTENSIONS.contains(&ext.as_str()) {
        return None;
    }
    Some(match ext.as_str() {
        ".r3d" | ".rdc" => "This is RED RAW, which cannot be read here. Download the ProRes version of this clip instead.".to_string(),
        ".exr" | ".dpx" | ".tif" | ".tiff" => "These are single image frames, not a video. Download the ProRes version of this clip instead.".to_string(),
        ".braw" | ".ari" | ".arx" | ".crm" => "This is camera RAW, which cannot be read here. Download a ProRes or MP4 version of this clip instead.".to_string(),
        "" => format!("{} has no file type, so it cannot be used as a video.", file_name(path)),
        other => format!("{other} files are not a video type this app can use. Use a .mov, .mp4, .webm, .mkv or .avi file."),
    })
}

/// The category a clip probably belongs to, from its name.
pub fn suggest_category(name: &str) -> &'static str {
    let lower = name.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| lower.contains(w));
    if has(&["dust"]) {
        "dust"
    } else if has(&["leak"]) {
        "light_leaks"
    } else if has(&["flare", "lens"]) {
        "flares"
    } else if has(&["snow"]) {
        "snow"
    } else if has(&["smoke", "fog", "steam", "atmosphere", "mist"]) {
        "smoke"
    } else if has(&["particle", "spark", "ember", "bokeh"]) {
        "particles"
    } else {
        "other"
    }
}

fn clean_category(value: &str, name: &str) -> String {
    let wanted = value.trim().to_lowercase().replace([' ', '-'], "_");
    if CATEGORIES.contains(&wanted.as_str()) { wanted } else { suggest_category(name).to_string() }
}

fn sanitize_file_name(name: &str) -> String {
    let cleaned: String = name.chars().map(|c| if c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ' | '(' | ')') { c } else { '_' }).collect();
    let cleaned = cleaned.trim().trim_matches('.').to_string();
    if cleaned.is_empty() { "clip".into() } else { cleaned }
}

/// A library id (`category/file.ext`) as a path inside the library, or an error when it points anywhere else.
fn item_path(env: &Env, id: &str) -> Result<PathBuf, String> {
    let relative = Path::new(id.trim());
    let parts: Vec<_> = relative.components().collect();
    if parts.is_empty() || parts.len() > 2 || !parts.iter().all(|c| matches!(c, Component::Normal(_))) {
        return Err("That is not an overlay library clip.".into());
    }
    Ok(env.overlay_library().join(relative))
}

fn meta_path(env: &Env, id: &str) -> PathBuf {
    env.overlay_library().join(".meta").join(format!("{}.json", id.replace(['/', '\\'], "__")))
}

fn thumb_files(env: &Env, id: &str) -> (PathBuf, PathBuf) {
    let base = id.replace(['/', '\\'], "__");
    let dir = env.overlay_library().join(".thumbs");
    (dir.join(format!("{base}.jpg")), dir.join(format!("{base}.mp4")))
}

/// What ffprobe says about a clip. `None` when it has no video stream.
fn probe(env: &Env, path: &Path) -> Result<Option<Value>, String> {
    let tools = env.tools()?;
    let out = tools
        .ffprobe_checked(
            &h3_media::tools::args(&["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=codec_name,width,height,r_frame_rate,pix_fmt,duration:stream_tags=alpha_mode:format=duration", "-of", "json", &path.display().to_string()]),
            &format!("ffprobe could not read {}", path.display()),
        )
        .map_err(|e| e.to_string())?;
    let parsed: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    let Some(stream) = parsed["streams"].as_array().and_then(|s| s.first()) else { return Ok(None) };
    let number = |v: &Value| v.as_str().and_then(|s| s.parse::<f64>().ok()).filter(|f| f.is_finite());
    let duration = number(&stream["duration"]).or_else(|| number(&parsed["format"]["duration"])).unwrap_or(0.0);
    let rate = stream["r_frame_rate"].as_str().and_then(|r| r.split_once('/')).and_then(|(a, b)| Some(a.parse::<f64>().ok()? / b.parse::<f64>().ok().filter(|b| *b != 0.0)?)).unwrap_or(0.0);
    let pix_fmt = stream["pix_fmt"].as_str().unwrap_or("");
    // Alpha shows in the pixel format (yuva444p12le, rgba, gbrap) or, for WebM, in a tag.
    let has_alpha = pix_fmt.contains("yuva") || pix_fmt.contains("rgba") || pix_fmt.contains("bgra") || pix_fmt.contains("argb") || pix_fmt.contains("gbrap") || stream["tags"]["alpha_mode"].as_str() == Some("1");
    Ok(Some(json!({
        "codec": stream["codec_name"].as_str().unwrap_or(""),
        "width": stream["width"].as_u64().unwrap_or(0),
        "height": stream["height"].as_u64().unwrap_or(0),
        "fps": rate,
        "duration": duration,
        "pix_fmt": pix_fmt,
        "has_alpha": has_alpha,
        "suggested_blend": if has_alpha { "normal" } else { "screen" },
    })))
}

fn atomic_json(path: &Path, value: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let temp = path.with_extension("json.partial");
    fs::write(&temp, serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?).map_err(|e| format!("{}: {e}", temp.display()))?;
    fs::rename(&temp, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// The still and the two second loop shown in the library. A failure only means the clip has no preview.
fn make_previews(env: &Env, id: &str, path: &Path, duration: f64) {
    let Ok(tools) = env.tools() else { return };
    let (still, looped) = thumb_files(env, id);
    if let Some(parent) = still.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let seek = format!("{:.2}", (duration * 0.3).clamp(0.0, 1.0));
    let source = path.display().to_string();
    let _ = h3_media::tools::run(&tools.ffmpeg, &h3_media::tools::args(&["-y", "-v", "error", "-ss", &seek, "-i", &source, "-frames:v", "1", "-vf", "scale=320:-2", &still.display().to_string()]));
    let _ = h3_media::tools::run(
        &tools.ffmpeg,
        &h3_media::tools::args(&["-y", "-v", "error", "-i", &source, "-t", "2", "-vf", "scale=320:-2,fps=12", "-an", "-c:v", "libx264", "-preset", "veryfast", "-crf", "28", "-pix_fmt", "yuv420p", "-movflags", "+faststart", &looped.display().to_string()]),
    );
}

/// One library clip as the Builder lists it.
fn describe(env: &Env, id: &str, path: &Path) -> Result<Value, String> {
    let cached = fs::read(meta_path(env, id)).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok());
    let meta = match cached {
        Some(meta) => meta,
        None => {
            let meta = probe(env, path)?.ok_or_else(|| format!("{} has no video.", file_name(path)))?;
            atomic_json(&meta_path(env, id), &meta)?;
            meta
        }
    };
    let (still, looped) = thumb_files(env, id);
    let query = |kind: &str| format!("/vrgdg/music_builder/overlay_library/thumb?id={}&kind={kind}", urlencode(id));
    let category = id.split('/').next().unwrap_or("other");
    let mut item = json!({
        "id": id,
        "name": path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
        "file": file_name(path),
        "category": category,
        "path": path.display().to_string(),
        "size_bytes": fs::metadata(path).map(|m| m.len()).unwrap_or(0),
        "thumb_url": if still.is_file() { Value::String(query("image")) } else { Value::Null },
        "loop_url": if looped.is_file() { Value::String(query("loop")) } else { Value::Null },
    });
    if let (Some(map), Some(extra)) = (item.as_object_mut(), meta.as_object()) {
        map.extend(extra.clone());
    }
    Ok(item)
}

fn urlencode(value: &str) -> String {
    value.bytes().map(|b| if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') { char::from(b).to_string() } else { format!("%{b:02X}") }).collect()
}

/// `GET /vrgdg/music_builder/overlay_library`.
pub fn list(env: &Env) -> Reply {
    let root = env.overlay_library();
    let mut items = Vec::new();
    let mut problems = Vec::new();
    if let Ok(categories) = fs::read_dir(&root) {
        let mut folders: Vec<_> = categories.filter_map(Result::ok).filter(|e| e.path().is_dir() && !e.file_name().to_string_lossy().starts_with('.')).collect();
        folders.sort_by_key(|e| e.file_name());
        for folder in folders {
            let category = folder.file_name().to_string_lossy().into_owned();
            let mut files: Vec<_> = fs::read_dir(folder.path()).into_iter().flatten().filter_map(Result::ok).filter(|e| e.path().is_file() && refusal(&e.path()).is_none() && !e.file_name().to_string_lossy().starts_with('.')).collect();
            files.sort_by_key(|e| e.file_name());
            for file in files {
                let id = format!("{category}/{}", file.file_name().to_string_lossy());
                match describe(env, &id, &file.path()) {
                    Ok(item) => items.push(item),
                    Err(error) => problems.push(json!({"id": id, "error": error})),
                }
            }
        }
    }
    Ok(json!({"items": items, "problems": problems, "folder": root.display().to_string(), "categories": CATEGORIES}))
}

/// The video files in a folder an import was pointed at (the folder and the ones directly in it), and the unusable ones it found.
fn videos_in(folder: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let (mut videos, mut refused) = (Vec::new(), Vec::new());
    let mut stack = vec![(folder.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        let mut entries: Vec<_> = fs::read_dir(&dir).into_iter().flatten().filter_map(Result::ok).map(|e| e.path()).collect();
        entries.sort();
        for path in entries {
            let name = file_name(&path);
            if name.starts_with('.') || name == "__MACOSX" {
                continue;
            }
            if path.is_dir() {
                if extension(&path) == ".rdc" {
                    refused.push(path);
                } else if depth < 2 {
                    stack.push((path, depth + 1));
                }
            } else if refusal(&path).is_none() {
                videos.push(path);
            } else if matches!(extension(&path).as_str(), ".r3d" | ".exr" | ".dpx" | ".braw" | ".ari") {
                refused.push(path);
            }
        }
    }
    videos.sort();
    (videos, refused)
}

fn copy_into(source: &Path, target_dir: &Path, name: &str) -> Result<(PathBuf, bool), String> {
    fs::create_dir_all(target_dir).map_err(|e| format!("{}: {e}", target_dir.display()))?;
    let size = fs::metadata(source).map_err(|e| format!("{}: {e}", source.display()))?.len();
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) => (s.to_string(), format!(".{e}")),
        None => (name.to_string(), String::new()),
    };
    let mut target = target_dir.join(name);
    let mut n = 2;
    while target.exists() {
        // The same clip already there is reused. A different file with the same name gets a number.
        if fs::metadata(&target).map(|m| m.len()).ok() == Some(size) {
            return Ok((target, false));
        }
        target = target_dir.join(format!("{stem}_{n}{ext}"));
        n += 1;
    }
    let partial = target_dir.join(format!(".{}.partial", file_name(&target)));
    if let Err(error) = fs::copy(source, &partial) {
        let _ = fs::remove_file(&partial);
        return Err(format!("The clip could not be copied to {}: {error}", target.display()));
    }
    fs::rename(&partial, &target).map_err(|e| {
        let _ = fs::remove_file(&partial);
        format!("{}: {e}", target.display())
    })?;
    Ok((target, true))
}

/// `POST /vrgdg/music_builder/overlay_library/import`: `{path, category?}`. `path` is a video file, or a folder (what a download unzips to) that
/// holds one. Returns the clips it added.
pub fn import(env: &Env, payload: &Obj) -> Reply {
    let wanted = path_text(payload, "path");
    let wanted = wanted.trim().trim_matches('"');
    if wanted.is_empty() {
        return Err("path is required.".into());
    }
    let source = h3_core::abspath::absolute(Path::new(wanted)).map_err(|e| e.to_string())?;
    if !source.exists() {
        return Err(format!("Not found: {}", source.display()));
    }
    let category = py_str(payload.get("category")).to_string();
    let sources = if source.is_dir() && extension(&source) != ".rdc" {
        let (videos, refused) = videos_in(&source);
        if videos.is_empty() {
            return Err(match refused.first() {
                Some(path) => refusal(path).unwrap_or_else(|| "This is RED RAW, which cannot be read here. Download the ProRes version of this clip instead.".into()),
                None => format!("No video file was found in {}.", source.display()),
            });
        }
        videos
    } else {
        if let Some(why) = refusal(&source) {
            return Err(why);
        }
        vec![source]
    };
    let mut added = Vec::new();
    let mut existing = Vec::new();
    for path in sources {
        let name = sanitize_file_name(&file_name(&path));
        let kind = clean_category(&category, &name);
        let (target, copied) = copy_into(&path, &env.overlay_library().join(&kind), &name)?;
        let id = format!("{kind}/{}", file_name(&target));
        let Some(meta) = probe(env, &target).map_err(|e| {
            if copied {
                let _ = fs::remove_file(&target);
            }
            format!("{} is not a video that ffmpeg can read. ({})", file_name(&path), e.lines().next().unwrap_or("").trim())
        })?
        else {
            if copied {
                let _ = fs::remove_file(&target);
            }
            return Err(format!("{} has no video stream that ffmpeg can read.", file_name(&path)));
        };
        if meta["duration"].as_f64().unwrap_or(0.0) <= 0.0 || meta["width"].as_u64().unwrap_or(0) == 0 {
            if copied {
                let _ = fs::remove_file(&target);
            }
            return Err(format!("{} is not a video ffmpeg can read.", file_name(&path)));
        }
        atomic_json(&meta_path(env, &id), &meta)?;
        make_previews(env, &id, &target, meta["duration"].as_f64().unwrap_or(0.0));
        let item = describe(env, &id, &target)?;
        if copied { added.push(item) } else { existing.push(item) }
    }
    Ok(json!({"added": added, "already_there": existing, "items": list(env)?["items"]}))
}

/// `POST /vrgdg/music_builder/overlay_library/delete`: `{id}`. Removes the clip, its details and its previews.
pub fn delete(env: &Env, payload: &Obj) -> Reply {
    let id = path_text(payload, "id");
    let path = item_path(env, &id)?;
    if !path.is_file() {
        return Err(format!("The clip {id} is not in the library."));
    }
    fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (still, looped) = thumb_files(env, id.trim());
    for leftover in [meta_path(env, id.trim()), still, looped] {
        let _ = fs::remove_file(leftover);
    }
    Ok(json!({"deleted": id.trim()}))
}

/// The still or the loop of a clip, for `GET /vrgdg/music_builder/overlay_library/thumb?id=&kind=image|loop`.
pub fn thumb_path(env: &Env, id: &str, kind: &str) -> Result<PathBuf, String> {
    item_path(env, id)?;
    let (still, looped) = thumb_files(env, id.trim());
    let path = if kind == "loop" { looped } else { still };
    if path.is_file() { Ok(path) } else { Err("This clip has no preview.".into()) }
}

/// `POST /vrgdg/music_builder/overlay_library/use`: `{id, project_folder}`. Copies the clip into the project's `overlay` folder (once) and returns the
/// path to save on the clip, relative to the project folder.
pub fn use_in_project(env: &Env, payload: &Obj) -> Reply {
    let source = item_path(env, &path_text(payload, "id"))?;
    if !source.is_file() {
        return Err("That clip is not in the overlay library.".into());
    }
    let folder = h3_core::abspath::absolute(Path::new(path_text(payload, "project_folder").trim())).map_err(|e| e.to_string())?;
    if !folder.join("vrgdg_builder_session.json").is_file() {
        return Err("project_folder is not a project folder.".into());
    }
    let (target, copied) = copy_into(&source, &folder.join("overlay"), &file_name(&source))?;
    Ok(json!({"path": format!("overlay/{}", file_name(&target)), "absolute_path": target.display().to_string(), "copied": copied}))
}

/// Where a path from the Builder points: a path inside the project folder (relative ones start there) or inside the overlay library. Anything else
/// is refused, so the preview cannot be used to read other files.
fn preview_source(env: &Env, path: &str, project_folder: &str) -> Result<PathBuf, String> {
    let wanted = path.trim().trim_matches('"');
    if wanted.is_empty() {
        return Err("path is required.".into());
    }
    let folder = project_folder.trim();
    let candidate = if Path::new(wanted).is_relative() {
        if folder.is_empty() {
            return Err("project_folder is required for a path inside the project.".into());
        }
        Path::new(folder).join(wanted)
    } else {
        PathBuf::from(wanted)
    };
    let full = h3_core::abspath::absolute(&candidate).map_err(|e| e.to_string())?;
    let inside = |root: &Path| h3_core::abspath::absolute(root).is_ok_and(|root| full.starts_with(&root));
    if !(inside(&env.overlay_library()) || (!folder.is_empty() && inside(Path::new(folder)))) {
        return Err("That clip is not in the overlay library or the project.".into());
    }
    if !full.is_file() {
        return Err(format!("Not found: {}", full.display()));
    }
    if let Some(why) = refusal(&full) {
        return Err(why);
    }
    Ok(full)
}

/// A small H.264 copy of a clip the page can play (a browser cannot play a 4K ProRes file), made the first time it is asked for and kept
/// next to the clip in a hidden `.proxy` folder. It has no sound and is at most 960 pixels wide. Used by the live preview in the player.
pub fn preview_proxy(env: &Env, path: &str, project_folder: &str) -> Result<PathBuf, String> {
    let source = preview_source(env, path, project_folder)?;
    let meta = fs::metadata(&source).map_err(|e| format!("{}: {e}", source.display()))?;
    let stamp = meta.modified().ok().and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    let dir = source.parent().unwrap_or(Path::new(".")).join(".proxy");
    let stem = source.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let target = dir.join(format!("{stem}.{}.{stamp}.mp4", meta.len()));
    if target.is_file() {
        return Ok(target);
    }
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let partial = dir.join(format!(".{stem}.{}.partial.mp4", meta.len()));
    // Another request for the same clip is already making it: wait for that one.
    if partial.exists() {
        for _ in 0..240 {
            std::thread::sleep(std::time::Duration::from_millis(250));
            if target.is_file() {
                return Ok(target);
            }
            if !partial.exists() {
                break;
            }
        }
    }
    let tools = env.tools()?;
    let _ = fs::remove_file(&partial);
    let out = h3_media::tools::run(
        &tools.ffmpeg,
        &h3_media::tools::args(&[
            "-y", "-v", "error", "-i", &source.display().to_string(), "-an", "-vf", "scale='min(960,iw)':-2:flags=bilinear", "-c:v", "libx264", "-preset", "veryfast", "-crf", "27", "-pix_fmt", "yuv420p", "-movflags", "+faststart",
            &partial.display().to_string(),
        ]),
    )
    .map_err(|e| e.to_string())?;
    if !out.status.success() {
        let _ = fs::remove_file(&partial);
        return Err(format!("A preview of {} could not be made: {}", file_name(&source), h3_media::tools::failure_text(&out, "ffmpeg failed.")));
    }
    fs::rename(&partial, &target).map_err(|e| {
        let _ = fs::remove_file(&partial);
        format!("{}: {e}", target.display())
    })?;
    Ok(target)
}

/// The media clips' paths, made absolute against the project folder when they are relative (`overlay/AirParticles-008.mov`).
pub fn resolve_clip_paths(clips: &Value, project_folder: &str) -> Value {
    let folder = project_folder.trim();
    let Some(items) = clips.as_array() else { return clips.clone() };
    if folder.is_empty() {
        return clips.clone();
    }
    let resolved = items
        .iter()
        .map(|item| {
            let mut item = item.clone();
            if let Some(path) = item.get("path").and_then(Value::as_str).map(str::trim).filter(|p| !p.is_empty() && Path::new(p).is_relative()) {
                let full = Path::new(folder).join(path);
                item["path"] = json!(full.display().to_string());
            }
            item
        })
        .collect();
    Value::Array(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(dir: &Path) -> Env {
        Env { output: dir.join("output"), roots: vec![], ffmpeg: "ffmpeg".into(), ffprobe: "ffprobe".into(), luts: dir.join("luts"), overlay_dir: None }
    }

    #[test]
    fn the_library_sits_beside_the_luts_folder() {
        assert_eq!(env(Path::new("C:/cfg")).overlay_library(), Path::new("C:/cfg").join("overlay"));
    }

    #[test]
    fn red_raw_and_image_frames_are_refused_with_the_download_to_take() {
        for name in ["Atmosphere-014.R3D", "Atmosphere-014.RDC", "frame.exr", "x.dpx"] {
            let why = refusal(Path::new(name)).unwrap();
            assert!(why.contains("ProRes"), "{name}: {why}");
        }
        assert!(refusal(Path::new("clip.braw")).unwrap().contains("camera RAW"));
        assert!(refusal(Path::new("notes.txt")).unwrap().contains(".txt"));
        assert!(refusal(Path::new("noext")).unwrap().contains("no file type"));
        for name in ["a.mov", "a.MP4", "a.webm", "a.mkv", "a.avi", "a.m4v"] {
            assert_eq!(refusal(Path::new(name)), None, "{name}");
        }
    }

    #[test]
    fn a_category_comes_from_the_words_in_the_name() {
        for (name, want) in [("Dust Hit - 002.mov", "dust"), ("Light_Leak_07.mp4", "light_leaks"), ("Lens Flare 3.mov", "flares"), ("Snow-001.mov", "snow"), ("Atmosphere-014.mov", "smoke"), ("AirParticles-008.mov", "particles"), ("Bokeh 2.mov", "particles"), ("clip.mov", "other")] {
            assert_eq!(suggest_category(name), want, "{name}");
        }
        assert_eq!(clean_category("Light Leaks", "x.mov"), "light_leaks");
        assert_eq!(clean_category("nonsense", "snow.mov"), "snow");
    }

    #[test]
    fn a_library_id_cannot_leave_the_library() {
        let e = env(Path::new("C:/cfg"));
        assert!(item_path(&e, "dust/a.mov").is_ok());
        assert!(item_path(&e, "a.mov").is_ok());
        for bad in ["", "../a.mov", "dust/../../a.mov", "/etc/passwd", "C:/Windows/x.mov", "a/b/c.mov", "dust/.."] {
            assert!(item_path(&e, bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn file_names_are_made_safe() {
        assert_eq!(sanitize_file_name("FX Elements - Air Particles - 008.mov"), "FX Elements - Air Particles - 008.mov");
        assert_eq!(sanitize_file_name("a/b:c*d.mov"), "a_b_c_d.mov");
        assert_eq!(sanitize_file_name("..."), "clip");
    }

    #[test]
    fn relative_clip_paths_are_resolved_against_the_project_folder() {
        let clips = json!([{"kind": "media", "path": "overlay/dust.mov"}, {"kind": "media", "path": "C:/abs/x.mov"}, {"kind": "grain"}, {"kind": "media", "path": ""}]);
        let got = resolve_clip_paths(&clips, "C:/p/My Song");
        assert!(got[0]["path"].as_str().unwrap().replace('\\', "/").ends_with("C:/p/My Song/overlay/dust.mov"));
        assert_eq!(got[1]["path"], "C:/abs/x.mov");
        assert_eq!(got[2], json!({"kind": "grain"}));
        assert_eq!(got[3]["path"], "");
        assert_eq!(resolve_clip_paths(&clips, ""), clips);
    }
}
