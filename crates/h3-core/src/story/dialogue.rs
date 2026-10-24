//! Scene cards from the MiniMax short-film plan: the model's plan checked against the Reference Builder's characters and locations, a still-image
//! prompt for every scene, and a camera plan that does not push in on every shot. Port of the MiniMax half of `storyboard/dialogue_scenes.py`
//! and `_id_lora_structured_image_prompt`.

use fancy_regex::Regex;
use serde_json::{json, Value};

use super::mode::text;
use super::scene::normalize_storyboard_scene;
use super::scene_text::camera_motion_for_speed;
use crate::pyval::{text_or_empty, truthy, Obj};

fn rx(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid pattern")
}

fn is_match(pattern: &str, value: &str) -> bool {
    rx(pattern).is_match(value).unwrap_or(false)
}

/// `_clean_scene_text(a or b or ... or default, limit)`.
fn clean_first(map: &Obj, keys: &[&str], default: &str, limit: usize) -> String {
    match keys.iter().filter_map(|k| map.get(*k)).find(|v| truthy(v)) {
        Some(value) => text(Some(value), limit),
        None => super::mode::text_s(default, limit),
    }
}

/// The first of the candidates that is not falsy, as cleaned text (`_clean_scene_text(a or b or default)`).
fn clean_of(candidates: &[Option<&Value>], default: &str, limit: usize) -> String {
    match candidates.iter().flatten().find(|v| truthy(v)) {
        Some(value) => text(Some(value), limit),
        None => super::mode::text_s(default, limit),
    }
}

fn has_image(card: &Obj) -> bool {
    card.get("image").and_then(Value::as_object).is_some_and(|image| image.get("path").is_some_and(truthy) || image.get("name").is_some_and(truthy))
}

/// `_id_lora_structured_image_prompt`: the model's own image prompt when it is rich enough, else a still-frame prompt built from the scene.
pub fn structured_image_prompt(item: &Obj, subject_ref: Option<&Value>, location_ref: Option<&Value>) -> String {
    let raw_prompt = clean_first(item, &["image_prompt", "visual_prompt"], "", 3000);
    let words = rx(r"[A-Za-z0-9']+").find_iter(&raw_prompt).count();
    let keywords = r"(?i)\b(close-up|medium close-up|upper body|waist-up|portrait|profile|over-the-shoulder|low-angle|lens|lighting|depth of field|bokeh|palette|texture|cinematic)\b";
    if words >= 45 && is_match(keywords, &raw_prompt) {
        return raw_prompt;
    }
    let subject = subject_ref.and_then(Value::as_object).cloned().unwrap_or_default();
    let location = location_ref.and_then(Value::as_object).cloned().unwrap_or_default();
    let subject_name = clean_of(&[item.get("character_name"), item.get("speaker"), subject.get("name")], "the speaking character", 160);
    let subject_description = clean_of(&[subject.get("description"), item.get("character_description")], "", 900);
    let location_name = clean_of(&[item.get("setting"), item.get("location_name"), location.get("name")], "the scene location", 160);
    let location_description = clean_of(&[location.get("description"), item.get("location_description")], "", 900);
    let shot_type = clean_first(item, &["shot_type"], "cinematic medium shot", 120);
    let visual_direction = clean_first(item, &["visual_direction", "summary", "story_beat", "beat"], "", 1000);
    let facial = clean_first(item, &["facial_performance_custom", "facial_performance", "emotion", "delivery"], "", 500);
    let opening = match (has_image(&subject), has_image(&location)) {
        (true, true) => "Using the provided character reference and location reference, create",
        (true, false) => "Using the provided character reference, create",
        (false, true) => "Using the provided location reference, create",
        (false, false) => "Create",
    };
    let subject_clause = if subject_description.is_empty() { subject_name } else { format!("{subject_name}, preserving {subject_description}") };
    let location_clause = if location_description.is_empty() { format!("in {location_name}") } else { format!("in {location_name}, with {location_description}") };
    let action = if visual_direction.is_empty() { "a tense dialogue-first short-film moment".to_string() } else { visual_direction };
    let face = if facial.is_empty() { String::new() } else { format!(" Give the face/body language {facial}.") };
    let prompt = format!(
        "{opening} a {shot_type} of {subject_clause} {location_clause}. Stage the still frame around {action}.{face} Use a new pose and camera angle, shallow depth of field, practical cinematic lighting, textured materials, atmospheric haze or background separation, a deliberate color palette, crisp facial detail, and high cinematic image quality. No captions, no text overlays, no dialogue printed in the image."
    );
    super::mode::text_s(&rx(r"\s+").replace_all(&prompt, " "), 3000)
}

/// `_minimax_camera_motion_family`: what kind of camera move a direction describes.
pub fn camera_motion_family(value: &str) -> &'static str {
    let motion = super::mode::text_s(value, 500).to_lowercase();
    if is_match(r"\b(push(?:es)?[ -]?in|doll(?:y|ies)[ -]?in|zoom(?:s)?[ -]?in|track(?:s|ing)?[ -]?(?:in|forward)|drift(?:s|ing)?[ -]?(?:closer|forward))\b", &motion) {
        "inward"
    } else if is_match(r"\b(pull(?:s)?[ -]?(?:back|out)|doll(?:y|ies)[ -]?out|zoom(?:s)?[ -]?out|track(?:s|ing)?[ -]?backward)\b", &motion) {
        "outward"
    } else if is_match(r"\b(orbit|arc|circle|rotate|rotation)\b", &motion) {
        "orbit"
    } else if is_match(r"\b(pan|lateral|side|truck)\b", &motion) {
        "lateral"
    } else if is_match(r"\b(rack focus|focus pull)\b", &motion) {
        "focus"
    } else if is_match(r"\b(hold|locked|static)\b", &motion) {
        "hold"
    } else if motion.is_empty() {
        ""
    } else {
        "other"
    }
}

const NON_INWARD_CAMERA_SEQUENCE: [&str; 8] = [
    "quiet handheld hold",
    "subtle lateral drift",
    "slow orbit left",
    "gentle pull-back",
    "restrained pan right",
    "rack focus between the speakers",
    "slow orbit right",
    "locked-off reaction hold",
];

/// `_rebalance_generated_minimax_camera_motion`: stop an LLM-planned sequence from collapsing into repeated push-ins. An inward move stays an accent,
/// at most once in a rolling six scenes.
pub fn rebalance_camera_motion(scenes: &mut [Value], camera_flow: &str, camera_motion_speed: i64) {
    if camera_flow.trim().to_lowercase() == "off" {
        return;
    }
    let speed = camera_motion_speed.clamp(0, 10);
    let mut recent: Vec<&str> = Vec::new();
    for (index, scene) in scenes.iter_mut().enumerate() {
        let Some(card) = scene.as_object_mut() else { continue };
        let motion = camera_motion_for_speed(&text_or_empty(card.get("camera_motion")), speed as f64);
        if !motion.is_empty() {
            card.insert("camera_motion".into(), json!(motion));
        }
        let mut family = camera_motion_family(&motion);
        let replacement = if speed <= 0 { "locked-off camera" } else { NON_INWARD_CAMERA_SEQUENCE[index % NON_INWARD_CAMERA_SEQUENCE.len()] };
        let window_start = recent.len().saturating_sub(5);
        if motion.is_empty() || (family == "inward" && recent[window_start..].contains(&"inward")) {
            card.insert("camera_motion".into(), json!(replacement));
            family = camera_motion_family(replacement);
        }
        recent.push(family);
    }
}

fn reference_card(source: &Value, with_type: bool) -> Value {
    let get = |key: &str, default: &str| source.get(key).cloned().unwrap_or_else(|| json!(default));
    let image = source.get("image").filter(|v| truthy(v)).and_then(Value::as_object).cloned().unwrap_or_default();
    let mut card = serde_json::Map::new();
    card.insert("id".into(), get("id", ""));
    card.insert("name".into(), get("name", ""));
    card.insert("description".into(), get("description", ""));
    if with_type {
        card.insert("reference_type".into(), get("reference_type", "character"));
    }
    card.insert("image".into(), Value::Object(image));
    Value::Object(card)
}

fn by_id(items: &[Value]) -> Vec<(String, &Value)> {
    items.iter().filter_map(|item| Some((text_or_empty(item.get("id")), item)).filter(|(id, _)| !id.is_empty())).collect()
}

fn lookup<'a>(table: &[(String, &'a Value)], id: &str) -> Option<&'a Value> {
    table.iter().find(|(key, _)| key == id).map(|(_, item)| *item)
}

/// `_normalize_generated_minimax_dialogue_scenes`: the plan's scenes as scene cards, with the known characters and locations attached.
pub fn normalize_generated_scenes(raw_scenes: Option<&Value>, subjects: &[Value], locations: &[Value], minimax_h3_mode: Option<&Value>, camera_flow: &str, camera_motion_speed: i64) -> Result<Vec<Value>, String> {
    let Some(Value::Array(raw_scenes)) = raw_scenes else { return Err("MiniMax dialogue plan did not include a scenes array.".into()) };
    let subject_by_id = by_id(subjects);
    let location_by_id = by_id(locations);
    let chosen = Some(text_or_empty(minimax_h3_mode.filter(|v| truthy(v))).trim().to_lowercase().replace(['-', ' '], "_")).filter(|m| !m.is_empty()).unwrap_or_else(|| "text_to_video".into());
    let mode = if matches!(chosen.as_str(), "text_to_video" | "image_to_video" | "reference_to_video" | "video_to_video") { chosen } else { "text_to_video".into() };
    let mut scenes: Vec<Value> = Vec::new();
    for (position, item) in raw_scenes.iter().take(80).enumerate() {
        let index = position + 1;
        let Some(item) = item.as_object() else { continue };
        let listed = match item.get("dialogue_cues") {
            Some(Value::Array(cues)) if !cues.is_empty() => cues.clone(),
            _ => {
                let first = |keys: &[&str]| keys.iter().filter_map(|k| item.get(*k)).find(|v| truthy(v)).cloned().unwrap_or_else(|| json!(""));
                vec![json!({
                    "character_id": first(&["character_id", "subject_id", "speaker_id"]),
                    "speaker": first(&["character_name", "speaker"]),
                    "dialogue": first(&["dialogue", "line", "lyrics"]),
                })]
            }
        };
        let mut assignments: Vec<Value> = Vec::new();
        let mut subject_refs: Vec<Value> = Vec::new();
        let mut seen: Vec<String> = Vec::new();
        for (cue_position, cue) in listed.iter().take(40).enumerate() {
            let Some(cue) = cue.as_object() else { continue };
            let mut subject_id = clean_first(cue, &["character_id", "subject_id", "speaker_id"], "", 180);
            if !subject_id.is_empty() && !subject_by_id.is_empty() && lookup(&subject_by_id, &subject_id).is_none() {
                subject_id.clear();
            }
            let subject = if subject_id.is_empty() { None } else { lookup(&subject_by_id, &subject_id) };
            let subject_name = subject.map(|s| s.get("name"));
            let speaker_name = clean_of(&[cue.get("speaker"), cue.get("character_name"), subject_name.flatten()], "", 160);
            let dialogue = clean_first(cue, &["dialogue", "line", "text"], "", 1200);
            if dialogue.is_empty() {
                continue;
            }
            assignments.push(json!({
                "id": format!("minimax_dialogue_{index}_{}", cue_position + 1),
                "speaker_id": subject_id,
                "speaker_name": if speaker_name.is_empty() { "Speaker".to_string() } else { speaker_name },
                "text": dialogue,
            }));
            if let Some(subject) = subject {
                if !seen.contains(&subject_id) {
                    subject_refs.push(reference_card(subject, true));
                    seen.push(subject_id);
                }
            }
        }
        if let Some(Value::Array(participants)) = item.get("participant_ids") {
            for participant in participants {
                let participant_id = text(Some(participant), 180);
                let Some(card) = (if participant_id.is_empty() { None } else { lookup(&subject_by_id, &participant_id) }) else { continue };
                if seen.contains(&participant_id) {
                    continue;
                }
                subject_refs.push(reference_card(card, true));
                seen.push(participant_id);
            }
        }
        let mut location_id = clean_first(item, &["location_id"], "", 180);
        if !location_id.is_empty() && !location_by_id.is_empty() && lookup(&location_by_id, &location_id).is_none() {
            location_id.clear();
        }
        let location_ref = if location_id.is_empty() { None } else { lookup(&location_by_id, &location_id).map(|l| reference_card(l, false)) };
        let lines: Vec<String> = assignments.iter().map(|cue| format!("{}: \"{}\"", text_or_empty(cue.get("speaker_name")), text_or_empty(cue.get("text")))).collect();
        let label = clean_first(item, &["label", "title"], &format!("Scene {index}"), 160);
        let location_name = location_ref.as_ref().map(|l| l.get("name"));
        let image_prompt = structured_image_prompt(item, subject_refs.first(), location_ref.as_ref());
        let exact_duration = ["exact_duration", "duration"].iter().filter_map(|k| item.get(*k)).find(|v| truthy(v)).cloned().unwrap_or_else(|| json!(0));
        let card = json!({
            "id": clean_first(item, &["id"], &format!("minimax_story_scene_{index}"), 160),
            "scene_number": index,
            "label": if label.is_empty() { format!("Scene {index}") } else { label },
            "lyrics": lines.join("\n"),
            "lyric_singers": assignments.iter().map(|cue| cue.get("speaker_name").cloned().unwrap_or(Value::Null)).collect::<Vec<_>>(),
            "speaker_assignments": assignments,
            "story_beat": clean_first(item, &["story_beat", "beat"], "", 1800),
            "prompt_summary": clean_first(item, &["visual_direction", "summary"], "", 1800),
            "motion_summary": clean_first(item, &["motion_summary", "video_notes"], "", 1400),
            "subjects": subject_refs.iter().map(|s| s.get("name").cloned().unwrap_or(Value::Null)).collect::<Vec<_>>(),
            "subject_refs": subject_refs,
            "setting": clean_of(&[item.get("setting"), item.get("location_name"), location_name.flatten()], "", 1000),
            "location_ref": location_ref,
            "video_prompt_type": "i2v",
            "project_video_engine": "minimax_h3",
            "minimax_h3_mode": mode,
            "minimax_h3_audio_mode": "built_in_audio",
            "performance_mode": "speaking",
            "timeline_start": item.get("timeline_start").cloned().unwrap_or_else(|| json!(0)),
            "timeline_end": item.get("timeline_end").cloned().unwrap_or_else(|| json!(0)),
            "exact_duration": exact_duration,
            "shot_type": clean_first(item, &["shot_type"], "", 160),
            "camera_motion": clean_first(item, &["camera_motion"], "", 500),
            "character_motion": clean_first(item, &["character_motion", "action"], "", 500),
            "facial_performance": clean_first(item, &["facial_performance", "emotion"], "", 240),
            "facial_performance_custom": clean_first(item, &["facial_performance_custom", "delivery"], "", 800),
            "image_prompt": image_prompt,
            "audio_direction": clean_first(item, &["audio_direction"], "", 4000),
            "continuity": clean_first(item, &["continuity"], "", 4000),
            "notes": clean_first(item, &["notes"], "", 4000),
        });
        scenes.push(normalize_storyboard_scene(&card, index as i64));
    }
    if scenes.is_empty() {
        return Err("The LLM returned no usable MiniMax dialogue scenes.".into());
    }
    rebalance_camera_motion(&mut scenes, camera_flow, camera_motion_speed);
    Ok(scenes)
}

/// `_apply_authoritative_script_plan`: one scene for every planned script segment, with the dialogue, durations and times locked to the plan.
pub fn apply_authoritative_script_plan(raw_scenes: Option<&Value>, script_import: &Value) -> Vec<Value> {
    let generated: Vec<Value> = match raw_scenes {
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    };
    let planned: Vec<Value> = script_import.get("scene_plan").and_then(|plan| plan.get("scenes")).and_then(Value::as_array).cloned().unwrap_or_default();
    let mut locked: Vec<Value> = Vec::new();
    let mut previous_location_id = String::new();
    for (index, plan) in planned.iter().enumerate() {
        let mut scene: Obj = generated.get(index).and_then(Value::as_object).cloned().unwrap_or_default();
        let cues: Vec<Value> = plan
            .get("speaker_assignments")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .map(|cue| {
                        let pick = |keys: &[&str], default: &str| keys.iter().filter_map(|k| cue.get(*k)).find(|v| truthy(v)).cloned().unwrap_or_else(|| json!(default));
                        json!({"character_id": pick(&["speaker_id"], ""), "speaker_id": pick(&["speaker_id"], ""), "speaker": pick(&["speaker_name", "speaker_alias"], "Speaker"), "dialogue": pick(&["text"], "")})
                    })
                    .collect()
            })
            .unwrap_or_default();
        let label = scene.get("label").into_iter().chain(plan.get("label")).find(|v| truthy(v)).cloned().unwrap_or_else(|| json!(format!("Script Segment {}", index + 1)));
        scene.insert("label".into(), label);
        scene.insert("dialogue_cues".into(), Value::Array(cues));
        scene.insert("participant_ids".into(), plan.get("participant_ids").filter(|v| truthy(v)).cloned().unwrap_or_else(|| json!([])));
        scene.insert("participant_names".into(), plan.get("participant_names").filter(|v| truthy(v)).cloned().unwrap_or_else(|| json!([])));
        let current_location_id = clean_first(&scene, &["location_id"], "", 180);
        let continuation = plan.get("continuation_of_previous").is_some_and(truthy);
        if continuation && !previous_location_id.is_empty() {
            scene.insert("location_id".into(), json!(previous_location_id));
        } else if !continuation {
            previous_location_id = current_location_id;
        } else if !current_location_id.is_empty() {
            previous_location_id = current_location_id;
        }
        let seconds = |key: &str| plan.get(key).filter(|v| truthy(v)).and_then(crate::pyval::to_float).unwrap_or(0.0);
        scene.insert("exact_duration".into(), json!(seconds("duration_seconds")));
        scene.insert("duration".into(), json!(seconds("duration_seconds")));
        scene.insert("timeline_start".into(), json!(seconds("timeline_start_seconds")));
        scene.insert("timeline_end".into(), json!(seconds("timeline_end_seconds")));
        let notes = [
            text_or_empty(scene.get("notes")),
            format!("Authoritative Script Mapper segment {}. Exact dialogue and order are locked.", index + 1),
            if continuation { "Continuation of the previous script segment.".to_string() } else { String::new() },
        ];
        let joined = notes.iter().filter(|n| !n.is_empty()).cloned().collect::<Vec<_>>().join("\n");
        scene.insert("notes".into(), json!(super::mode::text_s(&joined, 4000)));
        locked.push(Value::Object(scene));
    }
    locked
}
