//! Constants of the pack's LLM prompt modules (`llm/prompts/*.py`): instruction texts, schemas and tables. They are copied
//! by `tools/export_prompt_constants.py` into `data/prompt_constants.json`, so the texts are the pack's byte for byte.

use std::sync::OnceLock;

use serde_json::Value;

const DATA: &str = include_str!("../data/prompt_constants.json");

fn data() -> &'static Value {
    static PARSED: OnceLock<Value> = OnceLock::new();
    PARSED.get_or_init(|| serde_json::from_str(DATA).unwrap_or(Value::Null))
}

/// A constant of a prompt module (`"image"`, `"_VISUAL_T2I_INSTRUCTIONS"`). `Null` when there is none.
pub fn constant(module: &str, name: &str) -> &'static Value {
    static NULL: Value = Value::Null;
    data().get(module).and_then(|m| m.get(name)).unwrap_or(&NULL)
}

/// A text constant. Empty when there is none.
pub fn text(module: &str, name: &str) -> &'static str {
    constant(module, name).as_str().unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pack_texts_are_there() {
        assert!(text("image", "_VISUAL_T2I_INSTRUCTIONS").len() > 100);
        assert!(text("image", "_FLUX_KLEIN_T2I_INSTRUCTIONS").len() > 100);
        assert_eq!(text("image", "_NOPE"), "");
        assert!(constant("location_scout", "_LM_LOCATION_SCOUT_SCHEMA").is_object());
    }
}
