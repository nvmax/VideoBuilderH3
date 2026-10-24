//! Timed user notes used to plan the story. Port of `storyboard/timeline_notes.py`.

use serde_json::{json, Value};

use crate::pyval::{text_or_empty, to_float, truthy, Obj};

/// A finite time in seconds, never below zero. `None` for anything that is not a number.
fn time_seconds(value: Option<&Value>) -> Option<f64> {
    let number = value.and_then(to_float)?;
    number.is_finite().then(|| number.max(0.0))
}

/// `story_arc_timeline_notes`: the timeline notes with their times and the scenes each one touches, in time order.
pub fn story_arc_timeline_notes(payload: &Obj) -> Vec<Value> {
    let storyboard = payload.get("storyboard").and_then(Value::as_object).cloned().unwrap_or_default();
    let markers = payload.get("timeline_markers").or_else(|| storyboard.get("timeline_markers"));
    let scenes = payload.get("scenes").or_else(|| storyboard.get("scenes"));
    let Some(Value::Array(markers)) = markers else { return Vec::new() };
    let scenes: Vec<Value> = match scenes {
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    };
    let zero = json!(0);
    let mut notes: Vec<(f64, Value)> = Vec::new();
    for marker in markers {
        let Some(marker) = marker.as_object() else { continue };
        let note = text_or_empty(marker.get("note")).trim().to_string();
        let start = time_seconds(Some(marker.get("start").unwrap_or(&zero)));
        let (Some(start), false) = (start, note.is_empty()) else { continue };
        let end = time_seconds(marker.get("end")).filter(|e| *e > start);
        let mut scene_numbers: Vec<Value> = Vec::new();
        for (index, scene) in scenes.iter().enumerate() {
            let Some(scene) = scene.as_object() else { continue };
            let scene_start = time_seconds(Some(scene.get("timeline_start").or_else(|| scene.get("start")).unwrap_or(&zero)));
            let scene_end = time_seconds(scene.get("timeline_end").or_else(|| scene.get("end")));
            let (Some(scene_start), Some(scene_end)) = (scene_start, scene_end) else { continue };
            if scene_end <= scene_start {
                continue;
            }
            let overlaps = match end {
                None => scene_start <= start && start < scene_end,
                Some(end) => start < scene_end && end > scene_start,
            };
            if overlaps {
                scene_numbers.push(scene.get("scene_number").filter(|v| truthy(v)).cloned().unwrap_or_else(|| json!(index + 1)));
            }
        }
        let or_default = |key: &str, default: &str| match marker.get(key).filter(|v| truthy(v)) {
            Some(v) => text_or_empty(Some(v)).trim().to_string(),
            None => default.to_string(),
        };
        let (kind, label) = (or_default("type", "note"), or_default("label", "Timeline note"));
        notes.push((
            start,
            json!({
                "start": start,
                "end": end,
                "type": kind,
                "label": label,
                "note": note,
                "scene_numbers": scene_numbers,
            }),
        ));
    }
    notes.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    notes.into_iter().map(|(_, note)| note).collect()
}
