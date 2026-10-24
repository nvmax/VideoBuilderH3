//! Repair a session's file paths when it is opened. Port of `_rehydrate_builder_session` in `builder/project.py` and the
//! path helpers it uses in `builder/project_copy.py`.
//!
//! A project can be moved, copied or imported from a zip, so the paths saved inside its session may point at the old
//! folder. Loading rebases every path that lived in the old folder, finds files again by name, picks up approved images,
//! image history and rendered videos that exist on disk, and rebuilds scenes from loose files when a session has none.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use h3_core::pyval::{text_or_empty, to_float, truthy, Obj};
use serde_json::{json, Value};

use crate::layout::{context_folder, images_folder};
use crate::session::assign_overlay_slots;

const IMAGE_EXTENSIONS: [&str; 4] = [".png", ".jpg", ".jpeg", ".webp"];

/// `os.path.abspath`: a full path with `..` resolved and no trailing separator.
pub fn absolute(path: &Path) -> PathBuf {
    let full = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    // `Path::components` drops a trailing separator and a `.` in the middle.
    full.components().collect()
}

fn normcase(path: &Path) -> String {
    let text = path.display().to_string();
    if cfg!(windows) {
        text.replace('/', "\\").to_lowercase()
    } else {
        text
    }
}

/// `_is_inside_folder`: `path` is the folder or lies under it (case blind on Windows).
pub fn is_inside_folder(path: &Path, folder: &Path) -> bool {
    let path = PathBuf::from(normcase(&absolute(path)));
    let folder = PathBuf::from(normcase(&absolute(folder)));
    path.starts_with(&folder)
}

pub fn relative_to(path: &Path, folder: &Path) -> PathBuf {
    let path = absolute(path);
    let folder = absolute(folder);
    let (p, f) = (path.components().collect::<Vec<_>>(), folder.components().collect::<Vec<_>>());
    let skip = f.len().min(p.len());
    p[skip..].iter().collect()
}

fn trimmed(raw: &str) -> &str {
    raw.trim().trim_matches('"')
}

/// `_project_rebased_path`: where a path under the old project folder lives in the current one, or "" when it is not
/// under the old folder.
pub fn project_rebased_path(project: &Path, old_project: &str, raw: &str) -> String {
    let text = trimmed(raw);
    if text.is_empty() || old_project.is_empty() {
        return String::new();
    }
    let old = absolute(Path::new(old_project));
    let raw_abs = absolute(Path::new(text));
    if is_inside_folder(&raw_abs, &old) {
        return absolute(&project.join(relative_to(&raw_abs, &old))).display().to_string();
    }
    String::new()
}

fn scene_image_path(project: &Path, scene: i64, ext: &str) -> PathBuf {
    images_folder(project).join(format!("image_{:04}{ext}", scene.max(1)))
}

fn scene_audio_path(project: &Path, scene: i64, ext: &str) -> PathBuf {
    project.join("scene_audio").join(format!("audio_{:04}{ext}", scene.max(1)))
}

/// `_project_path_candidates`.
fn path_candidates(project: &Path, old_project: &str, raw: &str, scene: Option<i64>) -> Vec<PathBuf> {
    let text = trimmed(raw);
    if text.is_empty() {
        return Vec::new();
    }
    let abs_text = absolute(Path::new(text));
    let mut out = vec![PathBuf::from(text), abs_text.clone()];
    if !old_project.is_empty() {
        let old = absolute(Path::new(old_project));
        if is_inside_folder(&abs_text, &old) {
            out.push(project.join(relative_to(&abs_text, &old)));
        }
    }
    // `os.path.basename` splits on both separators on Windows.
    let base = text.rsplit(['\\', '/']).next().unwrap_or("");
    if !base.is_empty() {
        out.extend([
            project.join(base),
            images_folder(project).join(base),
            context_folder(project).join(base),
            project.join("project_audio").join(base),
            project.join("scene_audio").join(base),
            project.join("rendered_scene_videos").join(base),
        ]);
    }
    if let Some(scene) = scene.filter(|s| *s != 0) {
        for ext in IMAGE_EXTENSIONS {
            out.push(scene_image_path(project, scene, ext));
        }
        for ext in [".wav", ".mp3", ".m4a"] {
            out.push(scene_audio_path(project, scene, ext));
        }
        out.push(project.join("rendered_scene_videos").join(format!("video_{scene:04}-audio.mp4")));
    }
    out
}

/// `_resolve_project_asset_path`: the first candidate that is a file, else the saved text unchanged.
pub fn resolve_project_asset_path(project: &Path, old_project: &str, raw: &str, scene: Option<i64>) -> String {
    for candidate in path_candidates(project, old_project, raw, scene) {
        if candidate.is_file() {
            return absolute(&candidate).display().to_string();
        }
    }
    raw.to_string()
}

fn resolve_value(project: &Path, old_project: &str, value: Option<&Value>, scene: Option<i64>) -> String {
    resolve_project_asset_path(project, old_project, &text_or_empty(value), scene)
}

fn rebase_nested(value: &mut Value, project: &Path, old_project: &str) {
    match value {
        Value::Object(map) => map.values_mut().for_each(|v| rebase_nested(v, project, old_project)),
        Value::Array(items) => items.iter_mut().for_each(|v| rebase_nested(v, project, old_project)),
        Value::String(text) => {
            if old_project.is_empty() || !Path::new(text.as_str()).is_absolute() {
                return;
            }
            let rebased = project_rebased_path(project, old_project, text);
            if !rebased.is_empty() && Path::new(&rebased).exists() {
                *text = rebased;
            }
        }
        _ => {}
    }
}

fn numbers_from_folder(folder: &Path, prefix: &str, suffixes: &[&str], out: &mut BTreeSet<i64>) {
    let Ok(entries) = fs::read_dir(folder) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_lowercase();
        let Some(rest) = name.strip_prefix(prefix) else { continue };
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() {
            continue;
        }
        if suffixes.iter().any(|s| &rest[digits.len()..] == *s) && entry.path().is_file() {
            if let Ok(n) = digits.parse() {
                out.insert(n);
            }
        }
    }
}

/// `_project_scene_numbers`: scene numbers that have an image, a rendered video or a preview folder.
fn project_scene_numbers(project: &Path) -> BTreeSet<i64> {
    let mut numbers = BTreeSet::new();
    numbers_from_folder(&images_folder(project), "image_", &[".png", ".jpg", ".jpeg", ".webp"], &mut numbers);
    numbers_from_folder(&project.join("rendered_scene_videos"), "video_", &["-audio.mp4"], &mut numbers);
    if let Ok(entries) = fs::read_dir(project.join("scene_image_previews")) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if let Some(digits) = name.strip_prefix("scene_") {
                if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) && entry.path().is_dir() {
                    if let Ok(n) = digits.parse() {
                        numbers.insert(n);
                    }
                }
            }
        }
    }
    numbers
}

/// `_scene_preview_paths`: preview images of a scene, oldest first.
fn scene_preview_paths(project: &Path, scene: i64) -> Vec<String> {
    let folder = project.join("scene_image_previews").join(format!("scene_{:04}", scene.max(1)));
    let Ok(entries) = fs::read_dir(&folder) else { return Vec::new() };
    let mut found: Vec<(String, std::time::SystemTime, String)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let lower = entry.file_name().to_string_lossy().to_lowercase();
        let image = IMAGE_EXTENSIONS.iter().any(|e| lower.ends_with(e));
        if path.is_file() && image {
            let modified = fs::metadata(&path).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
            found.push((lower, modified, absolute(&path).display().to_string()));
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found.sort_by_key(|item| item.1);
    found.into_iter().map(|item| item.2).collect()
}

pub fn is_internal_approved_image(path: &str) -> bool {
    path.replace('/', "\\").split('\\').any(|part| part.eq_ignore_ascii_case("zimage_approved"))
}

fn is_recovered(segment: &Obj) -> bool {
    text_or_empty(segment.get("source")).to_lowercase() == "recovered" || text_or_empty(segment.get("id")).starts_with("recovered_scene_")
}

fn number(segment: &Obj, key: &str) -> f64 {
    segment.get(key).filter(|v| truthy(v)).and_then(to_float).unwrap_or(0.0)
}

const SEGMENT_PATH_KEYS: [&str; 8] = [
    "approved_image_path",
    "custom_image_path",
    "first_last_frame_end_image_path",
    "ref_image_path",
    "flux_subject_image_path",
    "flux_location_image_path",
    "video_path",
    "custom_audio_path",
];

fn resolve_ingredients(list: &mut [Value], project: &Path, old_project: &str, scene: Option<i64>) {
    for ingredient in list.iter_mut().filter_map(Value::as_object_mut) {
        let path = resolve_value(project, old_project, ingredient.get("path"), scene);
        ingredient.insert("path".into(), json!(path));
    }
}

/// Resolve the saved paths of one segment (timeline scene or insert), in place.
fn resolve_segment_files(segment: &mut Obj, project: &Path, old_project: &str, scene: i64) {
    for key in SEGMENT_PATH_KEYS {
        let path = resolve_value(project, old_project, segment.get(key), Some(scene));
        segment.insert(key.into(), json!(path));
    }
    let history: Vec<Value> = match segment.get("image_history") {
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| resolve_value(project, old_project, Some(item), Some(scene)))
            .filter(|p| !p.is_empty())
            .map(Value::from)
            .collect(),
        _ => Vec::new(),
    };
    segment.insert("image_history".into(), Value::Array(history));
}

fn mark_rendered_video(segment: &mut Obj, project: &Path, scene: i64) {
    let video = project.join("rendered_scene_videos").join(format!("video_{scene:04}-audio.mp4"));
    if video.is_file() {
        let video = absolute(&video);
        segment.insert("video_path".into(), json!(video.display().to_string()));
        segment.insert("video_folder".into(), json!(video.parent().map(|p| p.display().to_string()).unwrap_or_default()));
        segment.insert("video_status".into(), json!("done"));
    }
}

fn history_strings(segment: &Obj) -> Vec<String> {
    segment
        .get("image_history")
        .and_then(Value::as_array)
        .map(|items| items.iter().map(|v| text_or_empty(Some(v))).collect())
        .unwrap_or_default()
}

fn set_history(segment: &mut Obj, history: Vec<String>) {
    segment.insert("image_history".into(), Value::Array(history.into_iter().map(Value::from).collect()));
}

/// Fix up a session read from `project`.
pub fn rehydrate_session(project: &Path, mut session: Obj) -> Obj {
    let old_project = text_or_empty(session.get("project_folder"));
    let project = absolute(project);

    if !old_project.is_empty() {
        for value in session.values_mut() {
            rebase_nested(value, &project, &old_project);
        }
    }
    session.insert("project_folder".into(), json!(project.display().to_string()));

    let manifest = project.join("subject_location").join("reference_descriptions.json");
    if !session.get("flux_reference_builder").is_some_and(Value::is_object) && manifest.is_file() {
        let parsed = fs::read_to_string(&manifest).ok().and_then(|text| crate::pyjson::parse(text.trim_start_matches('\u{feff}')).ok());
        if let Some(Value::Object(found)) = parsed {
            if found.get("subjects").is_some_and(Value::is_array) && found.get("locations").is_some_and(Value::is_array) {
                session.insert("flux_reference_builder".into(), Value::Object(found));
            }
        }
    }

    let context = context_folder(&project);
    for (key, file) in [("theme_style_path", "themestyle.txt"), ("story_idea_path", "storyconcept.txt"), ("subject_scene_path", "subjectsandscenes.txt")] {
        if !session.contains_key(key) {
            session.insert(key.into(), json!(context.join(file).display().to_string()));
        }
    }
    for key in ["audio_path", "prompt_json_path", "theme_style_path", "story_idea_path", "subject_scene_path"] {
        let path = resolve_value(&project, &old_project, session.get(key), None);
        session.insert(key.into(), json!(path));
    }
    if let Some(Value::Array(list)) = session.get_mut("flux_global_image_ingredients") {
        resolve_ingredients(list, &project, &old_project, None);
    }

    if !session.get("segments").is_some_and(Value::is_array) {
        session.insert("segments".into(), json!([]));
    }
    if !session.get("overlay_segments").is_some_and(Value::is_array) {
        session.insert("overlay_segments".into(), json!([]));
    }
    if let Some(Value::Array(overlay)) = session.get_mut("overlay_segments") {
        assign_overlay_slots(overlay);
    }

    // Scenes are rebuilt from loose media only when the session has no scene list, so deleted scenes do not come back.
    if let Some(Value::Array(segments)) = session.get_mut("segments") {
        if segments.is_empty() {
            let target = project_scene_numbers(&project).into_iter().filter(|n| *n < 10000).max().unwrap_or(0);
            for index in 1..=target {
                let start = ((index - 1) * 4) as f64;
                segments.push(json!({"id": format!("recovered_scene_{index}"), "label": format!("Scene {index}"), "start": start, "end": start + 4.0, "source": "recovered"}));
            }
        }
        let snapshot = segments.clone();
        let kept: Vec<Value> = segments
            .iter()
            .filter(|segment| {
                let Some(segment) = segment.as_object() else { return false };
                if !is_recovered(segment) {
                    return true;
                }
                let (start, end) = (number(segment, "start"), segment.get("end").filter(|v| truthy(v)).and_then(to_float).unwrap_or(number(segment, "start")));
                let overlaps_real = snapshot.iter().filter_map(Value::as_object).any(|other| {
                    if is_recovered(other) {
                        return false;
                    }
                    let other_start = number(other, "start");
                    let other_end = other.get("end").filter(|v| truthy(v)).and_then(to_float).unwrap_or(other_start);
                    end.min(other_end) - start.max(other_start) > 0.05
                });
                !overlaps_real
            })
            .cloned()
            .collect();
        *segments = kept;
    }

    if let Some(Value::Array(segments)) = session.get_mut("segments") {
        for (i, segment) in segments.iter_mut().enumerate() {
            let index = i as i64 + 1;
            let Some(segment) = segment.as_object_mut() else { continue };
            let label = text_or_empty(segment.get("label"));
            if label.trim().is_empty() || label.to_lowercase() == "new scene" {
                segment.insert("label".into(), json!(format!("Scene {index}")));
            }
            resolve_segment_files(segment, &project, &old_project, index);
            if let Some(Value::Array(list)) = segment.get_mut("flux_image_ingredients") {
                resolve_ingredients(list, &project, &old_project, Some(index));
            }
            let mut approved = resolve_value(&project, &old_project, segment.get("approved_image_path"), Some(index));
            let cleared = segment.get("image_assignment_cleared").is_some_and(truthy);
            if approved.is_empty() && !cleared {
                for ext in IMAGE_EXTENSIONS {
                    let candidate = scene_image_path(&project, index, ext);
                    if candidate.is_file() {
                        approved = absolute(&candidate).display().to_string();
                        break;
                    }
                }
            }
            let mut history = history_strings(segment);
            if !approved.is_empty() && Path::new(&approved).is_file() {
                segment.insert("approved_image_path".into(), json!(approved));
                history.retain(|item| *item != approved && !is_internal_approved_image(item));
            }
            if !cleared {
                for preview in scene_preview_paths(&project, index) {
                    if !history.contains(&preview) {
                        history.push(preview);
                    }
                }
            }
            let has_index = segment.get("image_history_index").is_some_and(|v| v.is_i64() || v.is_u64());
            if !history.is_empty() && !has_index {
                segment.insert("image_history_index".into(), json!(history.len() - 1));
            }
            set_history(segment, history);
            mark_rendered_video(segment, &project, index);
        }
    }

    let slots: Vec<i64> = session
        .get("overlay_segments")
        .and_then(Value::as_array)
        .map(|list| list.iter().enumerate().map(|(i, s)| overlay_number(s, i as i64 + 1)).collect())
        .unwrap_or_default();
    if let Some(Value::Array(overlay)) = session.get_mut("overlay_segments") {
        for (i, segment) in overlay.iter_mut().enumerate() {
            let index = i as i64 + 1;
            let scene = slots[i];
            let Some(segment) = segment.as_object_mut() else { continue };
            let label = text_or_empty(segment.get("label"));
            if label.trim().is_empty() || label.to_lowercase() == "new scene" {
                segment.insert("label".into(), json!(format!("Insert {index}")));
            }
            segment.insert("track".into(), json!("overlay"));
            resolve_segment_files(segment, &project, &old_project, scene);
            let mut history = history_strings(segment);
            for preview in scene_preview_paths(&project, scene) {
                if !history.contains(&preview) {
                    history.push(preview);
                }
            }
            set_history(segment, history);
            mark_rendered_video(segment, &project, scene);
        }
    }
    session
}

/// `_overlay_scene_number`.
fn overlay_number(segment: &Value, fallback_index: i64) -> i64 {
    if let Some(segment) = segment.as_object() {
        for key in ["overlay_slot_number", "scene_slot_number", "slot_number"] {
            let value = segment.get(key).filter(|v| truthy(v)).and_then(to_float).map_or(0, |f| f as i64);
            if value >= 10001 {
                return value;
            }
        }
    }
    10000 + fallback_index.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(value: Value) -> Obj {
        value.as_object().cloned().unwrap()
    }

    #[test]
    fn moved_project_gets_paths_rebased_and_media_found() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("Song");
        fs::create_dir_all(project.join("zimage_approved")).unwrap();
        fs::create_dir_all(project.join("rendered_scene_videos")).unwrap();
        fs::write(project.join("zimage_approved").join("image_0001.png"), b"x").unwrap();
        fs::write(project.join("rendered_scene_videos").join("video_0002-audio.mp4"), b"x").unwrap();
        let old = if cfg!(windows) { r"D:\Old\Song" } else { "/old/Song" };
        let session = obj(json!({
            "project_folder": old,
            "segments": [{"start": 0, "end": 4}, {"start": 4, "end": 8, "label": "New Scene"}],
        }));
        let out = rehydrate_session(&project, session);
        let segments = out["segments"].as_array().unwrap();
        assert_eq!(segments[0]["label"], "Scene 1");
        assert_eq!(segments[1]["label"], "Scene 2");
        assert!(segments[0]["approved_image_path"].as_str().unwrap().ends_with("image_0001.png"));
        assert_eq!(segments[0]["image_history"], json!([]));
        assert_eq!(segments[1]["video_status"], "done");
        assert!(segments[1]["video_path"].as_str().unwrap().ends_with("video_0002-audio.mp4"));
        assert_eq!(segments[1]["approved_image_path"], "");
        assert_eq!(out["project_folder"], json!(absolute(&project).display().to_string()));
        assert!(out["theme_style_path"].as_str().unwrap().ends_with("themestyle.txt"));
    }

    #[test]
    fn empty_session_recovers_scenes_from_loose_files() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("P");
        fs::create_dir_all(project.join("zimage_approved")).unwrap();
        fs::write(project.join("zimage_approved").join("image_0003.jpg"), b"x").unwrap();
        let out = rehydrate_session(&project, obj(json!({})));
        let segments = out["segments"].as_array().unwrap();
        assert_eq!(segments.len(), 3);
        assert_eq!(segments[2]["id"], "recovered_scene_3");
        assert_eq!(segments[2]["start"], json!(8.0));
    }

    #[test]
    fn recovered_scene_overlapping_a_real_one_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("P");
        fs::create_dir_all(&project).unwrap();
        let session = obj(json!({"segments": [
            {"id": "a", "start": 0, "end": 4},
            {"id": "recovered_scene_2", "start": 1, "end": 3, "source": "recovered"},
            {"id": "recovered_scene_3", "start": 10, "end": 14, "source": "recovered"},
        ]}));
        let out = rehydrate_session(&project, session);
        let ids: Vec<&str> = out["segments"].as_array().unwrap().iter().map(|s| s["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["a", "recovered_scene_3"]);
    }
}
