//! The Storyboard's text-to-image prompt for a scene card. Port of `_build_storyboard_image_prompt` in `storyboard/scene_prompts.py`.

use h3_core::pyjson::dumps;
use h3_core::pyval::{text_or_empty, truthy, Obj};
use h3_core::story::helpers::first_truthy;
use h3_core::story::mode::text;
use h3_core::story::scene_text::{ensure_reference_opening, fix_single_subject_prompt_pronouns, image_mode_uses_reference_opening, selected_scene};
use h3_llm::beat_prompts::{flf_endpoint_instruction, image_world_style_contract};
use h3_llm::prompt_text::extract_prompt_text;
use h3_llm::prompts::text as constant;
use serde_json::{json, Value};

use crate::instructions::effective_instruction;
use crate::prompt_writers::{number, whole, Llm, TextCall};

/// `POST gemma_image_prompt`: the still image prompt of the selected scene of a scene card bundle.
pub async fn build_storyboard_image_prompt(llm: &dyn Llm, payload: &Obj) -> Result<Value, String> {
    let bundle = first_truthy(payload, &["storyboard_payload", "scene_bundle", "gpt_payload"]).filter(|v| v.is_object()).cloned();
    let Some(bundle) = bundle else { return Err("Storyboard scene-card payload is missing.".into()) };
    if !matches!(bundle.get("scenes"), Some(Value::Array(scenes)) if !scenes.is_empty()) {
        return Err("Storyboard scene-card payload has no scenes.".into());
    }
    let mut instruction_text = constant("storyboard", "_STORYBOARD_T2I_GEMMA_INSTRUCTIONS").to_string();
    let key = first_truthy(payload, &["builder_instruction_key", "instruction_key"]).map(|v| text_or_empty(Some(v))).unwrap_or_default().trim().to_string();
    if !key.is_empty() {
        instruction_text = effective_instruction(payload, &key, constant("image", "_STANDARD_IMAGE_T2I_INSTRUCTIONS"));
    }
    let selected = selected_scene(&bundle)?;
    instruction_text.push_str("\n\n");
    instruction_text.push_str(constant("storyboard", "_STORYBOARD_SCENE_CARD_CONTEXT_INSTRUCTIONS"));
    let style = first_truthy(payload, &["image_world_style"]).map(|v| text_or_empty(Some(v))).unwrap_or_else(|| "natural".into()).trim().to_lowercase();
    let custom = text(payload.get("image_custom_style_direction"), 3000);
    instruction_text.push_str(&image_world_style_contract(&style, &custom));
    let target = text_or_empty(payload.get("flf_image_target").filter(|v| truthy(v))).trim().to_lowercase();
    if target == "start" || target == "end" {
        let layer = selected.get("story_layer").and_then(Value::as_object).cloned().unwrap_or_default();
        let field = |key: &str| {
            let own = layer.get(key).filter(|v| truthy(v)).or_else(|| selected.get(key).filter(|v| truthy(v)));
            text(own, 1800)
        };
        let (start_state, transformation, end_state, carry_forward) = (field("flf_start_state"), field("flf_transformation"), field("flf_end_state"), field("flf_carry_forward"));
        let target_state = if target == "start" { start_state } else { end_state };
        instruction_text.push_str(&flf_endpoint_instruction(&target, &target_state, &transformation, &carry_forward));
    }
    let instruction = format!("{instruction_text}\n\nScene-card JSON:\n{}", dumps(&bundle));
    let temperature = number(payload, "temperature", 0.35)?;
    let top_p = number(payload, "top_p", 0.90)?;
    let tokens = whole(payload, "max_new_tokens", 1200)?;
    let (reply, info) = llm.text(TextCall::new(instruction, temperature, top_p, tokens, "Storyboard T2I Gemma").paragraphs()).await?;
    let extracted = extract_prompt_text(&reply, bundle.get("selected_scene_number"));
    let mut prompt = text(Some(&json!(fix_single_subject_prompt_pronouns(&extracted, &bundle)?)), 12000);
    if image_mode_uses_reference_opening(&bundle) {
        prompt = ensure_reference_opening(&prompt, &selected);
    }
    if prompt.is_empty() {
        return Err("Gemma returned an empty Storyboard image prompt.".into());
    }
    let mut out = Obj::new();
    out.insert("prompt".into(), json!(prompt));
    out.insert("runner".into(), info.get("runner").cloned().unwrap_or_else(|| json!("builtin")));
    out.insert("used_model".into(), info.get("used_model").cloned().unwrap_or_else(|| json!("")));
    out.insert("unloaded".into(), info.get("unloaded").cloned().unwrap_or_else(|| json!(true)));
    Ok(Value::Object(out))
}
