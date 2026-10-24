//! Motion notes: one short camera and movement note for every scene, written from the concept prompts and, by source mode, the director and
//! timeline notes. Port of `_generate_builder_motion_notes` in `llm/video_prompt_generation.py`.

use fancy_regex::Regex;
use h3_core::pyjson::dumps;
use h3_core::pyval::{text_or_empty, to_int, truthy, Obj};
use h3_llm::clean::clean_plain_text;
use h3_llm::locations::extract_json_object;
use serde_json::{json, Map, Value};

use super::{number, text, whole, Llm, TextCall};

type Reply = Result<Value, String>;

fn rx(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid pattern")
}

/// `str(value or "").strip()[:limit]`.
fn clip(value: Option<&Value>, limit: usize) -> String {
    text_or_empty(value).trim().chars().take(limit).collect()
}

/// `str(payload.get(key) or default).strip().lower()`: a value that is only blanks is kept as it is.
fn mode(payload: &Obj, key: &str, default: &str) -> String {
    match payload.get(key).filter(|v| truthy(v)) {
        Some(value) => text_or_empty(Some(value)).trim().to_lowercase(),
        None => default.to_string(),
    }
}

fn cleaned_scenes(payload: &Obj) -> Result<Vec<Value>, String> {
    let scenes = payload.get("scenes").and_then(Value::as_array).filter(|s| !s.is_empty()).ok_or("No scenes were provided for motion note creation.")?;
    let mut cleaned: Vec<Value> = Vec::new();
    for scene in scenes.iter().take(20) {
        let Some(scene) = scene.as_object() else { continue };
        let fallback = cleaned.len() as i64 + 1;
        let number = ["scene_number", "number"].iter().filter_map(|k| scene.get(*k)).find(|v| truthy(v)).map_or(fallback, |v| to_int(v).unwrap_or(fallback));
        let label = match scene.get("label").filter(|v| truthy(v)) {
            Some(value) => clip(Some(value), 160),
            None => format!("Scene {number}"),
        };
        let notes: Vec<Value> = scene
            .get("timeline_notes")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .take(8)
                    .filter_map(Value::as_object)
                    .map(|item| {
                        let label = ["label", "type"].iter().filter_map(|k| item.get(*k)).find(|v| truthy(v));
                        json!({
                            "label": label.map_or_else(|| "note".to_string(), |v| clip(Some(v), 160)),
                            "note": clip(item.get("note"), 800),
                            "start": item.get("start").cloned().unwrap_or(Value::Null),
                            "end": item.get("end").cloned().unwrap_or(Value::Null),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        cleaned.push(json!({
            "scene_number": number.max(1),
            "label": label,
            "concept_prompt": clip(scene.get("concept_prompt"), 1600),
            "director_note": clip(scene.get("director_note"), 1200),
            "timeline_notes": notes,
        }));
    }
    if cleaned.is_empty() {
        return Err("No valid scenes were provided for motion note creation.".into());
    }
    Ok(cleaned)
}

/// The instruction for a batch of scenes.
pub fn instruction(payload: &Obj) -> Result<String, String> {
    let source_mode = mode(payload, "source_mode", "concept");
    let story_idea = text(payload, "story_idea");
    let theme_style = text(payload, "theme_style");
    let previous = text(payload, "previous_summary");
    let video_mode = mode(payload, "video_mode", "i2v");
    let scenes = cleaned_scenes(payload)?;
    let source_note = match source_mode.as_str() {
        "concept" => "Use only concept prompts.",
        "concept_director" => "Use concept prompts and director notes.",
        "concept_timeline" => "Use concept prompts and overlapping timeline notes.",
        "all" => "Use concept prompts, director notes, and overlapping timeline notes.",
        _ => "Use concept prompts.",
    };
    let or = |text: &str, empty: &str| if text.is_empty() { empty.to_string() } else { text.to_string() };
    Ok(format!(
        "You will create concise I2V/T2V motion notes for scene-by-scene music video generation.\n\
Return only valid JSON in this exact flat format:\n\
{{\n  \"Motion1\": \"\",\n  \"Motion2\": \"\"\n}}\n\n\
Source mode: {source_mode}. {source_note}\n\
Target video mode: {}.\n\n\
Write one motion note for each provided scene.\n\
Each motion note should describe camera motion, subject movement or interaction, environmental motion, and emotional pacing.\n\
Use the concept prompt as the main visual action source. Use director notes and timeline notes only when provided by the source mode.\n\n\
Rules:\n\
- Keep each motion note one sentence or one short paragraph.\n\
- Do not write a full video prompt.\n\
- Do not repeat character appearance details.\n\
- Do not include camera settings, frame rate, aspect ratio, render quality, or model tags.\n\
- For instrumental/no-subject scenes, focus on environmental movement, transitions, symbolic motion, light, particles, atmosphere, or camera drift.\n\
- For solo character scenes, include performance/reaction movement, facial/emotional energy, body motion, and camera movement.\n\
- For two-character scenes, include interaction, blocking, distance, reaction, duet/confrontation energy, and camera movement.\n\
- Keep motion connected across the batch like shots from the same music video.\n\
- Return only the JSON object.\n\n\
Story idea:\n{}\n\n\
Style/theme:\n{}\n\n\
Previous batch motion summary:\n{}\n\n\
Scenes:\n{}",
        video_mode.to_uppercase(),
        or(&story_idea, "[none provided]"),
        or(&theme_style, "[none provided]"),
        or(&previous, "[none yet]"),
        dumps(&Value::Array(scenes)),
    ))
}

/// `int(digits)` as text: no leading zeros, so numbers of any length print and compare like Python's.
fn whole_number(digits: &str) -> String {
    let trimmed = digits.trim_start_matches('0');
    if trimmed.is_empty() { "0".to_string() } else { trimmed.to_string() }
}

/// `POST generate_motion_notes`.
pub async fn generate_motion_notes(llm: &dyn Llm, payload: &Obj) -> Reply {
    let instruction = instruction(payload)?;
    let call = TextCall::new(instruction, number(payload, "temperature", 0.45)?, number(payload, "top_p", 0.95)?, whole(payload, "max_new_tokens", 1200)?, "Motion Note Creator").paragraphs();
    let (text, info) = llm.text(call).await?;
    let raw = clean_plain_text(&text);
    let mut notes: Map<String, Value> = Map::new();
    if let Ok(Value::Object(parsed)) = extract_json_object(&raw) {
        let key = rx(r"(?i)(?:motion|scene)\s*(\d+)");
        for (name, value) in &parsed {
            let Some(digits) = key.captures(name).ok().flatten().and_then(|c| c.get(1)) else { continue };
            let note = text_or_empty(Some(value)).trim().to_string();
            if !note.is_empty() {
                notes.insert(format!("Motion{}", whole_number(digits.as_str())), json!(note));
            }
        }
    }
    if notes.is_empty() {
        for caps in rx(r#"(?is)"?\b(?:Motion|Scene)\s*(\d+)"?\s*:\s*"([^"]+)""#).captures_iter(&raw).flatten() {
            let (Some(digits), Some(body)) = (caps.get(1), caps.get(2)) else { continue };
            let note = body.as_str().trim().to_string();
            if !note.is_empty() {
                notes.insert(format!("Motion{}", whole_number(digits.as_str())), json!(note));
            }
        }
    }
    if notes.is_empty() {
        return Err("Gemma did not return any MotionN notes.".into());
    }
    let mut keys: Vec<&String> = notes.keys().collect();
    keys.sort_by_key(|k| (k.len(), k.as_str()));
    let lines: Vec<String> = keys.iter().map(|k| format!("{k}: {}", text_or_empty(notes.get(*k)).chars().take(180).collect::<String>())).collect();
    let summary: String = lines.join("\n").chars().take(1200).collect();
    Ok(json!({"notes": Value::Object(notes), "raw": raw, "summary": summary, "run_info": Value::Object(info)}))
}
