//! MiniMax H3 two pass drafts: render pass 1 alone, keep its latent, and later finish the clip from that latent. Port of the
//! graph work in `runner/minimax_drafts.py`. The folder and file work around it is in `h3-builder`.

use std::collections::HashSet;

use h3_core::pyval::{text_or_empty, Obj};
use serde_json::{json, Value};

use crate::api::link;
use crate::env::{GraphError, Result};

/// The sampler whose output the draft keeps, and the node that writes the clip.
const PASS_ONE_SAMPLER: &str = "125";
const VIDEO_OUTPUT: &str = "142";
const DIFFUSION_LOADERS: [(&str, &str); 2] = [("DiffusionModelLoaderKJ", "model_name"), ("UNETLoader", "unet_name")];

fn class_type(node: &Value) -> &str {
    node.get("class_type").and_then(Value::as_str).unwrap_or("")
}

fn prompt_mut(result: &mut Obj) -> Result<&mut Obj> {
    result.get_mut("prompt").and_then(Value::as_object_mut).ok_or_else(|| GraphError::new("The workflow has no graph."))
}

fn prompt_of(result: &Obj) -> Result<&Obj> {
    result.get("prompt").and_then(Value::as_object).ok_or_else(|| GraphError::new("The workflow has no graph."))
}

/// `_remove_other_video_outputs`: only the main video output stays.
fn remove_other_video_outputs(prompt: &mut Obj) {
    let others: Vec<String> = prompt.iter().filter(|(key, node)| key.as_str() != VIDEO_OUTPUT && class_type(node) == "VHS_VideoCombine").map(|(key, _)| key.clone()).collect();
    for key in others {
        prompt.shift_remove(&key);
    }
}

/// The node a link points at: a list of a node id (text) and an output number.
fn link_target(value: &Value) -> Option<&str> {
    match value.as_array()?.as_slice() {
        [Value::String(id), Value::Number(index)] if index.is_i64() || index.is_u64() => Some(id),
        _ => None,
    }
}

/// `_prune_draft_graph`: keep the main output, the latent savers, and everything they read from.
pub fn prune(prompt: &Obj) -> Obj {
    let mut roots = vec![VIDEO_OUTPUT.to_string()];
    roots.extend(prompt.iter().filter(|(_, node)| class_type(node) == "VRGDG_MiniMaxH3SaveLatent").map(|(key, _)| key.clone()));
    let mut keep: HashSet<String> = HashSet::new();
    let mut pending = roots;
    while let Some(key) = pending.pop() {
        if !prompt.contains_key(&key) || !keep.insert(key.clone()) {
            continue;
        }
        for value in prompt[&key].get("inputs").and_then(Value::as_object).into_iter().flat_map(|i| i.values()) {
            if let Some(id) = link_target(value) {
                pending.push(id.to_string());
            }
        }
    }
    prompt.iter().filter(|(key, _)| keep.contains(*key)).map(|(key, node)| (key.clone(), node.clone())).collect()
}

fn add_to_prefix(prompt: &mut Obj, suffix: &str) {
    if let Some(inputs) = prompt.get_mut(VIDEO_OUTPUT).and_then(|n| n.get_mut("inputs")).and_then(Value::as_object_mut) {
        let prefix = text_or_empty(inputs.get("filename_prefix"));
        inputs.insert("filename_prefix".into(), json!(format!("{prefix}{suffix}")));
    }
}

/// Whether a two pass graph can be split: it has the pass 1 sampler and says which two pass builder made it.
pub fn is_two_pass(result: &Obj) -> bool {
    result.get("prompt").and_then(Value::as_object).is_some_and(|p| p.contains_key(PASS_ONE_SAMPLER)) && (result.contains_key("two_pass") || result.contains_key("advanced_two_pass"))
}

/// The steps of the two passes, for the saved draft.
pub fn draft_steps(result: &Obj) -> Result<Value> {
    let prompt = prompt_of(result)?;
    let pass1 = prompt.get("124").and_then(|n| n.get("inputs")).and_then(|i| i.get("steps")).cloned().ok_or_else(|| GraphError::new("'steps'"))?;
    let pass2 = prompt.get("190").and_then(|n| n.get("inputs")).and_then(|i| i.get("value")).cloned().ok_or_else(|| GraphError::new("'value'"))?;
    Ok(json!({"pass1": pass1, "pass2": pass2}))
}

/// The graph that renders only pass 1 and saves its latent to `checkpoint`. `saved` is the full result with the draft fields.
pub fn draft_graph(saved: &Obj, checkpoint: &str) -> Result<Obj> {
    let mut draft = saved.clone();
    let prompt = prompt_mut(&mut draft)?;
    remove_other_video_outputs(prompt);
    prompt.insert("draft_save".into(), json!({"class_type": "VRGDG_MiniMaxH3SaveDraft", "inputs": {"latent": link(PASS_ONE_SAMPLER, 0), "checkpoint_path": checkpoint}}));
    prompt.insert("draft_decode".into(), json!({"class_type": "VAEDecode", "inputs": {"samples": link("draft_save", 0), "vae": link("119", 0)}}));
    if let Some(inputs) = prompt.get_mut(VIDEO_OUTPUT).and_then(|n| n.get_mut("inputs")).and_then(Value::as_object_mut) {
        inputs.insert("images".into(), link("draft_decode", 0));
    }
    add_to_prefix(prompt, "_draft_stage1");
    for node in prompt.values_mut() {
        if class_type(node) == "VRGDG_MiniMaxH3SaveLatent" {
            if let Some(inputs) = node.get_mut("inputs").and_then(Value::as_object_mut) {
                inputs.insert("latent".into(), link(PASS_ONE_SAMPLER, 0));
            }
        }
    }
    let pruned = prune(prompt);
    draft.insert("prompt".into(), Value::Object(pruned));
    Ok(draft)
}

/// The diffusion model files a saved draft graph loads, sorted.
pub fn draft_diffusion_models(prompt: &Obj) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for node in prompt.values() {
        for (class, key) in DIFFUSION_LOADERS {
            let name = text_or_empty(node.get("inputs").and_then(|i| i.get(key)));
            if class_type(node) == class && !name.is_empty() {
                found.push(base_name(&name));
            }
        }
    }
    found.sort();
    found.dedup();
    found
}

/// `os.path.basename`: the part after the last `/` or `\`.
pub fn base_name(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or("").to_string()
}

/// The graph that finishes a draft: pass 1 is replaced by the saved latent, everything after it runs as before.
pub fn finalize_graph(saved: &Obj, checkpoint: &str) -> Result<Obj> {
    let mut result = saved.clone();
    let prompt = prompt_mut(&mut result)?;
    remove_other_video_outputs(prompt);
    // The sampler itself is replaced, so every pass 2 consumer reuses exactly the saved output.
    prompt.insert(PASS_ONE_SAMPLER.into(), json!({"class_type": "VRGDG_MiniMaxH3LoadDraft", "inputs": {"checkpoint_path": checkpoint}}));
    add_to_prefix(prompt, "_finalized");
    let pruned = prune(prompt);
    result.insert("prompt".into(), Value::Object(pruned));
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph() -> Obj {
        json!({
            "119": {"class_type": "VAELoader", "inputs": {}},
            "124": {"class_type": "KSampler", "inputs": {"steps": 4, "model": ["10", 0]}},
            "125": {"class_type": "SamplerCustom", "inputs": {"latent": ["124", 0]}},
            "126": {"class_type": "Pass2", "inputs": {"latent": ["125", 0]}},
            "10": {"class_type": "UNETLoader", "inputs": {"unet_name": "models\\big.gguf"}},
            "11": {"class_type": "DiffusionModelLoaderKJ", "inputs": {"model_name": "a/small.safetensors"}},
            "190": {"class_type": "Primitive", "inputs": {"value": 3}},
            "141": {"class_type": "VHS_VideoCombine", "inputs": {"images": ["126", 0], "filename_prefix": "stage"}},
            "142": {"class_type": "VHS_VideoCombine", "inputs": {"images": ["126", 0], "filename_prefix": "scene"}},
            "200": {"class_type": "VRGDG_MiniMaxH3SaveLatent", "inputs": {"latent": ["126", 0]}},
            "300": {"class_type": "Unused", "inputs": {}}
        })
        .as_object()
        .cloned()
        .unwrap()
    }

    fn saved() -> Obj {
        json!({"prompt": Value::Object(graph()), "two_pass": true}).as_object().cloned().unwrap()
    }

    #[test]
    fn a_draft_renders_pass_one_and_keeps_its_latent() {
        let draft = draft_graph(&saved(), "C:/p/pass1.pt").unwrap();
        let prompt = draft["prompt"].as_object().unwrap();
        assert!(!prompt.contains_key("141") && !prompt.contains_key("300") && !prompt.contains_key("126"));
        assert_eq!(prompt["draft_save"]["inputs"]["checkpoint_path"], "C:/p/pass1.pt");
        assert_eq!(prompt["draft_decode"]["inputs"]["vae"], json!(["119", 0]));
        assert_eq!(prompt["142"]["inputs"]["images"], json!(["draft_decode", 0]));
        assert_eq!(prompt["142"]["inputs"]["filename_prefix"], "scene_draft_stage1");
        assert_eq!(prompt["200"]["inputs"]["latent"], json!(["125", 0]));
        let order: Vec<&String> = prompt.keys().collect();
        assert_eq!(order.last().map(|s| s.as_str()), Some("draft_decode"));
    }

    #[test]
    fn finishing_a_draft_replaces_the_pass_one_sampler() {
        let done = finalize_graph(&saved(), "C:/p/pass1.pt").unwrap();
        let prompt = done["prompt"].as_object().unwrap();
        assert_eq!(prompt["125"], json!({"class_type": "VRGDG_MiniMaxH3LoadDraft", "inputs": {"checkpoint_path": "C:/p/pass1.pt"}}));
        assert!(!prompt.contains_key("124") && !prompt.contains_key("141") && prompt.contains_key("126"));
        assert_eq!(prompt["142"]["inputs"]["filename_prefix"], "scene_finalized");
    }

    #[test]
    fn only_two_pass_graphs_can_be_split_and_their_models_are_listed() {
        assert!(is_two_pass(&saved()));
        let mut plain = saved();
        plain.remove("two_pass");
        assert!(!is_two_pass(&plain));
        assert_eq!(draft_diffusion_models(&graph()), vec!["big.gguf", "small.safetensors"]);
        assert_eq!(draft_steps(&saved()).unwrap(), json!({"pass1": 4, "pass2": 3}));
    }
}
