//! Editable scene card fields and the segment keys each one is saved under. Port of `storyboard/scene_card_fields.py`
//! without the Storyboard card side: this app saves the timeline scene only.

use serde_json::{json, Value};

use crate::pyval::{py_str, to_float, Obj};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text,
    ImagePrompt,
    VideoPrompt,
    Reference,
    Choice,
    Bool,
    Strings,
    References,
    Objects,
    Speakers,
}

#[derive(Debug, Clone, Copy)]
pub struct Field {
    pub name: &'static str,
    pub kind: Kind,
    pub segment_keys: &'static [&'static str],
    pub card_key: Option<&'static str>,
    pub aliases: &'static [&'static str],
    pub choices: &'static [&'static str],
    pub limit: usize,
}

const PERFORMANCE_MODES: &[&str] = &["singing", "speaking", "no_lip_sync"];
const VIDEO_PROMPT_TYPES: &[&str] = &["i2v", "id_lora", "t2v", "rtv", "ingredients", "flf"];
const VIDEO_PROMPT_ORIGINS: &[&str] = &["gemma", "manual"];
const TRIGGER_POSITIONS: &[&str] = &["start", "end"];

const fn text(name: &'static str, segment_keys: &'static [&'static str], card_key: Option<&'static str>, aliases: &'static [&'static str], limit: usize) -> Field {
    Field { name, kind: Kind::Text, segment_keys, card_key, aliases, choices: &[], limit }
}

const fn flag(name: &'static str, keys: &'static [&'static str]) -> Field {
    Field { name, kind: Kind::Bool, segment_keys: keys, card_key: Some(name), aliases: &[], choices: &[], limit: 20000 }
}

const fn other(name: &'static str, kind: Kind, segment_keys: &'static [&'static str], card_key: Option<&'static str>, aliases: &'static [&'static str], choices: &'static [&'static str], limit: usize) -> Field {
    Field { name, kind, segment_keys, card_key, aliases, choices, limit }
}

pub const FIELDS: &[Field] = &[
    text("label", &["label"], Some("label"), &[], 180),
    text("lyric_text", &["lyric_text"], Some("lyrics"), &["lyrics"], 4000),
    text("lyric_section", &["lyric_section"], Some("lyric_section"), &[], 160),
    text("story_beat", &["story_beat"], Some("story_beat"), &[], 1800),
    text("timeline_note", &["timeline_note"], Some("timeline_note"), &["director_note", "director_notes"], 4000),
    text("i2v_notes", &["i2v_notes", "video_notes"], Some("motion_summary"), &["video_notes", "motion_summary"], 3000),
    text("notes", &["notes"], Some("notes"), &["planning_notes"], 4000),
    other("prompt_summary", Kind::Text, &[], Some("prompt_summary"), &["summary"], &[], 1000),
    text("shot_type", &["shot_type"], Some("shot_type"), &[], 200),
    text("camera_motion", &["camera_motion"], Some("camera_motion"), &[], 200),
    text("character_motion", &["character_motion"], Some("character_motion"), &[], 240),
    other("performance_mode", Kind::Choice, &["performance_mode"], Some("performance_mode"), &[], PERFORMANCE_MODES, 20000),
    text("performance_style", &["performance_style"], Some("performance_style"), &[], 120),
    text("facial_performance", &["facial_performance"], Some("facial_performance"), &[], 120),
    text("facial_performance_custom", &["facial_performance_custom"], Some("facial_performance_custom"), &[], 1200),
    text("emotion_expression_tags", &["emotion_expression_tags"], Some("emotion_expression_tags"), &[], 1200),
    flag("include_microphone", &["include_microphone"]),
    text("audio_direction", &["audio_direction"], Some("audio_direction"), &[], 4000),
    text("continuity", &["continuity"], Some("continuity"), &["continuity_direction"], 4000),
    text("flf_start_state", &["flf_start_state"], Some("flf_start_state"), &[], 4000),
    text("flf_transformation", &["flf_transformation"], Some("flf_transformation"), &[], 4000),
    text("flf_end_state", &["flf_end_state"], Some("flf_end_state"), &[], 4000),
    text("flf_carry_forward", &["flf_carry_forward"], Some("flf_carry_forward"), &[], 4000),
    text("video_style", &["minimax_h3_video_style"], Some("video_style"), &[], 160),
    text("video_style_custom", &["minimax_h3_video_style_custom"], Some("video_style_custom"), &[], 3000),
    other("temporal_world_effect_override", Kind::Text, &["temporal_world_effect_override"], Some("temporal_world_effect_override"), &[], &[], 120),
    text("temporal_world_effect_custom", &["temporal_world_effect_custom"], Some("temporal_world_effect_custom"), &[], 3000),
    other("trigger_phrase", Kind::Text, &[], Some("trigger_phrase"), &[], &[], 1200),
    other("trigger_position", Kind::Choice, &[], Some("trigger_position"), &[], TRIGGER_POSITIONS, 20000),
    flag("no_character_present", &["no_character_present"]),
    flag("lyric_no_lip_sync", &["lyric_no_lip_sync"]),
    flag("lyric_instrumental", &["lyric_instrumental"]),
    other("lyric_singers", Kind::Strings, &["lyric_singers"], Some("lyric_singers"), &[], &[], 20000),
    other("lyric_cue_map", Kind::Objects, &["lyric_cue_map"], Some("lyric_cue_map"), &[], &[], 20000),
    flag("lyric_shot_word_timing_enabled", &["lyric_shot_word_timing_enabled"]),
    text("lyric_performance_mode", &["lyric_performance_mode"], Some("lyric_performance_mode"), &[], 40),
    other("speaker_assignments", Kind::Speakers, &["minimax_speaker_assignments"], Some("speaker_assignments"), &["minimax_speaker_assignments"], &[], 20000),
    other("video_prompt_type", Kind::Choice, &["video_prompt_type"], Some("video_prompt_type"), &[], VIDEO_PROMPT_TYPES, 20000),
    other("image_prompt", Kind::ImagePrompt, &["t2i_prompt"], Some("image_prompt"), &[], &[], 12000),
    other("video_prompt", Kind::VideoPrompt, &[], Some("video_prompt"), &[], &[], 100000),
    other("video_prompt_origin", Kind::Choice, &[], Some("video_prompt_origin"), &[], VIDEO_PROMPT_ORIGINS, 20000),
    text("minimax_h3_pass2_prompt", &["minimax_h3_pass2_prompt"], Some("minimax_h3_pass2_prompt"), &[], 100000),
    other("subject_ids", Kind::References, &[], Some("subject_refs"), &[], &[], 20000),
    other("location_id", Kind::Reference, &[], Some("location_ref"), &[], &[], 20000),
];

/// The field a request key names (its own name or an alias).
pub fn canonical(key: &str) -> Option<&'static Field> {
    FIELDS.iter().find(|f| f.name == key).or_else(|| FIELDS.iter().find(|f| f.aliases.contains(&key)))
}

/// Every accepted name and alias, sorted.
pub fn field_names() -> Vec<String> {
    let mut names: Vec<String> = FIELDS.iter().flat_map(|f| std::iter::once(f.name).chain(f.aliases.iter().copied())).map(str::to_string).collect();
    names.sort();
    names.dedup();
    names
}

fn clean_text(field: &Field, value: &Value) -> Result<String, String> {
    let raw = match value {
        Value::Null => return Ok(String::new()),
        Value::Bool(_) | Value::Array(_) | Value::Object(_) => return Err(format!("{} must be text.", field.name)),
        other => py_str(Some(other)),
    };
    let text = raw.replace("\r\n", "\n").replace('\r', "\n").trim().to_string();
    if text.chars().count() > field.limit {
        return Err(format!("{} is longer than {} characters.", field.name, field.limit));
    }
    Ok(text)
}

fn seconds(value: Option<&Value>) -> Value {
    match value.and_then(to_float).filter(|n| n.is_finite()) {
        Some(n) => json!(n.max(0.0)),
        None => Value::Null,
    }
}

fn text_of(item: &Value, keys: &[&str]) -> String {
    keys.iter().filter_map(|k| item.get(*k)).find(|v| crate::pyval::truthy(v)).map(|v| py_str(Some(v))).unwrap_or_default()
}

fn speaker_assignments(value: &Value) -> Result<Value, String> {
    let list = value.as_array().ok_or("speaker_assignments must be a list of {speaker_id, speaker_name, text} objects.")?;
    let mut cues = Vec::new();
    for (index, item) in list.iter().take(40).enumerate() {
        if !item.is_object() {
            return Err("Each speaker assignment must be an object.".into());
        }
        let cue_type = text_of(item, &["type", "kind"]).trim().to_lowercase();
        let id = text_of(item, &["id", "cue_id", "cueId"]);
        cues.push(json!({
            "id": if id.is_empty() { format!("speaker_cue_{}", index + 1) } else { id },
            "type": if cue_type == "instrumental" { "instrumental" } else { "dialogue" },
            "speaker_id": text_of(item, &["speaker_id", "speakerId", "subject_id"]),
            "speaker_name": text_of(item, &["speaker_name", "speakerName", "speaker"]).trim(),
            "text": text_of(item, &["text", "dialogue", "line"]).trim(),
            "action_note": text_of(item, &["action_note", "actionNote"]).trim(),
            "start": seconds(item.get("start")),
            "end": seconds(item.get("end")),
        }));
    }
    Ok(Value::Array(cues))
}

/// The value saved for `field`. Empty strings, `false` and empty lists are kept as given.
pub fn normalize_value(field: &Field, value: &Value) -> Result<Value, String> {
    match field.kind {
        Kind::Text | Kind::ImagePrompt | Kind::VideoPrompt | Kind::Reference => {
            let text = clean_text(field, value)?;
            if field.name == "temporal_world_effect_override" && text.is_empty() {
                return Ok(json!("global"));
            }
            Ok(json!(text))
        }
        Kind::Choice => {
            let text = if value.is_null() { String::new() } else { py_str(Some(value)) };
            let text = text.trim().to_lowercase().replace(['-', ' '], "_");
            if field.choices.contains(&text.as_str()) {
                Ok(json!(text))
            } else {
                Err(format!("{} must be one of: {}.", field.name, field.choices.join(", ")))
            }
        }
        Kind::Bool => match value {
            Value::Bool(_) => Ok(value.clone()),
            _ => Err(format!("{} must be true or false.", field.name)),
        },
        Kind::Strings | Kind::References => {
            let items: Vec<Value> = match value {
                Value::String(_) => vec![value.clone()],
                Value::Null => vec![],
                Value::Array(a) => a.clone(),
                _ => return Err(format!("{} must be a list of names or ids.", field.name)),
            };
            let mut cleaned: Vec<String> = Vec::new();
            for item in &items {
                let text = match item {
                    Value::String(s) => s.trim().to_string(),
                    Value::Number(n) if !n.is_f64() => n.to_string(),
                    _ => return Err(format!("{} must be a list of names or ids.", field.name)),
                };
                if !text.is_empty() && !cleaned.contains(&text) {
                    cleaned.push(text);
                }
            }
            Ok(json!(cleaned))
        }
        Kind::Objects => match value.as_array() {
            Some(items) if items.iter().all(Value::is_object) => Ok(value.clone()),
            _ => Err(format!("{} must be a list of objects.", field.name)),
        },
        Kind::Speakers => speaker_assignments(value),
    }
}

/// Split a patch into normalized card values (in the order of the request) and the keys this table does not know.
pub fn parse_scene_card_patch(patch: &Obj) -> Result<(Obj, Obj), String> {
    let mut values = Obj::new();
    let mut sources: Vec<(String, String)> = Vec::new();
    let mut rest = Obj::new();
    for (key, raw) in patch {
        let Some(field) = canonical(key) else {
            rest.insert(key.clone(), raw.clone());
            continue;
        };
        let value = normalize_value(field, raw)?;
        if let Some(existing) = values.get(field.name) {
            if existing != &value {
                let first = sources.iter().find(|(n, _)| n == field.name).map(|(_, s)| s.clone()).unwrap_or_default();
                return Err(format!("{first} and {key} set the same field to different values."));
            }
        }
        values.insert(field.name.to_string(), value);
        if !sources.iter().any(|(n, _)| n == field.name) {
            sources.push((field.name.to_string(), key.clone()));
        }
    }
    Ok((values, rest))
}

/// Segment keys the project's image model writes for an edited still prompt.
pub fn image_prompt_keys(image_mode: &str) -> &'static [&'static str] {
    match image_mode.trim().to_lowercase().as_str() {
        "flow_gpt" => &["t2i_prompt", "flow_gpt_prompt", "nb_prompt"],
        "nano_banana" => &["t2i_prompt", "nb_prompt"],
        "flux_klein" => &["t2i_prompt", "flux_prompt"],
        _ => &["t2i_prompt", "flux_prompt", "nb_prompt", "flow_gpt_prompt"],
    }
}

/// The scene's video prompt key and its origin key for the project's video engine.
pub fn video_prompt_keys(video_engine: &str) -> (&'static str, &'static str) {
    if video_engine.trim().to_lowercase() == "minimax_h3" { ("minimax_h3_prompt", "minimax_h3_prompt_origin") } else { ("i2v_prompt", "i2v_prompt_origin") }
}

/// Write card values onto a timeline scene with the keys the Builder uses.
pub fn apply_to_segment(segment: &mut Value, values: &Obj, video_engine: &str, image_mode: &str) {
    let Some(map) = segment.as_object_mut() else { return };
    for (name, value) in values {
        let Some(field) = canonical(name) else { continue };
        match field.kind {
            Kind::ImagePrompt => {
                for key in image_prompt_keys(image_mode) {
                    map.insert((*key).into(), value.clone());
                }
            }
            Kind::VideoPrompt => {
                let (prompt_key, origin_key) = video_prompt_keys(video_engine);
                map.insert(prompt_key.into(), value.clone());
                map.insert(origin_key.into(), values.get("video_prompt_origin").cloned().unwrap_or_else(|| json!("manual")));
            }
            _ if field.name == "video_prompt_origin" => {
                if !values.contains_key("video_prompt") {
                    map.insert(video_prompt_keys(video_engine).1.into(), value.clone());
                }
            }
            Kind::Speakers => {
                map.insert("minimax_speaker_assignments".into(), value.clone());
                let filled: Vec<&Value> = value.as_array().map(|a| a.iter().filter(|c| c["text"].as_str().is_some_and(|t| !t.is_empty())).collect()).unwrap_or_default();
                if !filled.is_empty() && !values.contains_key("lyric_text") {
                    map.insert("lyric_text".into(), json!(filled.iter().map(|c| c["text"].as_str().unwrap_or("")).collect::<Vec<_>>().join("\n")));
                }
                if !filled.is_empty() && !values.contains_key("lyric_singers") {
                    let mut names: Vec<String> = Vec::new();
                    for cue in &filled {
                        let n = cue["speaker_name"].as_str().unwrap_or("");
                        if !n.is_empty() && !names.iter().any(|x| x == n) {
                            names.push(n.to_string());
                        }
                    }
                    map.insert("lyric_singers".into(), json!(names));
                }
            }
            _ => {
                for key in field.segment_keys {
                    map.insert((*key).into(), value.clone());
                }
            }
        }
    }
}

/// Fields in `values` that only a Storyboard card stores. This app has no Storyboard card, so they are refused.
pub fn card_only_names(values: &Obj) -> Vec<String> {
    values
        .keys()
        .filter(|name| canonical(name).is_some_and(|f| f.segment_keys.is_empty() && !matches!(f.kind, Kind::VideoPrompt | Kind::References | Kind::Reference) && f.name != "video_prompt_origin"))
        .cloned()
        .collect()
}
