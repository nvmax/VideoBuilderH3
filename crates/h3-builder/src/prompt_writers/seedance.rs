//! Converting a saved prompt to a Seedance 2.5 prompt with the project's LLM. Port of `convert_seedance_prompt` in
//! `llm/seedance_conversion.py`.
//!
//! The LLM never rewrites the prompt. It sees the immutable blocks and answers with a layout plan, which
//! [`h3_core::seedance::assemble_seedance_prompt`] checks before it lays the blocks out under the plan's headings.

use h3_core::pyval::Obj;
use h3_core::seedance::{assemble_seedance_prompt, plan_schema, prepare_seedance_blocks};
use h3_llm::prompts::text as constant;
use serde_json::{json, Value};

use super::{Llm, TextCall};
use crate::instructions::effective_instruction;

/// `convert_seedance_prompt`: the converted prompt of the scene named in `payload`, from the backup saved before the conversion.
pub async fn convert_prompt(llm: &dyn Llm, payload: &Obj) -> Result<Value, String> {
    let scene = crate::seedance::load_backup_scene(payload)?;
    let blocks = prepare_seedance_blocks(&scene)?;
    let instructions = effective_instruction(payload, "seedance25_prompt_conversion", constant("seedance", "SEEDANCE_CONVERSION_INSTRUCTIONS"));
    let listing = h3_core::pyjson::dumps_inline(&serde_json::to_value(&blocks).map_err(|e| e.to_string())?);
    let mut call = TextCall::new(format!("{instructions}\n\nIMMUTABLE SOURCE BLOCKS:\n{listing}"), 0.1, 0.8, 2500, "Seedance format conversion").paragraphs();
    call.json_schema = Some(plan_schema());
    let (reply, info) = llm.text(call).await?;
    let plan = h3_llm::locations::extract_json_object(&reply)?;
    let prompt = assemble_seedance_prompt(&blocks, &plan)?;
    let used = |key: &str, default: &str| info.get(key).cloned().unwrap_or_else(|| json!(default));
    Ok(json!({
        "prompt": prompt,
        "characters": prompt.chars().count(),
        "preserved_blocks": blocks.len(),
        "runner": used("runner", "builtin"),
        "used_model": used("used_model", ""),
    }))
}
