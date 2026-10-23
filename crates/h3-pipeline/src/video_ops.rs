//! Raw takes: the untrimmed renders a scene can be re-trimmed from. Port of `list_scene_takes` and `resolve_scene_take` in
//! `agent_api/orchestrator/video_orchestrator.py`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use h3_core::pyval::{round_half_even, round_to, text_or_empty, truthy, Obj};
use h3_graph::inputs::output_location_in;
use h3_media::video::probe_format_duration;
use h3_media::Tools;
use serde_json::{json, Value};

const TAKE_FPS: f64 = 24.0;

#[derive(Debug, thiserror::Error)]
pub enum VideoOpError {
    #[error("Scene '{0}' was not found.")]
    SceneNotFound(String),
    #[error("{0}")]
    Invalid(String),
}

fn key_of(path: &Path) -> String {
    let resolved = fs::canonicalize(path).unwrap_or_else(|_| std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()));
    let text = resolved.display().to_string();
    if cfg!(windows) { text.replace('/', "\\").to_lowercase() } else { text }
}

fn same_file(a: &Path, b: &Path) -> bool {
    key_of(a) == key_of(b)
}

fn mtime(path: &Path) -> Option<f64> {
    fs::metadata(path).ok()?.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs_f64())
}

fn take_entry(tools: &Tools, path: &Path, exists: bool, recorded: bool, active_raw: &str) -> Obj {
    let mut entry = Obj::new();
    entry.insert("path".into(), json!(path.display().to_string()));
    entry.insert("name".into(), json!(path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
    entry.insert("exists".into(), json!(exists));
    entry.insert("recorded".into(), json!(recorded));
    entry.insert("is_latest_render".into(), json!(!active_raw.is_empty() && same_file(path, Path::new(active_raw))));
    if exists {
        if let Ok(meta) = fs::metadata(path) {
            entry.insert("size_bytes".into(), json!(meta.len()));
            entry.insert("modified".into(), json!(mtime(path).unwrap_or(0.0)));
        }
        let duration = probe_format_duration(tools, path);
        if duration > 0.0 {
            entry.insert("duration_seconds".into(), json!(round_to(duration, 3)));
            entry.insert("frame_count".into(), json!(round_half_even(duration * TAKE_FPS)));
        }
    }
    entry
}

/// The scene's raw renders, newest first: the ones the app recorded plus MiniMax renders found in the scene's scratch
/// folder under ComfyUI's output folder. A take whose file is gone is listed with `exists: false`. A take in a sibling
/// scratch folder of the same project name is listed with `other_folder: true`.
pub fn list_scene_takes(tools: &Tools, output_dir: &str, project: &Path, session: &Obj, scene_ref: &str) -> Result<Value, VideoOpError> {
    let segments: Vec<&Value> = session.get("segments").and_then(Value::as_array).map(|a| a.iter().collect()).unwrap_or_default();
    let index = segments
        .iter()
        .position(|s| text_or_empty(s.get("id")) == scene_ref)
        .or_else(|| (0..segments.len()).find(|i| (i + 1).to_string() == scene_ref))
        .ok_or_else(|| VideoOpError::SceneNotFound(scene_ref.to_string()))?;
    let segment = segments[index];
    let scene_number = index as u32 + 1;
    let (scratch, _) = output_location_in(output_dir, &project.display().to_string(), scene_number);
    let scratch = PathBuf::from(scratch);
    let recorded: Vec<String> = segment.get("raw_video_history").and_then(Value::as_array).map(|a| a.iter().map(|v| text_or_empty(Some(v))).filter(|p| !p.trim().is_empty()).collect()).unwrap_or_default();
    let active_raw = text_or_empty(segment.get("raw_video_path"));
    let mut found: Vec<(String, Obj)> = Vec::new();
    let mut seen: HashMap<String, ()> = HashMap::new();
    for path in &recorded {
        let path = Path::new(path);
        let key = key_of(path);
        if seen.insert(key.clone(), ()).is_none() {
            found.push((key, take_entry(tools, path, path.is_file(), true, &active_raw)));
        } else if let Some(slot) = found.iter_mut().find(|(k, _)| *k == key) {
            slot.1 = take_entry(tools, path, path.is_file(), true, &active_raw);
        }
    }
    // The scratch folder name ends with a hash of the project folder text, so a project rendered under a different
    // spelling of its path has its takes in a sibling folder with the same project name. Those are listed too, marked
    // `other_folder` and never picked by `latest`.
    let project_dir = scratch.parent().map(Path::to_path_buf).unwrap_or_default();
    let base = project_dir.parent().map(Path::to_path_buf).unwrap_or_default();
    let project_name = project_dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let prefix = project_name.rsplit_once('_').map_or(project_name.as_str(), |(head, _)| head).to_string();
    let scratch_name = scratch.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    let mut candidates: Vec<(PathBuf, bool)> = vec![(scratch.clone(), false)];
    if base.is_dir() {
        let mut siblings: Vec<String> = fs::read_dir(&base).map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
        siblings.sort();
        for sibling in siblings {
            let sibling_prefix = sibling.rsplit_once('_').map_or(sibling.as_str(), |(head, _)| head);
            let sibling_dir = base.join(&sibling);
            if sibling != project_name && sibling_prefix == prefix && sibling_dir.is_dir() {
                candidates.push((sibling_dir.join(&scratch_name), true));
            }
        }
    }
    for (folder, other) in candidates {
        let Ok(entries) = fs::read_dir(&folder) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let lowered = name.to_lowercase();
            let path = entry.path();
            if lowered.ends_with("-audio.mp4") && !lowered.contains("stage1") && path.is_file() {
                let key = key_of(&path);
                if seen.insert(key.clone(), ()).is_none() {
                    let mut take = take_entry(tools, &path, true, false, &active_raw);
                    take.insert("other_folder".into(), json!(other));
                    found.push((key, take));
                }
            }
        }
    }
    let mut takes: Vec<Obj> = found.into_iter().map(|(_, t)| t).collect();
    takes.sort_by(|a, b| b.get("modified").and_then(Value::as_f64).unwrap_or(0.0).total_cmp(&a.get("modified").and_then(Value::as_f64).unwrap_or(0.0)));
    for (i, take) in takes.iter_mut().enumerate() {
        take.insert("index".into(), json!(i));
    }
    let usable = takes.iter().filter(|t| t.get("exists").is_some_and(truthy)).count();
    let note = if usable > 0 { "" } else { "No raw take is on disk. Render the scene through the API again; the Video Builder deletes its scratch renders after each render." };
    Ok(json!({
        "scene_id": segment.get("id").cloned().unwrap_or(Value::Null),
        "scene_number": scene_number,
        "scratch_folder": scratch.display().to_string(),
        "takes": takes,
        "usable": usable,
        "note": note,
    }))
}

/// A finished but uncollected scene clip inside the project: the Video Builder's old scratch folders
/// (`image_to_video_clips*` and the like) searched for `video_NNNN-audio.mp4`. The best name match wins, then the newest.
pub fn find_scene_video_output(project: &Path, video_mode: &str, scene_number: u32) -> Option<PathBuf> {
    let prefix = match video_mode.trim().to_lowercase().as_str() {
        "rtv" => "reference_to_video_clips",
        "t2v" => "text_to_video_clips",
        "ingredients" => "ingredients_to_video_clips",
        "id_lora" => "id_lora_i2v_clips",
        _ => "image_to_video_clips",
    };
    let mut folders: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(project).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.path().is_dir() && (name == prefix || name.starts_with(&format!("{prefix}_"))) {
            folders.push(entry.path());
        }
    }
    let (padded, mut best): (String, Option<(i64, f64, PathBuf)>) = (format!("{scene_number:04}"), None);
    let mut stack = folders;
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let lower = name.to_lowercase();
            if !lower.ends_with("-audio.mp4") {
                continue;
            }
            let Ok(meta) = fs::metadata(&path) else { continue };
            if meta.len() == 0 {
                continue;
            }
            let modified = mtime(&path).unwrap_or(0.0);
            let mut score = 0;
            if scene_number > 0 && lower == format!("video_{padded}-audio.mp4") {
                score += 1000;
            }
            if scene_number > 0 && (lower.starts_with(&format!("video_{padded}_")) || lower.starts_with(&format!("video_{padded}-"))) {
                score += 700;
            }
            if scene_number > 0 && name.contains(&format!("_{padded}_")) {
                score += 100;
            }
            if best.as_ref().is_none_or(|(s, m, _)| (score, modified) > (*s, *m)) {
                best = Some((score, modified, path));
            }
        }
    }
    best.map(|(_, _, p)| p)
}

/// The file of a raw take named by `latest` or by its index in [`list_scene_takes`] (newest first).
pub fn resolve_scene_take(listing: &Value, scene_ref: &str, take: &Value) -> Result<String, VideoOpError> {
    let all: Vec<&Value> = listing["takes"].as_array().map(|a| a.iter().collect()).unwrap_or_default();
    let usable: Vec<&&Value> = all.iter().filter(|t| t["exists"].as_bool().unwrap_or(false)).collect();
    if usable.is_empty() {
        return Err(VideoOpError::Invalid(format!("Scene '{scene_ref}' has no raw take on disk to trim. {}", listing["note"].as_str().unwrap_or(""))));
    }
    let named = match take {
        Value::String(s) => s.trim().to_lowercase(),
        Value::Null => String::new(),
        other => other.to_string(),
    };
    if matches!(named.as_str(), "latest" | "newest" | "") {
        let own: Vec<&&&Value> = usable.iter().filter(|t| !t["other_folder"].as_bool().unwrap_or(false)).collect();
        return match own.first() {
            Some(t) => Ok(t["path"].as_str().unwrap_or("").to_string()),
            None => Err(VideoOpError::Invalid(format!(
                "Scene '{scene_ref}' has raw takes only in another scratch folder with the same project name (rendered under a different path spelling). Name one by its index from GET .../video/takes if it is yours."
            ))),
        };
    }
    let wanted: i64 = match take {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => s.trim().parse().ok(),
        Value::Bool(b) => Some(i64::from(*b)),
        _ => None,
    }
    .ok_or_else(|| VideoOpError::Invalid("take must be 'latest' or the index of a take from GET .../video/takes.".into()))?;
    let found = all.iter().find(|t| t["index"].as_i64() == Some(wanted)).ok_or_else(|| VideoOpError::Invalid(format!("Scene '{scene_ref}' has no take {wanted}; it has {}.", all.len())))?;
    if !found["exists"].as_bool().unwrap_or(false) {
        return Err(VideoOpError::Invalid(format!("Take {wanted} of scene '{scene_ref}' was recorded but its file is gone: {}", found["path"].as_str().unwrap_or(""))));
    }
    Ok(found["path"].as_str().unwrap_or("").to_string())
}
