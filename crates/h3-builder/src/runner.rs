//! The file side of a render, as the Builder asks for it: collect a finished clip into the project, trim it to the scene,
//! match its opening colour, find a render's output, back up and clean the scratch folders, stitch the scenes together,
//! render a slideshow and cut a scene's audio. Ports of the handlers in `runner/routes.py` over the functions in
//! `runner/video_files.py` and `runner/minimax_inputs.py`. The work itself is in `h3-media` and `h3-pipeline`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use h3_core::pyval::{text_or_empty, to_float, to_int, truthy, Obj};
use h3_media::collect::{collect_scene_video, Existing};
use h3_media::slideshow::{render_slideshow, SlideItem, SlideshowRequest};
use h3_media::stitch::{stitch_scene_videos, OverlayItem, SceneAudioItem, StitchRequest, TimingItem};
use h3_media::video::{apply_color_match, trim_scene_video, TrimRequest};
use h3_project::layout::Stamp;
use h3_project::takes;
use serde_json::{json, Value};

use crate::payload::path_text;
use crate::Env;

type Reply = Result<Value, String>;

const VIDEO_TYPES: [&str; 6] = ["mp4", "mov", "mkv", "webm", "avi", "m4v"];
const IMAGE_TYPES: [&str; 7] = ["png", "jpg", "jpeg", "webp", "bmp", "tif", "tiff"];

fn text(path: &Path) -> String {
    path.display().to_string()
}

fn absolute(path: &Path) -> PathBuf {
    h3_core::abspath::absolute(path).unwrap_or_else(|_| path.to_path_buf()).components().collect()
}

fn abs_text(payload: &Obj, key: &str) -> PathBuf {
    absolute(Path::new(&path_text(payload, key)))
}

/// `_int_payload`: an integer clamped to a range, the default when it is missing or not a number.
fn int_in(payload: &Obj, key: &str, default: i64, min: i64, max: i64) -> i64 {
    payload.get(key).and_then(to_int).unwrap_or(default).clamp(min, max)
}

/// `float(payload.get(key, 0) or 0)`.
fn float_of(payload: &Obj, key: &str) -> f64 {
    payload.get(key).filter(|v| truthy(v)).and_then(to_float).unwrap_or(0.0)
}

/// `_bool_payload`.
fn flag(payload: &Obj, key: &str, default: bool) -> bool {
    match payload.get(key) {
        None => default,
        Some(Value::String(s)) => matches!(s.trim().to_lowercase().as_str(), "1" | "true" | "yes" | "on"),
        Some(v) => truthy(v),
    }
}

fn has_extension(path: &Path, allowed: &[&str]) -> bool {
    path.extension().is_some_and(|e| allowed.contains(&e.to_string_lossy().to_lowercase().as_str()))
}

fn inside(parent: &Path, child: &Path) -> bool {
    let norm = |p: &Path| {
        let t = absolute(p).display().to_string();
        if cfg!(windows) { t.replace('/', "\\").to_lowercase() } else { t }
    };
    PathBuf::from(norm(child)).starts_with(norm(parent))
}

/// `_safe_project_subfolder`: the project folder and one of its folders, made.
fn project_subfolder(payload: &Obj, name: &str) -> Result<(PathBuf, PathBuf), String> {
    let project = path_text(payload, "project_folder");
    if project.is_empty() {
        return Err("Project folder is empty.".into());
    }
    let project = absolute(Path::new(&project));
    let target = project.join(name);
    fs::create_dir_all(&target).map_err(|e| format!("{}: {e}", target.display()))?;
    Ok((project, target))
}

fn optional_text(path: Option<PathBuf>) -> String {
    path.map(|p| text(&p)).unwrap_or_default()
}

/// `POST .../collect_scene_video`: move a finished render into `rendered_scene_videos` as `video_NNNN-audio.mp4`.
pub fn collect_video(env: &Env, payload: &Obj, now: &Stamp) -> Reply {
    let source = abs_text(payload, "source_path");
    if !source.is_file() {
        return Err(format!("Scene video was not found: {}", source.display()));
    }
    let (project, target_dir) = project_subfolder(payload, "rendered_scene_videos")?;
    let scene = int_in(payload, "scene_number", 1, 1, 999_999) as u32;
    let action = text_or_empty(payload.get("existing_action")).trim().to_lowercase();
    let existing = if action == "backup" { Existing::Backup } else { Existing::Overwrite };
    let tools = env.tools()?;
    let result = collect_scene_video(&tools, &project, scene, &source, existing, &now.compact, now).map_err(|e| e.to_string())?;
    Ok(json!({
        "video_path": text(&result.video_path),
        "thumbnail_path": optional_text(result.thumbnail_path),
        "video_folder": text(&target_dir),
        "backup_path": optional_text(result.backup_path),
        "backup_thumbnail_path": optional_text(result.backup_thumbnail_path),
        "existing_action": if existing == Existing::Backup { "backup" } else { "overwrite" },
        "source_path": text(&result.source_path),
        "removed_files": [],
        "removed_folder": "",
        "removed_scratch_folders": [],
    }))
}

/// `POST .../trim_scene_video`: cut a render to the scene's length.
pub fn trim_video(env: &Env, payload: &Obj, now: &Stamp) -> Reply {
    let source = abs_text(payload, "source_path");
    if !source.is_file() {
        return Err(format!("Scene video was not found: {}", source.display()));
    }
    if !has_extension(&source, &VIDEO_TYPES) {
        return Err(format!("Scene media is not a supported video file: {}", source.display()));
    }
    let (project, target_dir) = project_subfolder(payload, "rendered_scene_videos")?;
    let start = float_of(payload, "start").max(0.0);
    let duration = float_of(payload, "duration").max(0.05);
    let restore_text = path_text(payload, "restore_audio_path");
    let restore = if restore_text.is_empty() {
        None
    } else {
        let audio = absolute(Path::new(&restore_text));
        if !audio.is_file() {
            return Err(format!("Scene audio to restore was not found: {}", audio.display()));
        }
        Some((audio, float_of(payload, "restore_audio_start_seconds").max(0.0)))
    };
    let request = TrimRequest {
        project,
        scene: int_in(payload, "scene_number", 1, 1, 999_999) as u32,
        source: source.clone(),
        start,
        duration,
        frames: int_in(payload, "frames", 0, 0, 999_999) as u32,
        label: match text_or_empty(payload.get("label")) {
            l if l.is_empty() => "trim".into(),
            l => l,
        },
        mark_as_audio_video: flag(payload, "mark_as_audio_video", false),
        restore_audio: restore,
    };
    let tools = env.tools()?;
    let result = trim_scene_video(&tools, &request, &now.compact).map_err(|e| e.to_string())?;
    Ok(json!({
        "video_path": text(&result.video_path),
        "thumbnail_path": optional_text(result.thumbnail_path),
        "video_folder": text(&target_dir),
        "source_path": text(&source),
        "start": start,
        "duration": duration,
    }))
}

/// `POST .../match_scene_video_start_color`: make a clip open in the colours the clip before it ended with.
pub fn match_start_color(env: &Env, payload: &Obj) -> Reply {
    let project = abs_text(payload, "project_folder");
    let video = abs_text(payload, "video_path");
    let reference = abs_text(payload, "reference_video_path");
    if path_text(payload, "project_folder").is_empty() || !project.is_dir() {
        return Err("Project folder is empty or does not exist.".into());
    }
    for (label, path) in [("Scene video", &video), ("Previous scene video", &reference)] {
        if !path.is_file() {
            return Err(format!("{label} was not found: {}", path.display()));
        }
        if !inside(&project, path) {
            return Err(format!("{label} must be inside the current project folder."));
        }
    }
    let number = |key: &str, default: f64, min: f64, max: f64| payload.get(key).filter(|v| truthy(v)).and_then(to_float).unwrap_or(default).clamp(min, max);
    let fade = number("fade_seconds", 1.0, 0.05, 30.0);
    let strength = number("strength", 0.85, 0.0, 1.0);
    if strength <= 0.0 {
        return Ok(json!({"video_path": text(&video), "applied": false, "reason": "strength is zero"}));
    }
    let tools = env.tools()?;
    let before = takes::video_fingerprint(&video);
    let token = format!("{}_{}", Stamp::now().millis(), std::process::id());
    let result = apply_color_match(&tools, &video, &reference, fade, strength, &token).map_err(|e| e.to_string())?;
    if result.applied {
        let scene = video.file_name().map(|n| n.to_string_lossy().to_string()).and_then(|n| n.strip_prefix("video_").map(str::to_string)).and_then(|n| n.chars().take_while(char::is_ascii_digit).collect::<String>().parse::<u32>().ok());
        if let Some(scene) = scene {
            if let Err(e) = takes::rename_video(&project, scene, &before, &takes::video_fingerprint(&video)) {
                eprintln!("[VRGDG-RS Latent Takes] Could not keep the scene's latent take after the color match: {e}");
            }
        }
    }
    Ok(json!({
        "video_path": text(&video),
        "thumbnail_path": optional_text(result.thumbnail_path),
        "applied": result.applied,
        "fade_seconds": fade,
        "strength": strength,
        "reference_video_path": text(&reference),
    }))
}

fn modified_seconds(path: &Path) -> Option<f64> {
    fs::metadata(path).ok()?.modified().ok()?.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs_f64())
}

fn files_below(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.flatten().collect();
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

/// `POST .../find_scene_video_output`: the newest finished clip of a scene in the render's scratch folders.
pub fn find_video_output(payload: &Obj) -> Reply {
    let project = abs_text(payload, "project_folder");
    if path_text(payload, "project_folder").is_empty() || !project.is_dir() {
        return Err("Project folder is empty or does not exist.".into());
    }
    let mode = text_or_empty(payload.get("video_mode")).trim().to_lowercase();
    let prefix = match mode.as_str() {
        "rtv" => "reference_to_video_clips",
        "t2v" => "text_to_video_clips",
        "ingredients" => "ingredients_to_video_clips",
        "id_lora" => "id_lora_i2v_clips",
        _ => "image_to_video_clips",
    };
    let scene = int_in(payload, "scene_number", 0, 0, 999_999);
    let prompt_number = int_in(payload, "prompt_number_one_based", scene, 0, 999_999);
    let min_mtime = float_of(payload, "min_mtime");
    let given = path_text(payload, "output_folder");
    let mut folders: Vec<PathBuf> = Vec::new();
    if !given.is_empty() {
        let folder = absolute(Path::new(&given));
        if folder.is_dir() && inside(&project, &folder) {
            folders.push(folder);
        }
    }
    if !flag(payload, "strict_output_folder", false) {
        if let Ok(entries) = fs::read_dir(&project) {
            let mut names: Vec<_> = entries.flatten().collect();
            names.sort_by_key(|e| e.file_name());
            for entry in names {
                let name = entry.file_name().to_string_lossy().to_string();
                let path = absolute(&entry.path());
                if path.is_dir() && (name == prefix || name.starts_with(&format!("{prefix}_"))) && !folders.contains(&path) {
                    folders.push(path);
                }
            }
        }
    }
    let wanted_exact = format!("video_{scene:04}-audio.mp4");
    let prompt_prefix = format!("video_{prompt_number:04}");
    let scene_marker = format!("_{scene:04}_");
    let mut candidates: Vec<(i32, f64, PathBuf, PathBuf)> = Vec::new();
    for folder in &folders {
        let mut files = Vec::new();
        files_below(folder, &mut files);
        for path in files {
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let lower = name.to_lowercase();
            if !lower.ends_with("-audio.mp4") || !inside(&project, path.parent().unwrap_or(&project)) {
                continue;
            }
            let (Some(modified), Ok(meta)) = (modified_seconds(&path), fs::metadata(&path)) else { continue };
            if meta.len() == 0 || (min_mtime != 0.0 && modified + 1.0 < min_mtime) {
                continue;
            }
            let mut score = 0;
            if scene != 0 && lower == wanted_exact.to_lowercase() {
                score += 1000;
            }
            if prompt_number != 0 && lower.starts_with(&prompt_prefix) && lower[prompt_prefix.len()..].starts_with(['_', '-']) {
                score += 700;
            }
            if scene != 0 && name.contains(&scene_marker) {
                score += 100;
            }
            candidates.push((score, modified, absolute(&path), folder.clone()));
        }
    }
    let searched: Vec<String> = folders.iter().map(|f| text(f)).collect();
    candidates.sort_by(|a, b| (b.0, b.1).partial_cmp(&(a.0, a.1)).unwrap_or(std::cmp::Ordering::Equal));
    let Some((_, _, path, folder)) = candidates.into_iter().next() else {
        return Ok(json!({"video_path": "", "output_folder": "", "searched_folders": searched}));
    };
    Ok(json!({"video_path": text(&path), "output_folder": text(&folder), "searched_folders": searched}))
}

/// `POST .../find_minimax_h3_stage_outputs`.
pub fn find_stage_outputs(payload: &Obj) -> Reply {
    let folder = abs_text(payload, "output_folder");
    Ok(h3_pipeline::scratch_ops::find_stage_outputs(&folder, float_of(payload, "min_mtime")))
}

/// `POST .../collect_minimax_h3_stage_backup`: keep a copy of a render stage from the scratch folder.
pub fn stage_backup(env: &Env, payload: &Obj, now: &Stamp) -> Reply {
    let tools = env.tools()?;
    let stage = text_or_empty(payload.get("stage")).trim().to_lowercase();
    let scene = int_in(payload, "scene_number", 1, 1, 999_999) as u32;
    h3_pipeline::scratch_ops::backup_stage(&tools, &abs_text(payload, "project_folder"), &abs_text(payload, "source_path"), &stage, scene, &now.compact).map_err(|e| e.to_string())
}

/// `POST .../cleanup_minimax_h3_output`: remove a scene's scratch folder under ComfyUI's output.
pub fn cleanup_output(comfy_output: &str, payload: &Obj) -> Reply {
    let scene = payload.get("scene_number").and_then(to_int).unwrap_or(0);
    let scene = if (1..=999_999).contains(&scene) { scene as u32 } else { 0 };
    h3_pipeline::scratch_ops::cleanup_scratch(comfy_output, &abs_text(payload, "project_folder"), &path_text(payload, "output_folder"), scene).map_err(|e| e.to_string())
}

fn string_list(payload: &Obj, key: &str) -> Vec<Value> {
    payload.get(key).and_then(Value::as_array).cloned().unwrap_or_default()
}

fn existing_file(raw: &Value, what: String) -> Result<PathBuf, String> {
    let path = absolute(Path::new(text_or_empty(Some(raw)).trim().trim_matches('"')));
    if !path.is_file() {
        return Err(format!("{what} was not found: {}", path.display()));
    }
    Ok(path)
}

/// `POST .../stitch_scene_videos`: join the scenes (and inserts) with the song or the scenes' own audio.
pub fn stitch_videos(env: &Env, payload: &Obj) -> Reply {
    let raw_paths = string_list(payload, "scene_paths");
    if raw_paths.is_empty() {
        return Err("No scene video paths were provided.".into());
    }
    let (project, _target) = project_subfolder(payload, "rendered_scene_videos")?;
    let raw_audio_paths = string_list(payload, "scene_audio_paths");
    let raw_audio_items = string_list(payload, "scene_audio_items");
    let raw_overlays = string_list(payload, "overlay_items");
    let raw_timing = string_list(payload, "scene_timing_items");
    let audio_path = abs_text(payload, "audio_path");
    let use_embedded = payload.get("use_embedded_scene_audio").is_some_and(truthy);
    let timeline_fps = int_in(payload, "timeline_fps", 0, 0, 120) as u32;
    let song_given = path_text(payload, "song_path");
    let song = if !song_given.is_empty() && absolute(Path::new(&song_given)).is_file() {
        Some(absolute(Path::new(&song_given)))
    } else if audio_path.is_file() {
        Some(audio_path.clone())
    } else {
        None
    };

    let mut scene_paths = Vec::new();
    for (index, raw) in raw_paths.iter().enumerate() {
        let path = existing_file(raw, format!("Scene {} video", index + 1))?;
        if !has_extension(&path, &VIDEO_TYPES) {
            return Err(format!("Scene {} media is not a supported video file: {}", index + 1, path.display()));
        }
        scene_paths.push(path);
    }

    let mut audio_items: Vec<SceneAudioItem> = Vec::new();
    let item_has_path = |item: &Value| item.as_object().is_some_and(|o| !text_or_empty(o.get("path")).trim().is_empty());
    if raw_audio_items.iter().any(item_has_path) {
        if raw_audio_items.len() != scene_paths.len() {
            return Err("Scene audio item count does not match scene video count.".into());
        }
        for (index, item) in raw_audio_items.iter().enumerate() {
            let Some(item) = item.as_object() else { return Err(format!("Scene {} audio item is invalid.", index + 1)) };
            let path = existing_file(item.get("path").unwrap_or(&Value::Null), format!("Scene {} audio", index + 1))?;
            audio_items.push(SceneAudioItem { path, start: float_of(item, "start").max(0.0), duration: float_of(item, "duration").max(0.05), embedded: false });
        }
    } else if raw_audio_paths.iter().any(|p| !text_or_empty(Some(p)).trim().is_empty()) {
        if raw_audio_paths.len() != scene_paths.len() {
            return Err("Scene audio path count does not match scene video count.".into());
        }
        for (index, raw) in raw_audio_paths.iter().enumerate() {
            audio_items.push(SceneAudioItem { path: existing_file(raw, format!("Scene {} audio", index + 1))?, start: 0.0, duration: 0.0, embedded: false });
        }
    } else if use_embedded {
        audio_items = scene_paths.iter().map(|p| SceneAudioItem { path: p.clone(), start: 0.0, duration: 0.0, embedded: true }).collect();
    } else if !audio_path.is_file() {
        return Err(format!("Audio file was not found: {}", audio_path.display()));
    }

    let mut timing = Vec::new();
    if !raw_timing.is_empty() {
        if timeline_fps == 0 {
            return Err("Timeline FPS is required when scene timing items are provided.".into());
        }
        if raw_timing.len() != scene_paths.len() {
            return Err("Scene timing item count does not match scene video count.".into());
        }
        for (index, item) in raw_timing.iter().enumerate() {
            let Some(item) = item.as_object() else { return Err(format!("Scene {} timing item is invalid.", index + 1)) };
            let start = float_of(item, "start").max(0.0);
            let end = item.get("end").filter(|v| truthy(v)).and_then(to_float).unwrap_or(start).max(start);
            timing.push(TimingItem { start, end });
        }
    }

    let mut overlays = Vec::new();
    for (index, item) in raw_overlays.iter().enumerate() {
        let Some(item) = item.as_object() else { return Err(format!("Insert {} item is invalid.", index + 1)) };
        let path = existing_file(item.get("path").unwrap_or(&Value::Null), format!("Insert {} video", index + 1))?;
        if !has_extension(&path, &VIDEO_TYPES) {
            return Err(format!("Insert {} media is not a supported video file: {}", index + 1, path.display()));
        }
        let start = float_of(item, "start").max(0.0);
        let end = item.get("end").filter(|v| truthy(v)).and_then(to_float).unwrap_or(start + 4.0).max(start + 0.05);
        overlays.push(OverlayItem { path, start, end, source_start: float_of(item, "source_start").max(0.0) });
    }

    let request = StitchRequest {
        project,
        scene_paths,
        audio_path: audio_path.is_file().then_some(audio_path),
        song_path: song,
        scene_audio_items: audio_items,
        scene_timing: timing,
        timeline_fps,
        use_embedded_scene_audio: use_embedded,
        overlays,
        width: int_in(payload, "width", 0, 0, 8192) as u32,
        height: int_in(payload, "height", 0, 0, 8192) as u32,
        audio_start: float_of(payload, "audio_start").max(0.0),
        audio_duration: float_of(payload, "audio_duration").max(0.0),
        output_prefix: match text_or_empty(payload.get("output_prefix")) {
            p if p.is_empty() => "FINAL_VIDEO".into(),
            p => p,
        },
    };
    let tools = env.tools()?;
    let used_audio = !request.scene_audio_items.is_empty();
    let embedded = request.use_embedded_scene_audio;
    let fps = request.timeline_fps;
    let result = stitch_scene_videos(&tools, &request).map_err(|e| e.to_string())?;
    Ok(json!({
        "final_video_path": text(&result.final_video_path),
        "video_folder": text(&result.video_folder),
        "concat_file": "",
        "scene_count": result.scene_count,
        "insert_count": result.insert_count,
        "used_scene_audio": used_audio,
        "used_embedded_scene_audio": embedded && used_audio,
        "embedded_audio_mode": result.embedded_audio_mode,
        "normalized_canvas": result.normalized_canvas,
        "timeline_frame_sync": result.timeline_frame_sync,
        "timeline_fps": if result.timeline_frame_sync { fps } else { 0 },
        "timeline_frame_count": result.timeline_frame_count,
        "output_width": result.output_width,
        "output_height": result.output_height,
        "output_duration": result.output_duration,
        "output_audio_duration": result.output_audio_duration,
        "removed_scratch_folders": [],
    }))
}

/// `POST .../render_image_slideshow`: the scene images, each for its scene's length, over the song.
pub fn render_image_slideshow(env: &Env, payload: &Obj) -> Reply {
    let raw_items = string_list(payload, "image_items");
    if raw_items.is_empty() {
        return Err("No scene images were provided for the slideshow preview.".into());
    }
    let (project, _target) = project_subfolder(payload, "slideshow_previews")?;
    let audio = abs_text(payload, "audio_path");
    if !audio.is_file() {
        return Err(format!("Global audio file was not found: {}", audio.display()));
    }
    let mut items = Vec::new();
    for (index, item) in raw_items.iter().enumerate() {
        let Some(item) = item.as_object() else { return Err(format!("Scene {} slideshow item is invalid.", index + 1)) };
        let path = existing_file(item.get("path").unwrap_or(&Value::Null), format!("Scene {} image", index + 1))?;
        if !has_extension(&path, &IMAGE_TYPES) {
            return Err(format!("Scene {} media is not a supported slideshow image: {}", index + 1, path.display()));
        }
        items.push(SlideItem { path, seconds: float_of(item, "duration").max(0.05) });
    }
    let request = SlideshowRequest {
        project,
        audio,
        audio_start: float_of(payload, "audio_start").max(0.0),
        width: int_in(payload, "width", 1920, 64, 8192) as u32,
        height: int_in(payload, "height", 1080, 64, 8192) as u32,
        fps: int_in(payload, "fps", 24, 1, 120) as u32,
        output_prefix: match text_or_empty(payload.get("output_prefix")) {
            p if p.is_empty() => "IMAGE_SLIDESHOW_PREVIEW".into(),
            p => p,
        },
        items,
    };
    let tools = env.tools()?;
    render_slideshow(&tools, &request).map_err(|e| e.to_string())
}

/// `POST .../prepare_scene_audio_clip`: the scene's slice of the song as a WAV for MiniMax.
pub fn prepare_scene_audio_clip(env: &Env, payload: &Obj) -> Reply {
    let source = abs_text(payload, "audio_path");
    let project = path_text(payload, "project_folder");
    if path_text(payload, "audio_path").is_empty() {
        return Err("Audio file path is empty.".into());
    }
    if !source.is_file() {
        return Err(format!("Audio file was not found: {}", source.display()));
    }
    if project.is_empty() {
        return Err("Create or load a project before preparing scene audio.".into());
    }
    let project = absolute(Path::new(&project));
    let number = |key: &str, default: f64, min: f64, max: f64| payload.get(key).and_then(to_float).unwrap_or(default).clamp(min, max);
    let scene = number("scene_number", 1.0, 1.0, 9999.0) as u32;
    let start = number("start_seconds", 0.0, 0.0, 24.0 * 60.0 * 60.0);
    let duration = number("duration_seconds", 8.0, 0.05, 120.0);
    let target_dir = project.join("minimax_h3_scene_audio");
    fs::create_dir_all(&target_dir).map_err(|e| format!("{}: {e}", target_dir.display()))?;
    let target = target_dir.join(format!("scene_audio_{scene:04}.wav"));
    let tools = env.tools()?;
    let args: Vec<String> = ["-y", "-ss", &format!("{start:.9}"), "-i", &text(&source), "-t", &format!("{duration:.9}"), "-vn", "-ac", "2", "-ar", "44100", "-c:a", "pcm_s16le", &text(&target)].iter().map(|s| s.to_string()).collect();
    tools.ffmpeg_checked(&args, "FFmpeg failed to prepare scene audio.").map_err(|e| e.to_string())?;
    if !target.is_file() {
        return Err("FFmpeg failed to prepare scene audio.".into());
    }
    let actual = h3_media::audio::probe_duration(&tools, &text(&target)).map_err(|e| e.to_string())?;
    Ok(json!({"audio_path": text(&target), "start": start, "duration": actual, "requested_duration": duration, "format": "pcm_s16le_wav"}))
}

/// `_save_minimax_h3_advanced_2pass_debug_workflow`: keep the exact graph that was built beside the scene's outputs, for
/// looking at it when a render goes wrong. Returns the file, or "" when the build named no output folder.
pub fn save_advanced_debug_graph(result: &Obj, payload: &Obj) -> Result<String, String> {
    let folder = text_or_empty(result.get("output_folder")).trim().to_string();
    if folder.is_empty() {
        return Ok(String::new());
    }
    let folder = absolute(Path::new(&folder));
    let scene = int_in(payload, "scene_number", 1, 1, 999_999);
    let path = folder.join(format!("scene_{scene:04}_minimax_h3_advanced_2pass_api.json"));
    let snapshot = json!({
        "workflow_type": "minimax_h3_advanced_2pass",
        "source_template": result.get("workflow_path").cloned().unwrap_or(json!("")),
        "advanced_two_pass": result.get("advanced_two_pass").cloned().unwrap_or(json!({})),
        "prompt": result.get("prompt").cloned().unwrap_or(json!({})),
    });
    h3_project::atomic::atomic_write_json(&path, &snapshot).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(text(&path))
}

/// The folders ComfyUI keeps its images in.
pub struct ComfyFolders {
    pub output: String,
    pub input: String,
    pub temp: String,
}

/// `_resolve_comfy_image_path`: the file a ComfyUI `/history` image entry (`{filename, subfolder, type}`) names.
pub fn resolve_comfy_image(folders: &ComfyFolders, info: &Obj) -> Result<PathBuf, String> {
    let filename = Path::new(&text_or_empty(info.get("filename"))).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    if filename.is_empty() {
        return Err("Image filename is empty.".into());
    }
    let base = match text_or_empty(info.get("type")).to_lowercase().as_str() {
        "temp" => folders.temp.clone(),
        "input" => folders.input.clone(),
        _ => folders.output.clone(),
    };
    let base = absolute(Path::new(&base));
    let folder = absolute(&base.join(text_or_empty(info.get("subfolder"))));
    if !inside(&base, &folder) {
        return Err("Image subfolder escapes the allowed ComfyUI folder.".into());
    }
    let source = folder.join(&filename);
    if !source.is_file() {
        return Err(format!("Generated image was not found: {}", source.display()));
    }
    Ok(source)
}

/// `POST .../save_image`: keep a picture ComfyUI made (named by its `/history` entry) in a folder.
pub fn save_generated_image(folders: &ComfyFolders, output_default: &Path, payload: &Obj) -> Reply {
    let Some(info) = payload.get("image").and_then(Value::as_object) else { return Err("Image info is missing.".into()) };
    let source = resolve_comfy_image(folders, info)?;
    let filename = source.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let requested = path_text(payload, "save_folder");
    let requested = if requested.is_empty() { "VRGDG_WorkflowRunner_Saved".to_string() } else { requested };
    let target_dir = if Path::new(&requested).is_absolute() { absolute(Path::new(&requested)) } else { absolute(&output_default.join(&requested)) };
    fs::create_dir_all(&target_dir).map_err(|e| format!("{}: {e}", target_dir.display()))?;
    let stem = Path::new(&filename).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let ext = Path::new(&filename).extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_else(|| ".png".into());
    let stamp = Stamp::now().compact;
    let mut target = target_dir.join(format!("{stem}_approved_{stamp}{ext}"));
    let mut counter = 2;
    while target.exists() {
        target = target_dir.join(format!("{stem}_approved_{stamp}_{counter}{ext}"));
        counter += 1;
    }
    fs::copy(&source, &target).map_err(|e| e.to_string())?;
    Ok(json!({"saved_path": text(&target), "save_folder": text(&target_dir)}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(value: Value) -> Obj {
        value.as_object().cloned().unwrap()
    }

    #[test]
    fn find_output_prefers_the_scene_named_clip() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        let scratch = project.join("image_to_video_clips_x").join("scene_0002");
        fs::create_dir_all(&scratch).unwrap();
        fs::write(scratch.join("video_0002-audio.mp4"), b"1234").unwrap();
        fs::write(scratch.join("video_0003-audio.mp4"), b"1234").unwrap();
        fs::write(scratch.join("video_0002-audio.mov"), b"1234").unwrap();
        let found = find_video_output(&obj(json!({"project_folder": text(project), "scene_number": 2, "video_mode": "i2v"}))).unwrap();
        assert!(found["video_path"].as_str().unwrap().ends_with("video_0002-audio.mp4"));
        let none = find_video_output(&obj(json!({"project_folder": text(project), "scene_number": 9, "video_mode": "t2v"}))).unwrap();
        assert_eq!(none["video_path"], "");
        assert!(find_video_output(&obj(json!({"project_folder": "Y:\\nowhere"}))).is_err());
        let strict = find_video_output(&obj(json!({"project_folder": text(project), "scene_number": 2, "strict_output_folder": true}))).unwrap();
        assert_eq!(strict["video_path"], "");
    }

    #[test]
    fn stitch_checks_the_request_before_working() {
        let env = Env { output: PathBuf::new(), roots: vec![], ffmpeg: "ffmpeg".into(), ffprobe: "ffprobe".into(), luts: PathBuf::new(), overlay_dir: None };
        assert_eq!(stitch_videos(&env, &obj(json!({"project_folder": "x"}))).unwrap_err(), "No scene video paths were provided.");
        assert_eq!(stitch_videos(&env, &obj(json!({"scene_paths": ["a.mp4"], "project_folder": ""}))).unwrap_err(), "Project folder is empty.");
        let dir = tempfile::tempdir().unwrap();
        let error = stitch_videos(&env, &obj(json!({"scene_paths": ["Y:\\gone.mp4"], "project_folder": text(dir.path())}))).unwrap_err();
        assert!(error.starts_with("Scene 1 video was not found"));
        let clip = dir.path().join("a.mp4");
        fs::write(&clip, b"x").unwrap();
        let error = stitch_videos(&env, &obj(json!({"scene_paths": [text(&clip)], "project_folder": text(dir.path())}))).unwrap_err();
        assert!(error.starts_with("Audio file was not found"));
    }

    #[test]
    fn slideshow_checks_the_request_before_working() {
        let env = Env { output: PathBuf::new(), roots: vec![], ffmpeg: "ffmpeg".into(), ffprobe: "ffprobe".into(), luts: PathBuf::new(), overlay_dir: None };
        assert!(render_image_slideshow(&env, &obj(json!({}))).unwrap_err().starts_with("No scene images"));
        let dir = tempfile::tempdir().unwrap();
        let error = render_image_slideshow(&env, &obj(json!({"image_items": [{"path": "x.png"}], "project_folder": text(dir.path()), "audio_path": "Y:\\none.wav"}))).unwrap_err();
        assert!(error.starts_with("Global audio file was not found"));
    }

    #[test]
    fn saved_image_comes_from_the_comfy_folder_and_stays_inside_it() {
        let dir = tempfile::tempdir().unwrap();
        let (output, temp) = (dir.path().join("output"), dir.path().join("temp"));
        fs::create_dir_all(output.join("sub")).unwrap();
        fs::create_dir_all(&temp).unwrap();
        fs::write(output.join("sub").join("a.png"), b"png").unwrap();
        let folders = ComfyFolders { output: text(&output), input: String::new(), temp: text(&temp) };
        let saved = save_generated_image(&folders, &output, &obj(json!({"image": {"filename": "a.png", "subfolder": "sub", "type": "output"}, "save_folder": "kept"}))).unwrap();
        assert!(saved["saved_path"].as_str().unwrap().contains("a_approved_"));
        assert!(Path::new(saved["saved_path"].as_str().unwrap()).is_file());
        assert!(save_generated_image(&folders, &output, &obj(json!({"image": {"filename": "a.png", "subfolder": "..", "type": "output"}}))).unwrap_err().contains("escapes"));
        assert!(save_generated_image(&folders, &output, &obj(json!({"image": {"filename": "missing.png", "type": "temp"}}))).unwrap_err().starts_with("Generated image was not found"));
        assert_eq!(save_generated_image(&folders, &output, &obj(json!({}))).unwrap_err(), "Image info is missing.");
    }

    #[test]
    fn scene_audio_clip_needs_a_project_and_a_song() {
        let env = Env { output: PathBuf::new(), roots: vec![], ffmpeg: "ffmpeg".into(), ffprobe: "ffprobe".into(), luts: PathBuf::new(), overlay_dir: None };
        assert_eq!(prepare_scene_audio_clip(&env, &obj(json!({}))).unwrap_err(), "Audio file path is empty.");
        assert!(prepare_scene_audio_clip(&env, &obj(json!({"audio_path": "Y:\\none.wav"}))).unwrap_err().starts_with("Audio file was not found"));
    }
}
