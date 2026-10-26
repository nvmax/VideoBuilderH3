//! Record a rendered clip on its scene the way the Video Builder does. Port of `apply_scene_video` in
//! `agent_api/scene_video.py` and `_continuity_used` in `agent_api/orchestrator/video_orchestrator.py`.
//!
//! The timeline shows a scene's picture from `video_thumbnail_path` and its history from `video_history` with the
//! parallel `video_thumbnail_history`. Setting only `video_path` would leave the scene on the timeline with no picture.

use std::path::Path;

use h3_core::continuity::{canonical_continuity_mode, MASKED};
use h3_core::pyval::{text_or_empty, to_int, truthy, Obj};
use h3_media::video::thumbnail_path_for;
use serde_json::{json, Value};

/// Comparison key of a path: absolute, lower case, forward slashes (`normcase(abspath(p))` on Windows).
fn key_of(path: &str) -> String {
    if path.trim().is_empty() {
        return String::new();
    }
    h3_core::abspath::absolute(Path::new(path)).map(|p| p.display().to_string()).unwrap_or_else(|_| path.to_string()).to_lowercase().replace('\\', "/")
}

/// `os.path.dirname` for a Windows or Unix path.
fn dirname(path: &str) -> String {
    let Some(index) = path.rfind(['/', '\\']) else { return String::new() };
    let head = &path[..=index];
    let trimmed = head.trim_end_matches(['/', '\\']);
    // A drive or filesystem root keeps its separator.
    if trimmed.is_empty() || (trimmed.len() == 2 && trimmed.ends_with(':')) { head.to_string() } else { trimmed.to_string() }
}

fn thumbnail_for(video: &str, given: &str) -> String {
    if !given.trim().is_empty() {
        return given.trim().to_string();
    }
    let guess = thumbnail_path_for(Path::new(video));
    if guess.is_file() { guess.display().to_string() } else { String::new() }
}

fn text(value: &Value) -> String {
    text_or_empty(Some(value))
}

/// Make `video_path` the scene's current video, with its thumbnail and history entry. `now_millis` is the cache bust.
pub fn apply_scene_video(segment: &mut Obj, video_path: &str, thumbnail_path: &str, now_millis: i64) {
    let video = video_path.trim();
    if video.is_empty() {
        return;
    }
    let thumbnail = thumbnail_for(video, thumbnail_path);
    segment.insert("video_path".into(), json!(video));
    segment.insert("rendered_video_path".into(), json!(video));
    segment.insert("video_folder".into(), json!(dirname(video)));
    segment.insert("video_status".into(), json!("done"));
    segment.insert("scene_audio_render_dirty".into(), json!(false));
    segment.insert("preview_mode".into(), json!("video"));
    if !thumbnail.is_empty() {
        segment.insert("thumbnail_path".into(), json!(thumbnail));
    }

    let list = |key: &str| segment.get(key).filter(|v| truthy(v)).and_then(Value::as_array).cloned().unwrap_or_default();
    let mut history: Vec<String> = list("video_history").iter().map(text).filter(|p| !p.trim().is_empty()).collect();
    let mut thumbs: Vec<String> = list("video_thumbnail_history").iter().map(|t| if truthy(t) { text(t) } else { String::new() }).collect();
    while thumbs.len() < history.len() {
        thumbs.push(String::new());
    }
    let mut by_key: Vec<(String, String)> = Vec::new();
    for (i, p) in history.iter().enumerate() {
        let key = key_of(p);
        match by_key.iter_mut().find(|(k, _)| *k == key) {
            Some(entry) => entry.1 = thumbs[i].clone(),
            None => by_key.push((key, thumbs[i].clone())),
        }
    }
    let video_key = key_of(video);
    if !by_key.iter().any(|(k, _)| *k == video_key) {
        history.push(video.to_string());
    }
    if !thumbnail.is_empty() {
        match by_key.iter_mut().find(|(k, _)| *k == video_key) {
            Some(entry) => entry.1 = thumbnail.clone(),
            None => by_key.push((video_key.clone(), thumbnail.clone())),
        }
    }
    let lookup = |p: &str| by_key.iter().find(|(k, _)| *k == key_of(p)).map(|(_, t)| t.clone()).unwrap_or_default();
    let thumb_history: Vec<String> = history.iter().map(|p| lookup(p)).collect();
    let index = history.iter().position(|p| key_of(p) == video_key).unwrap_or(0);
    segment.insert("video_history".into(), json!(history));
    segment.insert("video_thumbnail_history".into(), json!(thumb_history));
    segment.insert("video_history_index".into(), json!(index));
    segment.insert("video_thumbnail_path".into(), json!(thumb_history.get(index).cloned().unwrap_or_default()));
    segment.insert("video_cache_bust".into(), json!(now_millis));
}

/// The continuity a MiniMax render really used, as the Video Builder records it on the scene. The answer comes from the
/// graph that was built (its `latent_continuation_settings` say whether a predecessor latent was loaded and from which
/// scene). A graph that does not report it falls back to the resolved mode.
pub fn continuity_used(mode: &str, report: Option<&Value>, segments: &[Value], scene_number: usize) -> Obj {
    let default_pred = scene_number as i64 - 1;
    let (used, predecessor) = match report.and_then(Value::as_object) {
        Some(map) => {
            let used = map.get("enabled").is_some_and(truthy);
            let given = map.get("predecessor_scene").filter(|v| truthy(v));
            let predecessor = match given {
                None => default_pred,
                Some(v) => to_int(v).unwrap_or(default_pred),
            };
            (used, predecessor)
        }
        None => (canonical_continuity_mode(Some(&json!(mode))) == Some(MASKED) && scene_number > 1, default_pred),
    };
    let mut out = Obj::new();
    if !used || predecessor < 1 || predecessor as usize > segments.len() {
        out.insert("minimax_h3_continuity_mode_used".into(), json!("off"));
        out.insert("minimax_h3_continuity_source_scene_id".into(), json!(""));
        return out;
    }
    let source = segments[predecessor as usize - 1].get("id").filter(|v| truthy(v)).map(text).unwrap_or_default();
    out.insert("minimax_h3_continuity_mode_used".into(), json!(MASKED));
    out.insert("minimax_h3_continuity_source_scene_id".into(), json!(source));
    out
}
