//! A Storyboard scene card as it is saved in `storyboard.json`, and the script import beside it. Port of `_normalize_storyboard_scene`
//! and `_normalize_script_import` in `storyboard/persistence.py`.

use serde_json::{json, Value};

use super::helpers::{
    clean_of, first_truthy, normalize_performance_mode, normalize_reference_item, normalize_reference_items, normalize_speaker_assignments, normalize_tags, scene_number,
};
use super::mode::{text, text_s};
use super::scene_text::enforce_video_facial_requirements;
use crate::pyval::{text_or_empty, to_float, to_int, truthy, Obj};

fn lowered(value: Option<&Value>) -> String {
    text_or_empty(value).trim().to_lowercase()
}

fn string_or(map: &Obj, keys: &[&str], default: &str) -> String {
    first_truthy(map, keys).map(|v| text_or_empty(Some(v))).filter(|s| !s.is_empty()).unwrap_or_else(|| default.to_string())
}

/// `_clean_scene_text(a or b or default, limit)`.
fn clean_or(map: &Obj, keys: &[&str], default: &str, limit: usize) -> String {
    match first_truthy(map, keys) {
        Some(v) => text(Some(v), limit),
        None => text_s(default, limit),
    }
}

fn any_truthy(map: &Obj, keys: &[&str]) -> bool {
    keys.iter().any(|k| map.get(*k).is_some_and(truthy))
}

fn float_of(value: Option<&Value>) -> f64 {
    value.filter(|v| truthy(v)).and_then(to_float).unwrap_or(0.0)
}

/// `.get(key, default)`: the value of the key when it is there at all.
fn get_or<'a>(map: &'a Obj, key: &str, default: Option<&'a Value>) -> Option<&'a Value> {
    map.get(key).or(default)
}

fn extra_subjects(scene: &Obj) -> Vec<Value> {
    let raw = first_truthy(scene, &["extra_subjects", "extraSubjects"]);
    let Some(Value::Array(items)) = raw else { return Vec::new() };
    let mut extras = Vec::new();
    for (index, item) in items.iter().take(100).enumerate() {
        let Some(item) = item.as_object() else { continue };
        let number = index + 1;
        let interaction = string_or(item, &["interaction"], "background").trim().to_string();
        let interaction = if matches!(interaction.as_str(), "background" | "background_dancing" | "alongside" | "dancing_with" | "direct") { interaction } else { "background".to_string() };
        let count = item.get("count").filter(|v| truthy(v)).map_or(Some(1.0), to_float).filter(|f| f.is_finite()).map_or(1, |f| crate::pyval::round_half_even(f).clamp(1, 100));
        extras.push(json!({
            "id": clean_or(item, &["id"], &format!("extra_{number}"), 180),
            "name": clean_or(item, &["name", "title"], &format!("Extra {number}"), 180),
            "count": count,
            "interaction": interaction,
            "identity": clean_of(item, &["identity", "description"], 240),
        }));
    }
    extras
}

/// A scene card cleaned the way the pack saves it. Missing fields get their defaults.
pub fn normalize_storyboard_scene(scene: &Value, fallback_number: i64) -> Value {
    let empty = Obj::new();
    let scene = scene.as_object().unwrap_or(&empty);
    let number = scene_number(scene, fallback_number);
    let label = clean_or(scene, &["label"], &format!("Scene {number}"), 180);
    let lyrics = clean_of(scene, &["lyrics", "lyric_text", "lyricNote"], 4000);
    let image_prompt = clean_of(scene, &["image_prompt", "t2i_prompt", "prompt"], 12000);
    let mut video_prompt = clean_of(scene, &["video_prompt", "i2v_prompt", "t2v_prompt"], 100_000);
    let image_path = clean_of(scene, &["image_path", "approved_image_path", "image"], 2000);
    let image_data = string_or(scene, &["image_data", "image_reference_data"], "").trim().to_string();
    let prompt_summary = match scene.get("prompt_summary").or_else(|| scene.get("summary")) {
        Some(v) => text(Some(v), 1000),
        None => text_s(&image_prompt.chars().take(260).collect::<String>(), 1000),
    };
    let subjects = normalize_tags(first_truthy(scene, &["subjects", "singers", "mapped_subjects"]));
    let subject_refs = normalize_reference_items(scene.get("subject_refs"));
    let setting = clean_of(scene, &["setting", "location"], 500);
    let location_ref = match scene.get("location_ref") {
        Some(value) if value.is_object() => normalize_reference_item(Some(value), if setting.is_empty() { "Location" } else { &setting }, "location"),
        _ => Value::Null,
    };
    let performance_mode = normalize_performance_mode(first_truthy(scene, &["performance_mode", "performanceMode", "video_performance_mode", "videoPerformanceMode"]));
    let facial_custom = clean_of(scene, &["facial_performance_custom", "facialPerformanceCustom", "facial_expression_custom", "facialExpressionCustom"], 1200);
    let facial_direction = clean_or(scene, &["facial_performance_direction", "facialPerformanceDirection"], &facial_custom, 1600);
    let trigger_position = string_or(scene, &["trigger_position", "triggerPosition", "trigger_placement"], "start").trim().to_lowercase();
    let video_prompt_type = {
        let found = clean_of(scene, &["video_prompt_type", "video_type", "mode"], 40);
        if matches!(found.as_str(), "i2v" | "id_lora" | "t2v" | "rtv" | "ingredients" | "flf") { found } else { "i2v".to_string() }
    };
    let engine = if string_or(scene, &["project_video_engine", "projectVideoEngine"], "").trim().to_lowercase() == "minimax_h3" { "minimax_h3" } else { "ltx" };
    let h3_mode = string_or(scene, &["minimax_h3_mode", "minimaxH3Mode"], "").trim().to_lowercase().replace(['-', ' '], "_");
    let h3_mode = if matches!(h3_mode.as_str(), "text_to_video" | "image_to_video" | "reference_to_video" | "video_to_video") { h3_mode } else { "text_to_video".to_string() };
    let audio_mode = string_or(scene, &["minimax_h3_audio_mode", "minimaxH3AudioMode"], "input_audio").trim().to_lowercase().replace(['-', ' '], "_");
    let audio_mode = if matches!(audio_mode.as_str(), "built_in_audio" | "native_audio" | "generated_audio") { "built_in_audio" } else { "input_audio" };
    // One unusable number makes all three zero.
    let seconds = |value: Option<&Value>| value.filter(|v| truthy(v)).map_or(Some(0.0), to_float);
    let (timeline_start, timeline_end, exact_duration) = match (
        seconds(get_or(scene, "timeline_start", scene.get("start"))),
        seconds(get_or(scene, "timeline_end", scene.get("end"))),
        seconds(get_or(scene, "exact_duration", scene.get("duration"))),
    ) {
        (Some(start), Some(end), Some(duration)) => (start, end, duration.max(0.0)),
        _ => (0.0, 0.0, 0.0),
    };
    let mut extras = extra_subjects(scene);
    if any_truthy(scene, &["no_character_present", "noCharacterPresent", "no_visible_subject", "no_subject"]) {
        extras = Vec::new();
    }
    if !video_prompt.is_empty() && engine != "minimax_h3" {
        let mut context = scene.clone();
        context.insert("subjects".into(), json!(subjects));
        context.insert("subject_refs".into(), Value::Array(subject_refs.clone()));
        context.insert("lyrics".into(), json!(lyrics));
        context.insert("performance_mode".into(), json!(performance_mode));
        video_prompt = enforce_video_facial_requirements(&video_prompt, &Value::Object(context));
    }
    let status = clean_or(scene, &["status"], if !image_path.is_empty() || !image_data.is_empty() { "image_ready" } else { "draft" }, 80);
    let id = clean_or(scene, &["id"], &format!("storyboard_scene_{number}"), 160);
    let mut card = Obj::new();
    let mut put = |key: &str, value: Value| {
        card.insert(key.into(), value);
    };
    put("id", json!(id));
    put("scene_number", json!(number));
    put("label", json!(label));
    put("lyrics", json!(lyrics));
    put("lyric_section", json!(clean_of(scene, &["lyric_section", "section", "song_section"], 160)));
    put("story_beat", json!(clean_of(scene, &["story_beat", "scene_story_beat", "narrative_beat"], 1800)));
    for key in ["flf_start_state", "flf_transformation", "flf_end_state", "flf_carry_forward"] {
        put(key, json!(clean_of(scene, &[key], 4000)));
    }
    put("performance_mode", json!(performance_mode));
    put("lyric_singers", json!(normalize_tags(scene.get("lyric_singers"))));
    put("lyric_no_lip_sync", json!(scene.get("lyric_no_lip_sync").is_some_and(truthy)));
    put("lyric_instrumental", json!(scene.get("lyric_instrumental").is_some_and(truthy)));
    put("no_character_present", json!(scene.get("no_character_present").is_some_and(truthy)));
    put("lyric_cue_map", match scene.get("lyric_cue_map") { Some(v @ Value::Array(_)) => v.clone(), _ => json!([]) });
    put("lyric_shot_word_timing_enabled", json!(scene.get("lyric_shot_word_timing_enabled").is_some_and(truthy)));
    put("lyric_performance_mode", json!(clean_of(scene, &["lyric_performance_mode"], 40)));
    put("timed_lyric_cue_contract", json!(clean_of(scene, &["timed_lyric_cue_contract"], 12000)));
    put("prompt_summary", json!(prompt_summary));
    put("motion_summary", json!(clean_of(scene, &["motion_summary", "video_notes", "i2v_notes"], 3000)));
    put("subjects", json!(subjects));
    put("subject_refs", Value::Array(subject_refs));
    put("extra_subjects", Value::Array(extras));
    put("speaker_assignments", Value::Array(normalize_speaker_assignments(first_truthy(scene, &["speaker_assignments", "minimax_speaker_assignments", "dialogue_cues"]))));
    put("setting", json!(setting));
    put("location_ref", location_ref);
    put("shot_type", json!(clean_of(scene, &["shot_type", "shot"], 200)));
    put("camera_motion", json!(clean_of(scene, &["camera_motion", "motion_preset"], 200)));
    put("character_motion", json!(clean_of(scene, &["character_motion", "character_motion_preset", "subject_motion"], 240)));
    put("performance_style", json!(clean_of(scene, &["performance_style", "song_style", "music_style"], 120)));
    put("performance_direction", json!(clean_of(scene, &["performance_direction"], 1000)));
    put("facial_performance", json!(clean_of(scene, &["facial_performance", "facialPerformance", "facial_expression", "facialExpression"], 120)));
    put("facial_performance_custom", json!(facial_custom));
    put("emotion_expression_tags", json!(text(scene.get("emotion_expression_tags"), 1200)));
    put("facial_performance_direction", json!(facial_direction));
    put("include_microphone", json!(any_truthy(scene, &["include_microphone", "use_microphone", "microphone"])));
    put("trigger_phrase", json!(clean_of(scene, &["trigger_phrase", "trigger", "Trigger"], 1200)));
    put("trigger_position", json!(if trigger_position == "end" { "end" } else { "start" }));
    put("video_prompt_type", json!(video_prompt_type));
    put("project_video_engine", json!(engine));
    put("minimax_h3_mode", json!(h3_mode));
    put("minimax_h3_audio_mode", json!(audio_mode));
    put("video_style", json!(clean_of(scene, &["video_style", "videoStyle"], 160)));
    put("video_style_custom", json!(clean_of(scene, &["video_style_custom", "videoStyleCustom"], 3000)));
    put("temporal_world_effect_override", json!(text_s(&string_or(scene, &["temporal_world_effect_override", "temporalWorldEffectOverride"], "global"), 120)));
    put("temporal_world_effect_custom", json!(clean_of(scene, &["temporal_world_effect_custom", "temporalWorldEffectCustom"], 3000)));
    put("timeline_start", json!(timeline_start));
    put("timeline_end", json!(timeline_end));
    put("exact_duration", json!(exact_duration));
    put("video_prompt_origin", json!(if lowered(first_truthy(scene, &["video_prompt_origin", "i2v_prompt_origin"])) == "gemma" { "gemma" } else { "manual" }));
    put("minimax_h3_pass2_prompt", json!(clean_of(scene, &["minimax_h3_pass2_prompt", "pass2_prompt"], 100_000)));
    put("status", json!(status));
    put("image_prompt", json!(image_prompt));
    put("video_prompt", json!(video_prompt));
    put("image_path", json!(image_path));
    put("image_data", json!(image_data));
    put("image_name", json!(clean_of(scene, &["image_name", "image_reference_name"], 260)));
    put("notes", json!(clean_of(scene, &["notes"], 4000)));
    put("timeline_note", json!(clean_of(scene, &["timeline_note"], 4000)));
    put("audio_direction", json!(clean_of(scene, &["audio_direction", "audioDirection"], 4000)));
    put("continuity", json!(clean_of(scene, &["continuity", "continuity_direction", "continuityDirection"], 4000)));
    put("id_lora_character_id", json!(clean_of(scene, &["id_lora_character_id", "character_id", "subject_id"], 180)));
    put("id_lora_location_id", json!(clean_of(scene, &["id_lora_location_id", "location_id"], 180)));
    Value::Object(card)
}

fn items(value: Option<&Value>) -> Vec<Value> {
    match value {
        Some(Value::Array(list)) => list.clone(),
        _ => Vec::new(),
    }
}

fn int_or(value: Option<&Value>, default: i64) -> i64 {
    match value.filter(|v| truthy(v)) {
        Some(v) => to_int(v).or_else(|| to_float(v).map(|f| f.trunc() as i64)).unwrap_or(default),
        None => default,
    }
}

/// `_normalize_short_film_planning_mode`.
pub fn normalize_short_film_planning_mode(value: Option<&Value>) -> String {
    let clean = lowered(value).replace(['-', ' '], "_");
    if matches!(clean.as_str(), "fully_custom" | "custom") { "fully_custom".into() } else { "guided_film".into() }
}

/// `_normalize_script_import`: the pasted script, its cues, speaker matches and the planned scenes.
pub fn normalize_script_import(value: Option<&Value>) -> Value {
    let source = value.and_then(Value::as_object).cloned().unwrap_or_default();
    let mut cues = Vec::new();
    for (index, item) in items(source.get("cues")).iter().take(1000).enumerate() {
        let Some(item) = item.as_object() else { continue };
        let speaker_alias = clean_of(item, &["speaker_alias", "speaker", "speaker_name"], 240);
        let line = clean_of(item, &["text", "dialogue", "line"], 4000);
        if speaker_alias.is_empty() || line.is_empty() {
            continue;
        }
        let speaker_name = clean_or(item, &["speaker_name", "reference_subject_name"], &speaker_alias, 240);
        let method = clean_or(item, &["speaker_match_method"], "manual", 40);
        cues.push(json!({
            "index": int_or(item.get("index"), index as i64 + 1),
            "line_number": int_or(item.get("line_number"), 0),
            "scene_index": int_or(item.get("scene_index"), 0),
            "scene_label": text(item.get("scene_label"), 240),
            "speaker": speaker_alias,
            "speaker_alias": speaker_alias,
            "speaker_id": clean_of(item, &["speaker_id", "reference_subject_id"], 180),
            "speaker_name": speaker_name,
            "reference_subject_id": clean_of(item, &["reference_subject_id", "speaker_id"], 180),
            "reference_subject_name": clean_of(item, &["reference_subject_name", "speaker_name"], 240),
            "speaker_match_method": method,
            "text": line,
            "word_count": item.get("word_count").filter(|v| truthy(v)).map_or(line.split_whitespace().count() as i64, |v| to_int(v).unwrap_or(0)),
        }));
    }
    let mut speaker_matches = Vec::new();
    for item in items(source.get("speaker_matches")).iter().take(180) {
        let Some(item) = item.as_object() else { continue };
        let alias = clean_of(item, &["speaker_alias", "speaker"], 240);
        if alias.is_empty() {
            continue;
        }
        let method = clean_or(item, &["match_method"], "manual", 40);
        speaker_matches.push(json!({
            "speaker_alias": alias,
            "reference_subject_id": clean_of(item, &["reference_subject_id", "speaker_id"], 180),
            "reference_subject_name": clean_of(item, &["reference_subject_name", "speaker_name"], 240),
            "match_method": method,
        }));
    }
    let maximum = first_truthy(&source, &["maximum_scene_seconds", "max_scene_seconds"]).and_then(to_float).filter(|f| f.is_finite()).unwrap_or(8.0).clamp(3.0, 15.0);
    let plan_source = source.get("scene_plan").and_then(Value::as_object).cloned().unwrap_or_default();
    let mut planned = Vec::new();
    for (scene_index, scene) in items(plan_source.get("scenes")).iter().take(240).enumerate() {
        let Some(scene) = scene.as_object() else { continue };
        let mut assignments = Vec::new();
        for cue in items(scene.get("speaker_assignments")).iter().take(80) {
            let Some(cue) = cue.as_object() else { continue };
            let dialogue = clean_of(cue, &["text", "dialogue"], 4000);
            if dialogue.is_empty() {
                continue;
            }
            let name = clean_or(cue, &["speaker_name", "speaker_alias"], "Speaker", 240);
            let alias = clean_or(cue, &["speaker_alias", "speaker_name"], "Speaker", 240);
            assignments.push(json!({
                "speaker_id": clean_of(cue, &["speaker_id", "reference_subject_id"], 180),
                "speaker_name": name,
                "speaker_alias": alias,
                "text": dialogue,
                "source_cue_index": int_or(cue.get("source_cue_index"), 0),
                "part_index": int_or(cue.get("part_index"), 1),
                "part_count": int_or(cue.get("part_count"), 1),
                "planned_start_seconds": float_of(cue.get("planned_start_seconds")),
                "planned_end_seconds": float_of(cue.get("planned_end_seconds")),
                "estimated_spoken_seconds": float_of(cue.get("estimated_spoken_seconds")),
            }));
        }
        if assignments.is_empty() {
            continue;
        }
        let label = clean_or(scene, &["label"], &format!("Script Segment {}", scene_index + 1), 240);
        let ids = |key: &str, limit: usize| -> Vec<String> {
            items(scene.get(key)).iter().map(|i| text(Some(i), limit)).filter(|s| !s.is_empty()).collect()
        };
        planned.push(json!({
            "index": int_or(scene.get("index"), scene_index as i64 + 1),
            "label": label,
            "source_scene_index": int_or(scene.get("source_scene_index"), 0),
            "source_scene_label": text(scene.get("source_scene_label"), 240),
            "continuation_of_previous": scene.get("continuation_of_previous").is_some_and(truthy),
            "duration_seconds": float_of(scene.get("duration_seconds")),
            "timeline_start_seconds": float_of(scene.get("timeline_start_seconds")),
            "timeline_end_seconds": float_of(scene.get("timeline_end_seconds")),
            "participant_ids": ids("participant_ids", 180),
            "participant_names": ids("participant_names", 240),
            "speaker_assignments": assignments,
        }));
    }
    let enabled = source.get("enabled").is_none_or(truthy) && !cues.is_empty();
    let unmatched: Vec<String> = items(source.get("unmatched_speakers")).iter().map(|i| text(Some(i), 240)).filter(|s| !s.is_empty()).collect();
    let format_text = clean_or(&source, &["format"], "text", 40);
    json!({
        "enabled": enabled,
        "authoritative": source.get("authoritative").is_none_or(truthy),
        "format": format_text,
        "raw_text": clean_of(&source, &["raw_text", "rawText"], 100_000),
        "imported_at": clean_of(&source, &["imported_at", "importedAt"], 80),
        "maximum_scene_seconds": maximum,
        "cues": cues,
        "speaker_matches": speaker_matches,
        "unmatched_speakers": unmatched,
        "scene_plan": {
            "maximum_scene_seconds": maximum,
            "scene_count": planned.len(),
            "estimated_total_seconds": float_of(plan_source.get("estimated_total_seconds")),
            "split_cue_count": int_or(plan_source.get("split_cue_count"), 0),
            "scenes": planned,
        },
    })
}
