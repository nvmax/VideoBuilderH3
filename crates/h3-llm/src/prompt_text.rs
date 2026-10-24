//! Reading and checking what an LLM wrote as an image or video prompt. Port of `llm/text_cleaning.py`
//! (`extract_prompt_text_from_gemma_output`) and the prompt checks of `llm/output_checks.py`.

use std::collections::HashSet;

use fancy_regex::Regex;
use h3_core::pyval::{py_str, text_or_empty, to_float, truthy, Obj};
use serde_json::Value;

use crate::checks::{looks_like_bad_reference_description, looks_like_repeat_failure, looks_like_unfilled_template};

fn rx(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid pattern")
}

const PROMPT_KEYS: [&str; 9] = ["image_prompt", "t2i_prompt", "text_to_image_prompt", "prompt", "flux_prompt", "nb_prompt", "nano_banana_prompt", "ernie_prompt", "enhance_prompt"];

/// `_strip_json_fence`: a leading `thought` label and a code fence around JSON removed.
pub fn strip_json_fence(text: &str) -> String {
    let mut cleaned = text.trim().to_string();
    cleaned = rx(r"(?i)^\s*[^A-Za-z0-9]*(?:(?:user|assistant|model)\b)?[^A-Za-z0-9]*(?:thought|analysis|reasoning)(?=[A-Z]|[^A-Za-z0-9]|$)[^A-Za-z0-9]*").replace(&cleaned, "").trim().to_string();
    cleaned = rx(r"(?i)^```(?:json)?\s*").replace(&cleaned, "").into_owned();
    cleaned = rx(r"\s*```$").replace(&cleaned, "").into_owned();
    cleaned.trim().to_string()
}

fn json_candidates(text: &str) -> Vec<String> {
    let cleaned = strip_json_fence(text);
    let mut out = vec![cleaned.clone()];
    let starts: Vec<usize> = ['{', '['].iter().filter_map(|c| cleaned.find(*c)).collect();
    if let Some(start) = starts.into_iter().min() {
        let end = ['}', ']'].iter().filter_map(|c| cleaned.rfind(*c)).max();
        if let Some(end) = end.filter(|e| *e > start) {
            out.push(cleaned[start..=end].to_string());
        }
    }
    out
}

/// `_scene_number`: the first number in the value, when it is above zero.
fn scene_number(value: Option<&Value>) -> Option<i64> {
    let value = value.filter(|v| !v.is_null())?;
    let text = py_str(Some(value));
    let found = rx(r"\d+").find(&text).ok().flatten()?;
    found.as_str().parse::<i64>().ok().filter(|n| *n > 0)
}

fn walk_prompt_values(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for key in PROMPT_KEYS {
                let text = text_or_empty(map.get(key)).trim().to_string();
                if !text.is_empty() {
                    out.push(text);
                }
            }
            for child in map.values() {
                walk_prompt_values(child, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|i| walk_prompt_values(i, out)),
        _ => {}
    }
}

fn items_for_scene<'a>(parsed: &'a Value, target: Option<i64>) -> Vec<&'a Value> {
    let objects = |list: &'a Vec<Value>| -> Vec<&'a Value> { list.iter().filter(|i| i.is_object()).collect() };
    let items: Vec<&Value> = match parsed {
        Value::Array(list) => objects(list),
        Value::Object(map) => ["scenes", "prompts", "items", "results"]
            .iter()
            .find_map(|key| map.get(*key).and_then(Value::as_array))
            .map(objects)
            .unwrap_or_else(|| vec![parsed]),
        _ => Vec::new(),
    };
    if let Some(target) = target {
        let matched: Vec<&Value> = items
            .iter()
            .filter(|item| {
                let number = ["scene_number", "sceneNumber", "scene", "number"].iter().filter_map(|k| item.get(*k)).find(|v| truthy(v));
                scene_number(number) == Some(target)
            })
            .copied()
            .collect();
        if !matched.is_empty() {
            return matched;
        }
    }
    items
}

/// `extract_prompt_text_from_gemma_output`: the prompt text when the model answered with JSON, else the text itself.
pub fn extract_prompt_text(text: &str, scene: Option<&Value>) -> String {
    let original = strip_json_fence(text);
    if original.is_empty() {
        return original;
    }
    let target = scene_number(scene);
    for candidate in json_candidates(&original) {
        let Ok(parsed) = h3_core::pyjson::parse(&candidate) else { continue };
        for item in items_for_scene(&parsed, target) {
            let mut found = Vec::new();
            walk_prompt_values(item, &mut found);
            if let Some(first) = found.into_iter().next() {
                return first;
            }
        }
        let mut found = Vec::new();
        walk_prompt_values(&parsed, &mut found);
        if let Some(first) = found.into_iter().next() {
            return first;
        }
    }
    original
}

/// `_looks_like_id_lora_script_prompt`.
pub fn looks_like_id_lora_script_prompt(text: &str) -> bool {
    let sample = text.trim().to_lowercase();
    ["[visual]", "[speech]", "[sounds]"].iter().all(|label| sample.contains(label))
}

/// `_normalized_prompt_similarity_text`: the words of a text, lower case, one space apart.
fn similarity_text(text: &str) -> String {
    rx(r"[\w']+").find_iter(&text.to_lowercase()).flatten().map(|m| m.as_str().to_string()).collect::<Vec<_>>().join(" ")
}

/// `_looks_like_source_lyric_echo`: the prompt is the scene's lyric again.
pub fn looks_like_source_lyric_echo(text: &str, payload: &Obj) -> bool {
    let source = ["lyric_text", "lyrics"].iter().map(|k| text_or_empty(payload.get(*k)).trim().to_string()).find(|t| !t.is_empty()).unwrap_or_default();
    let candidate = text.trim();
    if source.is_empty() || candidate.is_empty() {
        return false;
    }
    let (source_norm, candidate_norm) = (similarity_text(&source), similarity_text(candidate));
    if source_norm.is_empty() || candidate_norm.is_empty() {
        return false;
    }
    if candidate_norm == source_norm {
        return true;
    }
    let source_tokens: Vec<&str> = source_norm.split(' ').collect();
    let candidate_tokens: Vec<&str> = candidate_norm.split(' ').collect();
    if source_tokens.len() < 4 || candidate_tokens.len() < 4 {
        return false;
    }
    let source_set: HashSet<&str> = source_tokens.iter().copied().collect();
    let candidate_set: HashSet<&str> = candidate_tokens.iter().copied().collect();
    let overlap = source_set.intersection(&candidate_set).count() as f64 / candidate_set.len().max(1) as f64;
    let length_ratio = source_tokens.len().min(candidate_tokens.len()) as f64 / source_tokens.len().max(candidate_tokens.len()) as f64;
    const VISUAL: [&str; 23] = [
        "cinematic", "shot", "still", "frame", "portrait", "wide", "closeup", "close", "medium", "camera", "lens", "lighting", "background", "foreground", "composition", "scene", "environment", "location", "color", "palette", "texture", "detail", "reference",
    ];
    let has_visual_terms = candidate_set.iter().any(|t| VISUAL.contains(t));
    overlap >= 0.90 && length_ratio >= 0.75 && !has_visual_terms
}

/// `_validate_reference_description`.
pub fn validate_reference_description(text: &str, label: &str) -> Result<(), String> {
    if looks_like_bad_reference_description(text) {
        return Err(format!("Gemma returned unusable repeated text for the {label} description. Try again, or use a clearer reference image."));
    }
    Ok(())
}

/// `_validate_builder_gemma_prompt`: an error when the text is empty, the scene's lyric, a loop, or an unfilled template.
pub fn validate_builder_prompt(text: &str, label: &str, payload: Option<&Obj>) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err(format!("Gemma returned an empty {label} prompt."));
    }
    let empty = Obj::new();
    if looks_like_source_lyric_echo(text, payload.unwrap_or(&empty)) {
        return Err(format!("Gemma returned the scene lyrics instead of a usable {label} image prompt. Try again, add a short visual scene beat, or reduce Lyric Story Strength."));
    }
    if looks_like_repeat_failure(text) {
        let hint = if label.to_lowercase() != "flux/klein" { "Try again, shorten the notes, or turn off image reference." } else { "Try again or shorten the notes." };
        return Err(format!("Gemma returned repeated/thought text for the {label} prompt. {hint}"));
    }
    let label_key = label.trim().to_lowercase().replace(' ', "_");
    let id_lora = matches!(label_key.as_str(), "id-lora" | "id_lora" | "id-lora_i2v");
    if looks_like_unfilled_template(text) && !(id_lora && looks_like_id_lora_script_prompt(text)) {
        return Err(format!("Gemma returned an unfilled template for the {label} prompt. Try again or add more specific scene/motion notes."));
    }
    Ok(())
}

/// A number from a payload value, for the checks that read loose settings.
pub fn number(value: Option<&Value>, default: f64) -> f64 {
    value.filter(|v| truthy(v)).and_then(to_float).unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn json_answers_give_their_prompt_text() {
        assert_eq!(extract_prompt_text("```json\n{\"prompt\": \" A red door. \"}\n```", None), "A red door.");
        assert_eq!(extract_prompt_text("Here you go: {\"scenes\": [{\"scene_number\": 1, \"image_prompt\": \"one\"}, {\"scene_number\": 2, \"image_prompt\": \"two\"}]} Thanks", Some(&json!("Scene 2"))), "two");
        assert_eq!(extract_prompt_text("plain words", None), "plain words");
        assert_eq!(extract_prompt_text("", None), "");
        assert_eq!(extract_prompt_text("{\"other\": {\"t2i_prompt\": \"deep\"}}", None), "deep");
        assert_eq!(extract_prompt_text("{\"nothing\": 1}", None), "{\"nothing\": 1}");
    }

    #[test]
    fn a_prompt_that_repeats_the_lyric_is_refused() {
        let payload = json!({"lyric_text": "You want me waiting in the credits"}).as_object().cloned().unwrap();
        assert!(looks_like_source_lyric_echo("you want me waiting in the credits", &payload));
        assert!(!looks_like_source_lyric_echo("A cinematic shot of her waiting in the credits room", &payload));
        assert!(!looks_like_source_lyric_echo("short", &payload));
        assert_eq!(validate_builder_prompt("", "T2I", None).unwrap_err(), "Gemma returned an empty T2I prompt.");
        assert!(validate_builder_prompt("you want me waiting in the credits", "T2I", Some(&payload)).unwrap_err().starts_with("Gemma returned the scene lyrics instead of a usable T2I image prompt."));
        assert!(validate_builder_prompt("A quiet street at dusk with warm lamps.", "T2I", Some(&payload)).is_ok());
        assert!(looks_like_id_lora_script_prompt("[Visual] a [Speech] b [Sounds] c"));
    }
}
