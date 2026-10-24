//! The location scout ("LM Extract"): filming locations from lyrics, scene notes and the style theme. Port of
//! `_generate_lm_scout_locations` in `llm/image_prompt_generation.py` and the helpers it uses in `llm/output_checks.py`.

use fancy_regex::{Captures, Regex};
use serde_json::{json, Value};

use crate::checks::clean_visual_text;
use crate::client::{chat, ChatRequest, LlmError};
use crate::config::LlmConfig;

const INSTRUCTIONS: &str = include_str!("../data/location_scout.txt");

fn rx(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid pattern")
}

fn sub(pattern: &str, text: &str, replacement: &str) -> String {
    rx(pattern).replace_all(text, replacement).into_owned()
}

fn collapse(text: &str) -> String {
    sub(r"\s+", text, " ")
}

fn is_match(pattern: &str, text: &str) -> bool {
    rx(pattern).is_match(text).unwrap_or(false)
}

/// The JSON shape the scout asks for.
pub fn scout_schema() -> Value {
    json!({
        "type": "object",
        "properties": {"locations": {"type": "array", "items": {
            "type": "object",
            "properties": {"name": {"type": "string"}, "description": {"type": "string"}},
            "required": ["name", "description"],
            "additionalProperties": false
        }}},
        "required": ["locations"],
        "additionalProperties": false
    })
}

// -- reply handling ---------------------------------------------------------------------------------------------------------

const JSON_KEYS: &str = "locations|scene_map|name|description|id|label|prompt|runner|used_model|title|premise|scenes|character_id|subject_id|speaker_id|location_id|dialogue|line|lyrics|story_beat|beat|visual_direction|summary|image_prompt|visual_prompt|shot_type|camera_motion|facial_performance|facial_performance_custom|emotion|delivery|motion_summary|video_notes|setting|location_name|character_name|speaker";

/// Fix the usual slips in JSON a model writes: curly quotes, comments, trailing commas, bare keys, missing commas.
pub fn repair_json_like_text(text: &str) -> String {
    let mut repaired = text.trim().replace(['\u{201c}', '\u{201d}'], "\"").replace(['\u{2018}', '\u{2019}'], "'");
    repaired = sub(r"(?m)//.*?$", &repaired, "");
    repaired = sub(r",\s*([}\]])", &repaired, "$1");
    let scene = |caps: &Captures| format!("{}\"{}\":", &caps[1], caps[2].replace(' ', ""));
    repaired = rx(&format!(r"(?i)([{{\[,]\s*)({JSON_KEYS})\s*:")).replace_all(&repaired, "${1}\"${2}\":").into_owned();
    repaired = rx(r"(?i)([{\[,]\s*)(Scene\s*\d+|Scene\d+)\s*:").replace_all(&repaired, scene).into_owned();
    repaired = rx(&format!(r"(?im)(^\s*)({JSON_KEYS})\s*:")).replace_all(&repaired, "${1}\"${2}\":").into_owned();
    repaired = rx(r"(?im)(^\s*)(Scene\s*\d+|Scene\d+)\s*:").replace_all(&repaired, scene).into_owned();
    repaired = sub(r#"(["}\]])\s*\n\s*(")"#, &repaired, "$1,\n$2");
    repaired = sub(r"(})\s*\n\s*(\{)", &repaired, "$1,\n$2");
    repaired = sub(r"(})\s*(\{)", &repaired, "$1,$2");
    sub(r#"(?i)(\])\s*("scene_map"\s*:)"#, &repaired, "$1,$2")
}

/// The JSON in a model reply, from the first `{` to the last `}`, repaired when it does not parse as it is.
pub fn extract_json_object(text: &str) -> Result<Value, String> {
    let mut cleaned = sub(r"(?i)^```(?:json)?\s*", text.trim(), "");
    cleaned = sub(r"\s*```$", &cleaned, "");
    if let (Some(start), Some(end)) = (cleaned.find('{'), cleaned.rfind('}')) {
        if end > start {
            cleaned = cleaned[start..=end].to_string();
        }
    }
    let mut last_error = None;
    for candidate in [cleaned.clone(), repair_json_like_text(&cleaned)] {
        if candidate.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(&candidate) {
            Ok(value) => return Ok(value),
            Err(e) => last_error = Some(e.to_string()),
        }
    }
    Err(last_error.unwrap_or_else(|| "Gemma did not return a JSON object.".into()))
}

/// Text that is the model talking about the task instead of naming a place.
pub fn looks_like_location_meta_text(value: &str) -> bool {
    let text = collapse(value.trim()).to_lowercase();
    if text.is_empty() {
        return true;
    }
    if text.chars().count() > 140
        && !is_match(r"\b(?:room|hall|hallway|corridor|street|road|forest|temple|pool|motel|stage|club|warehouse|desert|beach|shore|city|rooftop|alley|kitchen|bedroom|bathroom|church|chapel|station|train|car|bus|field|garden|vault|cave|lake|river|bridge|tunnel|apartment|house|mansion|hotel|bar|lounge|studio|parking|garage)\b", &text)
    {
        return true;
    }
    const PATTERNS: [&str; 14] = [
        r"\bsince the provided\b",
        r"\bprovided .*content was not visible\b",
        r"\bprovided .*prompt\b",
        r"\bsubjectsandscenes\.txt\b",
        r"\bhere is (?:a|the)\b",
        r"\breusable location list\b",
        r"\bbased on the structural context\b",
        r"\bscene descriptions imply\b",
        r"\bcohesive project\b",
        r"\bi can(?:not|'t)\b",
        r"\bi(?:'|\u{2019})m sorry\b",
        r"\bas an ai\b",
        r"\boutput format\b",
        r"\buser input\b",
    ];
    PATTERNS.iter().any(|p| is_match(&format!("(?i){p}"), &text))
}

/// Notes and theme text with file names and chatter taken out.
pub fn clean_location_context_text(value: &str) -> String {
    let text = value.trim();
    if text.is_empty() {
        return String::new();
    }
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines: Vec<&str> = Vec::new();
    for raw in normalized.split('\n').map(str::trim) {
        if raw.is_empty() || is_match(r"(?i)\bsubjectsandscenes\.txt\b", raw) {
            continue;
        }
        if is_match(r"(?i)\.(?:txt|json|srt)\b", raw) && is_match(r"[A-Za-z]:\\|/|\\", raw) {
            continue;
        }
        if looks_like_location_meta_text(raw) {
            continue;
        }
        lines.push(raw);
    }
    let cleaned = lines.join("\n").trim().to_string();
    if looks_like_location_meta_text(&cleaned) { String::new() } else { cleaned }
}

/// The characters as one note for the scout: `Name (type): description`, a line each.
pub fn subject_context_for_locations(refs: &Value) -> String {
    let text = |v: Option<&Value>| match v {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Null) | None | Some(Value::Bool(false)) => String::new(),
        Some(other) => other.to_string().trim().to_string(),
    };
    let mut lines = Vec::new();
    for subject in refs.get("subjects").and_then(Value::as_array).into_iter().flatten() {
        let (name, description) = (text(subject.get("name")), text(subject.get("description")));
        if name.is_empty() && description.is_empty() {
            continue;
        }
        let kind = Some(text(subject.get("reference_type"))).filter(|k| !k.is_empty()).unwrap_or_else(|| "character".into());
        let who = if name.is_empty() { "Reference".to_string() } else { name };
        lines.push(format!("{who} ({kind})") + &if description.is_empty() { String::new() } else { format!(": {description}") });
    }
    lines.join("\n")
}

// -- the request ------------------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct ScoutInput {
    pub lyrics_text: String,
    /// `(concept, notes)` for each scene that has either.
    pub scenes: Vec<(String, String)>,
    pub style_theme: String,
    pub subject_context: String,
    /// Names of the locations the project already has.
    pub existing_locations: Vec<String>,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub max_new_tokens: Option<u32>,
}

/// The instruction sent to the model. An error says what is missing.
pub fn scout_instruction(input: &ScoutInput) -> Result<String, String> {
    let lyrics = input.lyrics_text.trim();
    let scene_lines: Vec<String> = input
        .scenes
        .iter()
        .enumerate()
        .filter_map(|(i, (concept, notes))| {
            let (concept, notes) = (concept.trim(), notes.trim());
            (!concept.is_empty() || !notes.is_empty()).then(|| format!("Scene {}: {concept}", i + 1) + &if notes.is_empty() { String::new() } else { format!(" | notes: {notes}") })
        })
        .collect();
    if lyrics.is_empty() && scene_lines.is_empty() {
        return Err("LM Extract needs lyrics, scene notes, or concept prompts first.".into());
    }
    let style = clean_location_context_text(&input.style_theme);
    let subjects = clean_location_context_text(&input.subject_context);
    let existing: Vec<String> = input.existing_locations.iter().map(|n| collapse(n.trim())).filter(|n| !n.is_empty()).map(|n| format!("- {n}")).collect();
    let or_none = |s: String| if s.is_empty() { "(none)".to_string() } else { s };
    Ok(format!(
        "{INSTRUCTIONS}\n--- PROJECT INPUTS ---\n\nStyle/theme notes:\n{}\n\nCharacter notes:\n{}\n\nExisting user locations (do not repeat these):\n{}\n\nScene concepts and notes (optional extra context):\n{}\n\nSong lyrics:\n{}",
        or_none(style),
        or_none(subjects),
        or_none(existing.join("\n")),
        or_none(scene_lines.join("\n")),
        or_none(lyrics.to_string()),
    ))
}

/// A place the scout found.
#[derive(Debug, Clone, PartialEq)]
pub struct Location {
    pub name: String,
    pub description: String,
}

/// The reply flattened to one line and cut at 700 characters, for an error message.
pub fn preview(text: &str) -> String {
    let flat = collapse(text).trim().to_string();
    if flat.chars().count() > 700 {
        format!("{}...", flat.chars().take(697).collect::<String>().trim_end())
    } else {
        flat
    }
}

/// The locations in a scout reply, without repeats.
pub fn parse_scout_reply(text: &str) -> Result<Vec<Location>, String> {
    let data = extract_json_object(&clean_visual_text(text)).ok();
    let raw = match &data {
        Some(Value::Object(map)) => map.get("locations").cloned(),
        other => other.clone(),
    };
    let mut seen = std::collections::HashSet::new();
    let mut locations = Vec::new();
    let as_text = |v: Option<&Value>| match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None | Some(Value::Bool(false)) => String::new(),
        Some(other) => other.to_string(),
    };
    for item in raw.as_ref().and_then(Value::as_array).into_iter().flatten() {
        let Some(obj) = item.as_object() else { continue };
        let name = collapse(as_text(obj.get("name")).trim_matches([' ', ',', '.', '-']));
        let description = collapse(as_text(obj.get("description")).trim()).trim().to_string();
        if !name.is_empty() && seen.insert(name.to_lowercase()) {
            locations.push(Location { name, description });
        }
    }
    if locations.is_empty() {
        let shown = preview(text);
        return Err(format!("The LLM did not return usable location JSON. Raw response preview: {}", if shown.is_empty() { "(empty)" } else { &shown }));
    }
    locations.truncate(50);
    Ok(locations)
}

#[derive(Debug, Clone)]
pub struct ScoutResult {
    pub locations: Vec<Location>,
    pub model: String,
    pub runner: String,
}

/// Ask the model for locations. A reply that holds none is an [`LlmError::Empty`].
pub async fn scout_locations(cfg: &LlmConfig, input: &ScoutInput) -> Result<ScoutResult, LlmError> {
    let instruction = scout_instruction(input).map_err(LlmError::Request)?;
    let mut request = ChatRequest::text(&instruction, input.temperature.filter(|t| *t != 0.0).unwrap_or(0.6), input.top_p.filter(|t| *t != 0.0).unwrap_or(0.9), input.max_new_tokens.filter(|t| *t != 0).unwrap_or(4000));
    request.json_schema = Some(scout_schema());
    let reply = chat(cfg, &request).await?;
    let locations = parse_scout_reply(&reply.text).map_err(LlmError::Empty)?;
    Ok(ScoutResult { locations, model: reply.model, runner: reply.runner })
}
