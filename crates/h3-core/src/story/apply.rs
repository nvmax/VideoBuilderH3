//! What a Story Mode plan does to the session: the performance of each scene, the masked continuations between scenes, and the characters of each
//! scene. Port of the functions of the same names in `agent_api/orchestrator/story_mode_orchestrator.py` (the twin of `story_continuation.mjs`).
//!
//! The functions change the session they are given and say which scenes they changed. Saving is the caller's job.

use std::collections::HashMap;

use serde_json::{json, Value};

use super::acts::validate_story_acts;
use super::mode::{
    apply_scene_cast, apply_story_book_text, is_story_person_card, normalize_story_book, normalize_story_mode_fields, normalize_story_scene_plan, plan_segment_fields, plan_to_value,
    resolve_story_continuations,
};
use crate::continuity::{continuity_allowed_for_mode, MASKED};
use crate::pyval::{text_or_empty, truthy, Obj};
use crate::settings::settings_for_scene;

/// Story Mode fields an agent may send to `PUT /story/settings`. Each is checked before it is saved.
pub const STORY_MODE_EDIT_KEYS: [&str; 9] =
    ["story_book_text", "story_settings", "story_acts", "story_scene_plan", "story_spot_min_scenes", "story_cast_ids", "story_allow_new_cast", "story_video_style", "story_continuation_mode"];

fn segments_of(session: &mut Obj) -> Option<&mut Vec<Value>> {
    session.get_mut("segments").and_then(Value::as_array_mut)
}

fn id_of(segment: &Value) -> String {
    text_or_empty(segment.get("id"))
}

/// Write each planned scene's performance to its timeline segment. Returns the changed scene ids.
pub fn apply_plan_to_segments(session: &mut Obj, plan: &Value) -> Vec<String> {
    let mut fields_by_scene: HashMap<String, Obj> = HashMap::new();
    for entry in plan.as_array().into_iter().flatten().filter(|e| e.is_object()) {
        let scene_id = text_or_empty(entry.get("scene_id"));
        if !scene_id.is_empty() {
            fields_by_scene.insert(scene_id, plan_segment_fields(entry.get("performance_mode")));
        }
    }
    let mut changed = Vec::new();
    for segment in segments_of(session).into_iter().flatten() {
        let Some(map) = segment.as_object_mut() else { continue };
        let id = text_or_empty(map.get("id"));
        let Some(fields) = fields_by_scene.get(&id) else { continue };
        if fields.iter().any(|(key, value)| map.get(key) != Some(value)) {
            for (key, value) in fields {
                map.insert(key.clone(), value.clone());
            }
            changed.push(id);
        }
    }
    changed
}

fn scene_masked(segment: &Value) -> bool {
    segment.get("use_scene_minimax_h3_settings").is_some_and(truthy)
        && segment.get("minimax_h3_settings").and_then(|s| s.get("continuity_mode")).and_then(Value::as_str) == Some(MASKED)
}

fn text(value: Option<&Value>) -> &str {
    value.and_then(Value::as_str).unwrap_or("")
}

/// Make each continued scene a MiniMax masked continuation, like the timeline's chain button. Returns the changed scene ids. Only MiniMax
/// projects are changed, and only scenes the story made masked are turned back off.
pub fn apply_continuations_to_segments(session: &mut Obj, plan: &Value) -> Vec<String> {
    if text_or_empty(session.get("video_engine")).trim().to_lowercase() != "minimax_h3" {
        return Vec::new();
    }
    let entries: HashMap<String, super::mode::PlanEntry> = normalize_story_scene_plan(Some(plan)).into_iter().map(|e| (e.scene_id.clone(), e)).collect();
    let project: Obj = session.get("minimax_h3_settings").and_then(Value::as_object).cloned().unwrap_or_default();
    // The settings a scene gets read the project and the scene, never the other scenes, so the project without its scenes is enough.
    let Some(Value::Array(mut segments)) = session.remove("segments") else { return Vec::new() };
    let snapshot = Value::Object(session.clone());
    let mut changed: Vec<String> = Vec::new();
    for (index, segment) in segments.iter_mut().filter(|s| s.is_object()).enumerate() {
        let id = id_of(segment);
        let entry = entries.get(&id);
        if let Some(entry) = entry.filter(|e| e.continues_previous && index > 0) {
            let effective = settings_for_scene(&snapshot, Some(segment));
            let video_mode = text(effective.get("video_mode")).to_string();
            if !continuity_allowed_for_mode(MASKED, &video_mode, text(effective.get("render_pass"))) {
                continue;
            }
            if !scene_masked(segment) {
                let own: Obj = segment.get("minimax_h3_settings").and_then(Value::as_object).cloned().unwrap_or_default();
                let locked = segment.get("use_scene_minimax_h3_settings").is_some_and(truthy);
                let mut base = if locked && !own.is_empty() { own } else { project.clone() };
                let map = segment.as_object_mut().expect("filtered to objects");
                map.insert(
                    "minimax_h3_masked_quick_prev".into(),
                    json!({
                        "locked": locked,
                        "continuity_mode": effective.get("continuity_mode").cloned().unwrap_or(Value::Null),
                        "location_transition_preset": effective.get("location_transition_preset").cloned().unwrap_or(Value::Null),
                        "continuity_prompt_from_last_frame": effective.get("continuity_prompt_from_last_frame").is_some_and(truthy),
                    }),
                );
                map.insert("use_scene_minimax_h3_settings".into(), json!(true));
                base.insert("video_mode".into(), json!(video_mode));
                base.insert("continuity_mode".into(), json!(MASKED));
                base.insert("location_transition_preset".into(), json!("masked"));
                base.insert("continuity_prompt_from_last_frame".into(), json!(true));
                map.insert("minimax_h3_settings".into(), Value::Object(base));
                map.insert("minimax_h3_mode".into(), json!(video_mode));
                map.insert("story_masked_continuation".into(), json!(true));
                changed.push(id.clone());
            }
            let map = segment.as_object_mut().expect("filtered to objects");
            if text(map.get("minimax_h3_continuation_direction")) != entry.continuation_direction {
                map.insert("minimax_h3_continuation_direction".into(), json!(entry.continuation_direction));
                if !changed.contains(&id) {
                    changed.push(id.clone());
                }
            }
            continue;
        }
        let was_story_masked = segment.as_object_mut().and_then(|m| m.remove("story_masked_continuation")).is_some_and(|v| truthy(&v));
        if !was_story_masked || !scene_masked(segment) {
            continue;
        }
        let map = segment.as_object_mut().expect("filtered to objects");
        let before = map.remove("minimax_h3_masked_quick_prev").filter(truthy).unwrap_or_else(|| json!({}));
        if before.get("locked") == Some(&json!(false)) {
            map.insert("use_scene_minimax_h3_settings".into(), json!(false));
            map.remove("minimax_h3_settings");
        } else {
            let mut settings: Obj = map.get("minimax_h3_settings").and_then(Value::as_object).cloned().unwrap_or_default();
            let or = |key: &str, default: &str| before.get(key).filter(|v| truthy(v)).cloned().unwrap_or_else(|| json!(default));
            settings.insert("continuity_mode".into(), or("continuity_mode", "off"));
            settings.insert("location_transition_preset".into(), or("location_transition_preset", "normal"));
            settings.insert("continuity_prompt_from_last_frame".into(), json!(before.get("continuity_prompt_from_last_frame").is_some_and(truthy)));
            map.insert("minimax_h3_settings".into(), Value::Object(settings));
        }
        map.insert("minimax_h3_continuation_direction".into(), json!(""));
        changed.push(id);
    }
    session.insert("segments".into(), Value::Array(segments));
    changed
}

/// Map each planned scene's cast onto it, and each act's cast onto its scenes that still have no characters (the Reference Builder's `subject_scene_map`).
pub fn fill_cast_from_acts(session: &mut Obj, acts: &Value, plan: &Value) -> Vec<String> {
    let refs = session.entry("flux_reference_builder").or_insert_with(|| json!({}));
    let Some(refs) = refs.as_object_mut() else { return Vec::new() };
    let subject_ids: Vec<String> = refs
        .get("subjects")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|s| is_story_person_card(s) && s.get("id").is_some_and(truthy))
        .map(|s| text_or_empty(s.get("id")))
        .collect();
    let current: Obj = refs.get("subject_scene_map").and_then(Value::as_object).cloned().unwrap_or_default();
    let (mapped, changed) = apply_scene_cast(&current, Some(plan), Some(acts), &subject_ids);
    if !changed.is_empty() {
        refs.insert("subject_scene_map".into(), Value::Object(mapped));
        refs.insert("use_subject_reference".into(), json!(true));
    }
    changed
}

/// The checked Story Mode fields of a `PUT /story/settings`: the book text is parsed into chapters and settings, the acts must cover the
/// timeline, and which scenes continue follows the continuation mode.
pub fn story_mode_edit_fields(session: &Obj, layer: &Obj, story_in: &Obj) -> Result<Obj, String> {
    let wanted: Obj = STORY_MODE_EDIT_KEYS.iter().filter_map(|key| story_in.get(*key).map(|v| (key.to_string(), v.clone()))).collect();
    let mut fields = normalize_story_mode_fields(&wanted);
    if story_in.contains_key("story_book_text") {
        let mut merged = layer.clone();
        merged.extend(fields.clone());
        let source = story_in.get("story_book_text").filter(|v| truthy(v)).map(|v| text_or_empty(Some(v))).unwrap_or_default();
        fields.extend(apply_story_book_text(&Value::Object(merged), &source));
    }
    let timeline: Vec<Value> = session.get("segments").and_then(Value::as_array).into_iter().flatten().filter(|s| s.is_object() && s.get("id").is_some_and(truthy)).cloned().collect();
    if let Some(acts) = fields.get("story_acts").and_then(Value::as_array).cloned() {
        let book = fields.get("story_book").filter(|v| truthy(v)).or_else(|| layer.get("story_book"));
        let chapters = normalize_story_book(book).get("chapters").and_then(Value::as_array).cloned().unwrap_or_default();
        let scene_ids: Vec<String> = timeline.iter().map(|s| text_or_empty(s.get("id"))).collect();
        let problems = validate_story_acts(&acts, &chapters, &scene_ids);
        if !problems.is_empty() {
            return Err(format!("story_acts are not valid: {}", problems.join(" ")));
        }
    }
    if fields.contains_key("story_continuation_mode") || fields.contains_key("story_scene_plan") {
        let plan = fields.get("story_scene_plan").cloned().or_else(|| layer.get("story_scene_plan").cloned()).unwrap_or_else(|| json!([]));
        if plan.as_array().is_some_and(|p| !p.is_empty()) {
            let acts = fields.get("story_acts").or_else(|| layer.get("story_acts"));
            let mode = fields.get("story_continuation_mode").or_else(|| layer.get("story_continuation_mode"));
            fields.insert("story_scene_plan".into(), plan_to_value(&resolve_story_continuations(Some(&plan), &timeline, acts, mode)));
        }
    }
    Ok(fields)
}
