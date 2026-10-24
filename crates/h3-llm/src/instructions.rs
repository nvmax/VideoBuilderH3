//! The Builder's default LLM instructions. The texts are the pack's (`llm/prompts/*.py`), copied by
//! `tools/export_instructions.py` into `data/instructions.json`.

use std::sync::OnceLock;

use serde_json::Value;

const DATA: &str = include_str!("../data/instructions.json");

fn data() -> &'static Value {
    static PARSED: OnceLock<Value> = OnceLock::new();
    PARSED.get_or_init(|| serde_json::from_str(DATA).unwrap_or(Value::Null))
}

fn entry(key: &str) -> Option<&'static Value> {
    data().pointer(&format!("/keys/{key}"))
}

/// `_safe_builder_instruction_key`: lower case, with anything but letters, digits and `_` turned into `_`.
pub fn safe_key(value: &str) -> Result<String, String> {
    let mut key = String::new();
    let mut pending = false;
    for c in value.trim().to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' {
            key.push(c);
            pending = false;
        } else if !pending {
            key.push('_');
            pending = true;
        }
    }
    let key = key.trim_matches('_').to_string();
    if entry(&key).is_none() {
        return Err(format!("Unknown Builder instruction key: {value}"));
    }
    Ok(key)
}

/// The default text of an instruction.
pub fn default_text(key: &str) -> Option<&'static str> {
    entry(key)?.get("default")?.as_str()
}

/// The name the Builder shows for an instruction.
pub fn label(key: &str) -> String {
    entry(key).and_then(|e| e.get("label")).and_then(Value::as_str).unwrap_or(key).to_string()
}

/// Instructions that share presets name the same group.
pub fn preset_group(key: &str) -> String {
    entry(key).and_then(|e| e.get("preset_group")).and_then(Value::as_str).unwrap_or(key).to_string()
}

pub fn preset_group_label(key: &str) -> String {
    let group = preset_group(key);
    data().pointer(&format!("/preset_group_labels/{group}")).and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| label(&group))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_checked_and_their_texts_are_there() {
        assert_eq!(safe_key(" MiniMax-H3 Prompt Enhance ").unwrap(), "minimax_h3_prompt_enhance");
        assert_eq!(safe_key("zimage_t2i").unwrap(), "zimage_t2i");
        assert_eq!(safe_key("nope").unwrap_err(), "Unknown Builder instruction key: nope");
        assert!(default_text("minimax_h3_text_to_video").unwrap().len() > 200);
        assert_eq!(label("zimage_t2i"), "ZImage Text to Image");
        assert_eq!(preset_group("zimage_t2i"), "standard_image_t2i");
        assert_eq!(preset_group_label("flux_klein_t2i"), "Reference/Image Edit T2I");
        assert_eq!(preset_group("minimax_h3_text_to_video"), "minimax_h3_text_to_video");
        assert_eq!(preset_group_label("minimax_h3_text_to_video"), "MiniMax H3 Text to Video");
    }
}
