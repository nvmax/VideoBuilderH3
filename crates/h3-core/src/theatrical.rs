//! Theatrical prompt directions: the camera flow, performance and facial presets the Builder offers, written into a prompt when one is selected.
//! Port of `storyboard/theatrical_presets.py`. The texts are the pack's snapshot (`data/theatrical_presets.json`, made by `tools/export_theatrical_presets.py`).

use std::sync::OnceLock;

use serde_json::Value;

use crate::pyval::{truthy, Obj};

fn presets() -> &'static Value {
    static DATA: OnceLock<Value> = OnceLock::new();
    DATA.get_or_init(|| serde_json::from_str(include_str!("../data/theatrical_presets.json")).expect("theatrical presets"))
}

fn text_at(path: &[&str]) -> &'static str {
    path.iter().try_fold(presets(), |value, key| value.get(*key)).and_then(Value::as_str).unwrap_or("")
}

/// The facial performance preset's direction, the text the lyric-free prompts use for "theatrical".
pub fn facial_direction() -> &'static str {
    text_at(&["facial", "direction"])
}

/// The Builder's project keys a scene falls back on, including the separate facial defaults (`default_facial_performance` and its custom text).
pub fn project_defaults(session: &Obj) -> Obj {
    let mut defaults = Obj::new();
    defaults.insert("facial_performance".into(), session.get("default_facial_performance").cloned().unwrap_or(Value::Null));
    defaults.insert("facial_performance_custom".into(), session.get("default_facial_performance_custom").cloned().unwrap_or(Value::Null));
    if let Some(stored) = session.get("builder_storyboard_defaults").and_then(Value::as_object) {
        defaults.extend(stored.clone());
    }
    defaults
}

/// Python `a or b`: `a` when it is truthy, else `b`.
fn either<'a>(a: Option<&'a Value>, b: Option<&'a Value>) -> Option<&'a Value> {
    a.filter(|v| truthy(v)).or(b)
}

/// The directions of the Theatrical presets a scene selects (its own overrides, else the project's defaults), in the order camera, performance, facial.
/// A card with its own Video Notes keeps its camera direction, and a custom facial text replaces the preset.
pub fn direction_lines(card: &Obj, defaults: Option<&Obj>) -> Vec<String> {
    let mut effective = defaults.cloned().unwrap_or_default();
    if let Some(scoped) = card.get("scene_defaults").and_then(Value::as_object) {
        effective.extend(scoped.clone());
    }
    let data = presets();
    let mut lines = Vec::new();
    let camera = either(card.get("camera_flow"), effective.get("camera_flow"));
    if camera.and_then(Value::as_str) == data.get("camera_key").and_then(Value::as_str) && !card.get("motion_summary").is_some_and(truthy) {
        let flow = &data["camera"];
        let candidates: Vec<String> = flow["sequence"].as_array().into_iter().flatten().map(|entry| format!("- {}; camera: {}", entry["shot"].as_str().unwrap_or(""), entry["camera"].as_str().unwrap_or(""))).collect();
        lines.push(format!(
            "Theatrical camera flow:\n{}\nUse the scene card's authored framing and camera path where supplied. For a continued take, follow its inherited opening frame and continuation contract. Choose suitable coverage from these candidates for the remaining planned shots:\n{}",
            flow["guidance"].as_str().unwrap_or(""),
            candidates.join("\n")
        ));
    }
    let performance = either(card.get("performance_style"), effective.get("performance_style"));
    if performance.and_then(Value::as_str) == Some("theatrical") {
        lines.push(format!("Theatrical performance direction:\n{}", text_at(&["performance", "direction"])));
    }
    let facial = either(card.get("facial_performance"), effective.get("facial_performance"));
    let custom = either(card.get("facial_performance_custom"), effective.get("facial_performance_custom"));
    if facial.and_then(Value::as_str) == Some("theatrical") && !custom.is_some_and(truthy) {
        lines.push(format!("Theatrical facial performance direction:\n{}", facial_direction()));
    }
    lines
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn obj(value: Value) -> Obj {
        value.as_object().cloned().unwrap()
    }

    #[test]
    fn nothing_is_added_unless_a_theatrical_preset_is_selected() {
        assert!(direction_lines(&obj(json!({"camera_flow": "medium_shots", "performance_style": "calm"})), None).is_empty());
    }

    #[test]
    fn the_scenes_overrides_win_over_the_project_defaults() {
        let project = obj(json!({"performance_style": "theatrical", "camera_flow": "pretty_little_malfunction"}));
        assert_eq!(direction_lines(&obj(json!({})), Some(&project)).len(), 2);
        let card = obj(json!({"scene_defaults": {"performance_style": "calm", "camera_flow": "close_ups"}}));
        assert!(direction_lines(&card, Some(&project)).is_empty());
    }

    #[test]
    fn video_notes_keep_their_camera_and_a_custom_face_replaces_the_preset() {
        let project = obj(json!({"camera_flow": "pretty_little_malfunction", "facial_performance": "theatrical"}));
        assert_eq!(direction_lines(&obj(json!({"motion_summary": "slow push in"})), Some(&project)).len(), 1);
        assert!(direction_lines(&obj(json!({"motion_summary": "x", "facial_performance_custom": "wry"})), Some(&project)).is_empty());
    }
}
