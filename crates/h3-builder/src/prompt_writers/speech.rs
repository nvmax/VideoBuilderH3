//! The two LLM helpers of the Speaking mode: a voice description for ElevenLabs Voice Design, and a character's dialogue with audio tags. Neither
//! calls ElevenLabs or saves anything. Port of `llm/voice_design.py` and `llm/scene_dialogue.py`.

use h3_core::pyval::{text_or_empty, Obj};
use h3_core::speaking::{required_text, spoken_words, validate_dialogue};
use h3_llm::prompts::text as constant;
use serde_json::{json, Value};

use super::{Llm, TextCall};
use crate::instructions::effective_instruction;

type Reply = Result<Value, String>;

fn clipped(payload: &Obj, key: &str, limit: usize) -> String {
    text_or_empty(payload.get(key)).chars().take(limit).collect()
}

fn run_fields(info: &Obj) -> (Value, Value) {
    (info.get("runner").cloned().unwrap_or_else(|| json!("")), info.get("used_model").cloned().unwrap_or_else(|| json!("")))
}

/// `generate_voice_description`: an editable voice description from the user's brief and the character.
pub async fn generate_voice_description(llm: &dyn Llm, payload: &Obj) -> Reply {
    let brief = required_text(payload.get("user_input").unwrap_or(&Value::Null), "Voice brief", 1, 4000)?;
    let character = clipped(payload, "character_description", 4000);
    let name = clipped(payload, "character_name", 256);
    let instruction = format!(
        "Write one voice description for ElevenLabs Voice Design using the user's brief. Return only a single paragraph of 20 to 1000 characters, preferably 300\u{2013}700. Describe audible qualities: perceived age and vocal range, accent when specified, timbre, texture, resonance, rhythm, cadence, articulation and delivery. Preserve the user's intent. Use character context only to support the brief; do not include appearance, clothing, locations, stage directions or dialogue. Do not add a name, title, markdown, quotation marks, explanations or multiple options.\n\nUser voice brief:\n{brief}\n\nCharacter name: {name}\nCharacter context:\n{character}"
    );
    let (reply, info) = llm.text(TextCall::new(instruction, 0.5, 0.95, 500, "Voice Description").paragraphs()).await?;
    let description = required_text(&json!(reply), "LLM voice description", 20, 1000)?;
    let (runner, used_model) = run_fields(&info);
    Ok(json!({"voice_description": description, "runner": runner, "used_model": used_model}))
}

/// `craft_dialogue`: the dialogue with sparse audio tags. Unless rewriting is allowed, the spoken words must stay as they were.
pub async fn craft_dialogue(llm: &dyn Llm, payload: &Obj) -> Reply {
    let draft = validate_dialogue(payload.get("dialogue").unwrap_or(&Value::Null), true)?;
    let instructions = effective_instruction(payload, "elevenlabs_dialogue", constant("speech", "ELEVENLABS_DIALOGUE_INSTRUCTIONS"));
    let allow_rewrite = draft["allow_rewrite"] == Value::Bool(true);
    let limit = if draft["model_id"] == json!("eleven_v3") { 5000 } else { 10000 };
    let prompt = format!(
        "{instructions}\n\nRewrite allowed: {}\nMaximum output characters: {limit}\nCharacter: {}\nCharacter context: {}\nDelivery direction: {}\nDialogue:\n{}",
        if allow_rewrite { "True" } else { "False" },
        clipped(payload, "character_name", 256),
        clipped(payload, "character_description", 4000),
        text_or_empty(draft.get("delivery")),
        text_or_empty(draft.get("text")),
    );
    let (reply, info) = llm.text(TextCall::new(prompt, 0.5, 0.95, 1800, "Dialogue").paragraphs()).await?;
    let mut revised = draft.clone();
    revised["text"] = json!(reply);
    let result = validate_dialogue(&revised, true)?;
    if !allow_rewrite && spoken_words(&text_or_empty(result.get("text"))) != spoken_words(&text_or_empty(draft.get("text"))) {
        return Err("The LLM changed the spoken words. Try again, add tags manually, or enable Allow rewriting.".into());
    }
    let (runner, used_model) = run_fields(&info);
    Ok(json!({"dialogue": result, "runner": runner, "used_model": used_model}))
}
