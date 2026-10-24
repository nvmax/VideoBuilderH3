//! Normalizers for the Storyboard's data: tags, reference cards, the reference catalog and the story layer. Port of the pure parts of
//! `storyboard/scene_helpers.py`.

use fancy_regex::Regex;
use serde_json::{json, Value};

use super::mode::{normalize_story_mode_fields, text, text_s};
use crate::pyval::{text_or_empty, to_float, to_int, truthy, Obj};

fn rx(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid pattern")
}

fn is_match(pattern: &str, text: &str) -> bool {
    rx(pattern).is_match(text).unwrap_or(false)
}

fn array(value: Option<&Value>) -> Vec<Value> {
    match value {
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    }
}

/// `str(a or b or ... or "")`: the first of the values that is not falsy, as text.
pub fn first_truthy<'a>(map: &'a Obj, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().filter_map(|k| map.get(*k)).find(|v| truthy(v))
}

/// `_clean_scene_text(a or b or ...)`.
pub fn clean_of(map: &Obj, keys: &[&str], limit: usize) -> String {
    text(first_truthy(map, keys), limit)
}

/// `_scene_number`: the scene's number, at least 1.
pub fn scene_number(scene: &Obj, fallback: i64) -> i64 {
    let value = scene.get("scene_number").or_else(|| scene.get("number"));
    match value.map_or(Some(fallback), to_int) {
        Some(n) => n.max(1),
        None => (if fallback == 0 { 1 } else { fallback }).max(1),
    }
}

/// `_normalize_tags`: up to twelve short tags from a list or from comma, semicolon or line separated text.
pub fn normalize_tags(value: Option<&Value>) -> Vec<String> {
    if let Some(Value::Array(items)) = value {
        return items.iter().map(|i| text_or_empty(Some(i)).trim().to_string()).filter(|s| !s.is_empty()).map(|s| s.chars().take(120).collect()).take(12).collect();
    }
    let source = text_or_empty(value).trim().to_string();
    if source.is_empty() {
        return Vec::new();
    }
    rx(r"[,;\n]+").split(&source).flatten().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).map(|s| s.chars().take(120).collect()).take(12).collect()
}

/// `_normalize_performance_mode`.
pub fn normalize_performance_mode(value: Option<&Value>) -> String {
    let lowered = text_or_empty(value).trim().to_lowercase();
    let text = rx(r"[\s-]+").replace_all(&lowered, "_").into_owned();
    match text.as_str() {
        "speaking" | "short_film" | "dialogue" | "dialog" => "speaking",
        "no_lip_sync" | "nolipsync" | "no_lipsync" | "no_sync" | "silent" | "visual_only" => "no_lip_sync",
        _ => "singing",
    }
    .to_string()
}

pub fn normalize_reference_image(value: Option<&Value>) -> Value {
    let image = value.and_then(Value::as_object).cloned().unwrap_or_default();
    json!({
        "path": text(image.get("path"), 2000),
        "data": text(image.get("data"), 400_000),
        "name": text(image.get("name"), 240),
    })
}

fn number_or(value: Option<&Value>, default: f64) -> f64 {
    value.and_then(to_float).unwrap_or(default)
}

/// The RefMod part of a Reference Builder card (source, saved RefMod, type, clothing link), or nothing for other cards. Twin of
/// `refmodCardFields` in `web/music_video_builder/refmod_labels.mjs`.
pub fn refmod_card_fields(item: &Obj) -> Obj {
    let mut fields = Obj::new();
    let refmod = item.get("refmod").and_then(Value::as_object);
    let source = text_or_empty(item.get("source")).trim().to_string();
    let Some(refmod) = refmod.filter(|r| source == "refmod" && !text_or_empty(r.get("name")).trim().is_empty()) else { return fields };
    fields.insert("source".into(), json!("refmod"));
    fields.insert(
        "refmod".into(),
        json!({
            "name": text(refmod.get("name"), 400),
            "folder": text(refmod.get("folder"), 80),
            "type": text(refmod.get("type"), 80),
            "kind": if text_or_empty(refmod.get("kind")).trim() == "image" { "image" } else { "video" },
            "tokens": number_or(refmod.get("tokens"), 0.0).trunc() as i64,
            "frames": number_or(refmod.get("frames"), 1.0).trunc() as i64,
            "strength": number_or(refmod.get("strength").or(Some(&json!(1.0))), 1.0).clamp(0.0, 1.0),
        }),
    );
    let reference_type = text_or_empty(item.get("reference_type")).trim().to_string();
    if !reference_type.is_empty() {
        fields.insert("reference_type".into(), json!(text_s(&reference_type, 40)));
    }
    for key in ["wears", "clothing_set"] {
        if !text_or_empty(item.get(key)).trim().is_empty() {
            fields.insert(key.into(), json!(text(item.get(key), 160)));
        }
    }
    if matches!(item.get("follow"), Some(Value::Bool(false))) {
        fields.insert("follow".into(), json!(false));
    }
    fields
}

fn trigger_end(item: &Obj, keys: &[&str]) -> &'static str {
    let position = first_truthy(item, keys).map(|v| text_or_empty(Some(v))).unwrap_or_default().trim().to_lowercase();
    let position = if position.is_empty() { "start".to_string() } else { position };
    if position == "end" { "end" } else { "start" }
}

/// `_normalize_reference_item`.
pub fn normalize_reference_item(value: Option<&Value>, fallback_name: &str, fallback_id: &str) -> Value {
    let item = value.and_then(Value::as_object).cloned().unwrap_or_default();
    let raw_voice = first_truthy(&item, &["minimax_voice", "miniMaxVoice"]).and_then(Value::as_object).cloned().unwrap_or_default();
    let voice_preset = {
        let found = clean_of(&raw_voice, &["preset_id", "presetId", "preset"], 120);
        if found.is_empty() { "none".to_string() } else { found }
    };
    let minimax_voice = json!({
        "preset_id": voice_preset,
        "gender": text(raw_voice.get("gender"), 40),
        "preset_name": clean_of(&raw_voice, &["preset_name", "presetName", "name"], 240),
        "description": clean_of(&raw_voice, &["description", "voice_description", "voiceDescription"], 2000),
    });
    let id = text(item.get("id"), 160);
    let name = text(item.get("name"), 240);
    let mut card = Obj::new();
    card.insert("id".into(), json!(if id.is_empty() { text_s(fallback_id, 160) } else { id }));
    card.insert("name".into(), json!(if name.is_empty() { text_s(fallback_name, 240) } else { name }));
    card.insert("description".into(), json!(text(item.get("description"), 4000)));
    card.insert("minimax_voice".into(), minimax_voice);
    card.insert("trigger_phrase".into(), json!(clean_of(&item, &["trigger_phrase", "trigger", "Trigger"], 1200)));
    card.insert("trigger_position".into(), json!(trigger_end(&item, &["trigger_position", "triggerPosition", "trigger_placement"])));
    card.insert("image".into(), normalize_reference_image(item.get("image").filter(|v| v.is_object())));
    for key in ["reference_type", "wears"] {
        if !text_or_empty(item.get(key)).trim().is_empty() {
            card.insert(key.into(), json!(text(item.get(key), 160)));
        }
    }
    for (key, value) in refmod_card_fields(&item) {
        card.insert(key, value);
    }
    Value::Object(card)
}

pub fn normalize_reference_items(value: Option<&Value>) -> Vec<Value> {
    let mut refs = Vec::new();
    for (index, item) in array(value).iter().take(12).enumerate() {
        if !item.is_object() {
            continue;
        }
        refs.push(normalize_reference_item(Some(item), &format!("Subject {}", index + 1), &format!("subject_{}", index + 1)));
    }
    refs
}

pub fn normalize_speaker_assignments(value: Option<&Value>) -> Vec<Value> {
    let mut assignments = Vec::new();
    for (index, item) in array(value).iter().take(40).enumerate() {
        let Some(item) = item.as_object() else { continue };
        let id = clean_of(item, &["id", "cue_id"], 160);
        assignments.push(json!({
            "id": if id.is_empty() { format!("speaker_cue_{}", index + 1) } else { id },
            "speaker_id": clean_of(item, &["speaker_id", "speakerId", "subject_id"], 160),
            "speaker_name": clean_of(item, &["speaker_name", "speakerName", "speaker", "character"], 240),
            "text": clean_of(item, &["text", "dialogue", "line", "lyric"], 2000),
        }));
    }
    assignments
}

/// `_normalize_reference_catalog`: the Reference Builder's subjects, locations and trigger positions.
pub fn normalize_reference_catalog(value: Option<&Value>) -> Value {
    let source = value.and_then(Value::as_object).cloned().unwrap_or_default();
    let list = |items: Option<&Value>, name: &str, id: &str| -> Vec<Value> {
        let mut refs = Vec::new();
        for (index, item) in array(items).iter().take(180).enumerate() {
            if item.is_object() {
                refs.push(normalize_reference_item(Some(item), &format!("{name} {}", index + 1), &format!("{id}_{}", index + 1)));
            }
        }
        refs
    };
    let position = |keys: &[&str]| trigger_end(&source, keys);
    json!({
        "subjects": list(source.get("subjects"), "Subject", "subject"),
        "locations": list(source.get("locations"), "Location", "location"),
        "locations_cleared": source.get("locations_cleared").is_some_and(truthy),
        "trigger_position": position(&["trigger_position", "triggerPosition", "trigger_placement"]),
        "subject_trigger_position": position(&["subject_trigger_position", "subjectTriggerPosition", "trigger_position"]),
        "location_trigger_position": position(&["location_trigger_position", "locationTriggerPosition", "trigger_position"]),
    })
}

const IMAGE_WORLD_STYLES: [&str; 6] = ["natural", "surreal_subject", "balanced_surreal", "full_surreal", "abstract", "custom"];

/// `_normalize_story_layer`.
pub fn normalize_story_layer(value: Option<&Value>) -> Value {
    let source = value.and_then(Value::as_object).cloned().unwrap_or_default();
    let style = first_truthy(&source, &["image_world_style", "imageWorldStyle"]).map(|v| text_or_empty(Some(v))).filter(|s| !s.is_empty()).unwrap_or_else(|| "natural".into());
    let style = if IMAGE_WORLD_STYLES.contains(&style.as_str()) { style } else { "natural".into() };
    let strength_source = source.get("lyric_story_strength").or_else(|| source.get("lyricStoryStrength")).cloned().unwrap_or_else(|| json!(7));
    let strength = to_float(&strength_source).filter(|f| f.is_finite()).map_or(7, |f| f.trunc() as i64).clamp(0, 10);
    let mut layer = Obj::new();
    layer.insert("enabled".into(), json!(source.get("enabled").is_none_or(truthy)));
    layer.insert("overall_story_idea".into(), json!(clean_of(&source, &["overall_story_idea", "overallStoryIdea", "story_idea", "storyIdea"], 4000)));
    layer.insert("user_story_arc".into(), json!(clean_of(&source, &["user_story_arc", "userStoryArc"], 40000)));
    layer.insert("song_story_brief".into(), json!(clean_of(&source, &["song_story_brief", "songStoryBrief"], 4000)));
    layer.insert("lyric_story_strength".into(), json!(strength));
    layer.insert("image_world_style".into(), json!(style));
    layer.insert("image_custom_style_direction".into(), json!(clean_of(&source, &["image_custom_style_direction", "imageCustomStyleDirection"], 3000)));
    for (key, value) in normalize_story_mode_fields(&source) {
        layer.insert(key, value);
    }
    Value::Object(layer)
}

/// `_lyric_story_strength_guidance`.
pub fn lyric_story_strength_guidance(story_layer: &Value) -> String {
    let strength = story_layer.get("lyric_story_strength").and_then(to_float).filter(|f| f.is_finite()).map_or(7, |f| f.trunc() as i64).clamp(0, 10);
    let guidance = if strength <= 0 {
        "Ignore the lyrics as story source. Use the story arc, style, subjects, and locations instead. Do not force lyric objects, actions, or meanings into scenes."
    } else if strength <= 3 {
        "Use lyrics lightly as mood and emotional timing only. Avoid literal lyric objects/actions unless they naturally support the story."
    } else if strength <= 6 {
        "Balance lyrics with the story arc. Each vocal scene should reflect the lyric's emotional intent, and concrete lyric anchors can appear when they fit."
    } else if strength <= 8 {
        "Lyrics strongly shape the story. For each vocal scene, preserve the lyric's main feeling, situation, or image, and include a recognizable lyric anchor when possible."
    } else {
        "Use lyrics as literally as possible while staying cinematic. For every non-instrumental scene, include at least one concrete object, action, emotion, or situation from that exact lyric line unless it would be impossible or unsafe."
    };
    format!("Lyric Story Strength: {strength}/10. {guidance}")
}

/// Story Arc detail level to `(max words per section, total word budget across all sections)`.
pub fn story_arc_detail_profile(level: &str) -> (i64, i64) {
    match level {
        "compact" => (60, 1000),
        "detailed" => (160, 2400),
        "rich" => (240, 3600),
        _ => (100, 1500),
    }
}

/// Story Arc detail level to the most words per scene entry when the arc is written scene by scene.
pub fn story_arc_entry_words(level: &str) -> i64 {
    match level {
        "compact" => 35,
        "detailed" => 80,
        "rich" => 110,
        _ => 55,
    }
}

pub fn normalize_story_arc_detail(value: Option<&Value>) -> String {
    let key = text_or_empty(value).trim().to_lowercase();
    if matches!(key.as_str(), "compact" | "standard" | "detailed" | "rich") { key } else { "standard".into() }
}

/// `_speed_value`: a whole number from 0 to 10.
pub fn speed_value(value: Option<&Value>, fallback: i64) -> i64 {
    value.and_then(to_float).filter(|f| f.is_finite()).map_or(fallback, |f| f.trunc() as i64).clamp(0, 10)
}

/// `_safe_file_stem`.
pub fn safe_file_stem(value: &str, fallback: &str) -> String {
    let cleaned = rx(r"[^A-Za-z0-9._-]+").replace_all(value.trim(), "_").into_owned();
    let stripped = cleaned.trim_matches(['.', '_']);
    let cut: String = stripped.chars().take(90).collect();
    let chosen = if cut.is_empty() { fallback } else { cut.as_str() };
    let finished = chosen.trim_matches(['.', '_']);
    if finished.is_empty() { fallback.to_string() } else { finished.to_string() }
}

/// `_storyboard_scene_has_visible_character`.
pub fn scene_has_visible_character(scene: &Value) -> bool {
    let Some(map) = scene.as_object() else { return false };
    if map.get("vocal_status").and_then(Value::as_object).is_some_and(|s| s.get("no_character_present").is_some_and(truthy)) {
        return false;
    }
    if map.get("no_character_present").is_some_and(truthy) || map.get("noCharacterPresent").is_some_and(truthy) {
        return false;
    }
    ["subject_refs", "subjects", "visible_subjects", "visibleSubjects"].iter().any(|k| map.get(*k).is_some_and(truthy))
}

/// `_storyboard_prompt_mentions_visible_face`.
pub fn prompt_mentions_visible_face(prompt: &str) -> bool {
    let lowered = text_s(prompt, 12000).to_lowercase();
    !lowered.is_empty()
        && is_match(r"(?i)\b(?:woman|man|girl|boy|person|subject|singer|rapper|performer|speaker|character|face|eyes?|brows?|gaze|mouth|jaw|cheeks?|expression|smile|frown|sings?|singing|says|speaks?)\b", &lowered)
}

/// The subjects and locations of the Reference Builder in a request, trimmed for a prompt (no picture data).
pub fn dialogue_reference_catalog(payload: &Obj) -> (Vec<Value>, Vec<Value>) {
    let builder = first_truthy(payload, &["reference_builder", "referenceBuilder"]).filter(|v| v.is_object()).cloned();
    let catalog = normalize_reference_catalog(builder.as_ref());
    let trimmed_image = |item: &Value| {
        let image = item.get("image").and_then(Value::as_object).cloned().unwrap_or_default();
        json!({"path": text(image.get("path"), 2000), "data": "", "name": text(image.get("name"), 240)})
    };
    let mut subjects = Vec::new();
    for subject in array(catalog.get("subjects")) {
        let Some(map) = subject.as_object() else { continue };
        let (id, name, description) = (text(map.get("id"), 160), text(map.get("name"), 160), text(map.get("description"), 1200));
        if !id.is_empty() || !name.is_empty() || !description.is_empty() {
            let kind = text(map.get("reference_type"), 80);
            subjects.push(json!({
                "id": id,
                "name": if !name.is_empty() { name } else if !id.is_empty() { id.clone() } else { "Character".into() },
                "description": description,
                "reference_type": if kind.is_empty() { "character".to_string() } else { kind },
                "image": trimmed_image(&subject),
            }));
        }
    }
    let mut locations = Vec::new();
    for location in array(catalog.get("locations")) {
        let Some(map) = location.as_object() else { continue };
        let (id, name, description) = (text(map.get("id"), 160), text(map.get("name"), 160), text(map.get("description"), 1200));
        if !id.is_empty() || !name.is_empty() || !description.is_empty() {
            locations.push(json!({
                "id": id,
                "name": if !name.is_empty() { name } else if !id.is_empty() { id.clone() } else { "Location".into() },
                "description": description,
                "image": trimmed_image(&location),
            }));
        }
    }
    (subjects, locations)
}

/// The kinds of picture a data URL can hold, and the bytes. `Err` carries the pack's message.
pub fn decode_image_data_url(value: &str) -> Result<(Vec<u8>, String), String> {
    use base64::Engine;
    let source = value.trim();
    let (mut ext, payload) = match rx(r"(?s)^data:image/([A-Za-z0-9.+-]+);base64,(.*)$").captures(source).ok().flatten() {
        Some(caps) => (caps[1].to_lowercase(), caps[2].to_string()),
        None => ("png".to_string(), source.to_string()),
    };
    if ext == "jpeg" {
        ext = "jpg".into();
    }
    if !matches!(ext.as_str(), "png" | "jpg" | "webp") {
        ext = "png".into();
    }
    // Python's b64decode drops what is not in the alphabet.
    let compact: String = payload.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=')).collect();
    let data = base64::engine::general_purpose::STANDARD.decode(compact.as_bytes()).map_err(|_| "Reference image data could not be decoded.".to_string())?;
    if data.is_empty() {
        return Err("Reference image data is empty.".into());
    }
    if data.len() > 30 * 1024 * 1024 {
        return Err("Reference image is too large.".into());
    }
    Ok((data, ext))
}
