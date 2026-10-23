//! Import a workflow and guess its bindings.

use h3_comfy::models::combo_choices;
use serde_json::Value;

use crate::preset::{Binding, BindingKind};
use crate::ImageError;

/// Read a workflow saved with ComfyUI's "Save (API Format)". The editor's own format (nodes and links) is refused with a
/// message that says how to export the right one.
pub fn parse_api_workflow(text: &str) -> Result<Value, ImageError> {
    let value: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| ImageError::Preset(format!("The workflow is not valid JSON: {e}")))?;
    let Value::Object(mut map) = value else {
        return Err(ImageError::Preset("The workflow must be a JSON object.".into()));
    };
    if map.get("nodes").is_some_and(Value::is_array) && map.get("links").is_some() {
        return Err(ImageError::Preset(
            "This is a workflow saved from the editor. In ComfyUI use Workflow > Export (API) and import that file.".into(),
        ));
    }
    // A request body: {"prompt": {...}}.
    if let Some(Value::Object(inner)) = map.get("prompt").cloned() {
        if inner.values().all(Value::is_object) {
            map = inner;
        }
    }
    if map.is_empty() {
        return Err(ImageError::Preset("The workflow has no nodes.".into()));
    }
    for (id, node) in &map {
        let class = node.get("class_type").and_then(Value::as_str);
        if class.is_none() || !node.get("inputs").is_some_and(Value::is_object) {
            return Err(ImageError::Preset(format!("Node {id} is not an API format node (it needs class_type and inputs).")));
        }
    }
    Ok(Value::Object(map))
}

fn literal<'a>(node: &'a Value, input: &str) -> Option<&'a Value> {
    node.get("inputs").and_then(|i| i.get(input)).filter(|v| !v.is_array() && !v.is_null())
}

fn class(node: &Value) -> &str {
    node.get("class_type").and_then(Value::as_str).unwrap_or("")
}

fn sorted_ids(workflow: &Value) -> Vec<String> {
    let mut ids: Vec<String> = workflow.as_object().map(|m| m.keys().cloned().collect()).unwrap_or_default();
    ids.sort_by(|a, b| match (a.parse::<u64>(), b.parse::<u64>()) {
        (Ok(x), Ok(y)) => x.cmp(&y),
        _ => a.cmp(b),
    });
    ids
}

/// The text encoder a conditioning input comes from, following a few pass through nodes.
fn text_node(workflow: &Value, id: &str, depth: u8) -> Option<String> {
    let node = workflow.get(id)?;
    if class(node) == "CLIPTextEncode" && literal(node, "text").is_some_and(Value::is_string) {
        return Some(id.to_string());
    }
    if depth == 0 {
        return None;
    }
    for input in ["conditioning", "conditioning_1", "positive", "negative"] {
        if let Some(link) = node.get("inputs").and_then(|i| i.get(input)).and_then(Value::as_array) {
            if let Some(next) = link.first().and_then(Value::as_str) {
                if let Some(found) = text_node(workflow, next, depth - 1) {
                    return Some(found);
                }
            }
        }
    }
    None
}

fn linked_source(workflow: &Value, input: &str) -> Option<String> {
    for id in sorted_ids(workflow) {
        let link = workflow[&id].get("inputs").and_then(|i| i.get(input)).and_then(Value::as_array);
        if let Some(source) = link.and_then(|l| l.first()).and_then(Value::as_str) {
            if let Some(found) = text_node(workflow, source, 3) {
                return Some(found);
            }
        }
    }
    None
}

/// Bindings a new workflow most likely wants, and the nodes that produce its images. The user can change them all.
pub fn suggest_bindings(workflow: &Value) -> (Vec<Binding>, Vec<String>) {
    let ids = sorted_ids(workflow);
    let mut bindings: Vec<Binding> = Vec::new();

    // Models and files the workflow loads.
    const LOADERS: [(&str, &[&str]); 7] = [
        ("UNETLoader", &["unet_name"]),
        ("CheckpointLoaderSimple", &["ckpt_name"]),
        ("CLIPLoader", &["clip_name"]),
        ("DualCLIPLoader", &["clip_name1", "clip_name2"]),
        ("VAELoader", &["vae_name"]),
        ("LoraLoader", &["lora_name"]),
        ("LoraLoaderModelOnly", &["lora_name"]),
    ];
    for (loader, inputs) in LOADERS {
        for id in &ids {
            let node = &workflow[id];
            if class(node) != loader {
                continue;
            }
            for input in inputs {
                let Some(current) = literal(node, input).and_then(Value::as_str) else { continue };
                let taken = bindings.iter().any(|b| b.key == *input);
                let key = if taken { format!("{input}_{id}") } else { (*input).to_string() };
                let mut binding = Binding::new(&key, &key.replace('_', " "), BindingKind::Choice, &[(id, input)]);
                binding.default = Value::String(current.to_string());
                bindings.push(binding);
            }
        }
    }

    let positive = linked_source(workflow, "positive");
    let negative = linked_source(workflow, "negative");
    let first_text = ids.iter().find(|id| text_node(workflow, id, 0).is_some()).cloned();
    if let Some(id) = positive.clone().or(first_text) {
        bindings.push(Binding::new("prompt", "Prompt", BindingKind::Text, &[(&id, "text")]));
        if let Some(neg) = negative.filter(|n| *n != id) {
            let mut binding = Binding::new("negative_prompt", "Negative prompt", BindingKind::Text, &[(&neg, "text")]);
            binding.default = workflow[&neg]["inputs"]["text"].clone();
            bindings.push(binding);
        }
    }

    // Image size and batch.
    let sizes: Vec<&String> = ids
        .iter()
        .filter(|id| {
            let node = &workflow[id.as_str()];
            class(node).contains("Empty") && literal(node, "width").is_some() && literal(node, "height").is_some()
        })
        .collect();
    if let Some(first) = sizes.first() {
        let node = &workflow[first.as_str()];
        let width: Vec<(&str, &str)> = sizes.iter().map(|id| (id.as_str(), "width")).collect();
        let height: Vec<(&str, &str)> = sizes.iter().map(|id| (id.as_str(), "height")).collect();
        let mut w = Binding::new("width", "Width", BindingKind::Int, &width).range(64.0, 8192.0);
        w.default = literal(node, "width").cloned().unwrap_or_default();
        let mut h = Binding::new("height", "Height", BindingKind::Int, &height).range(64.0, 8192.0);
        h.default = literal(node, "height").cloned().unwrap_or_default();
        bindings.push(w);
        bindings.push(h);
        if literal(node, "batch_size").is_some() {
            let batch: Vec<(&str, &str)> = sizes.iter().filter(|id| literal(&workflow[id.as_str()], "batch_size").is_some()).map(|id| (id.as_str(), "batch_size")).collect();
            bindings.push(Binding::new("batch_size", "Batch size", BindingKind::Int, &batch).range(1.0, 16.0).default(literal(node, "batch_size").cloned().unwrap_or_default()));
        }
    }

    // Seeds, steps and guidance.
    let collect = |names: &[&str]| -> Vec<(String, String)> {
        let mut found = Vec::new();
        for id in &ids {
            for name in names {
                if literal(&workflow[id], name).is_some_and(Value::is_number) {
                    found.push((id.clone(), (*name).to_string()));
                }
            }
        }
        found
    };
    let seeds = collect(&["seed", "noise_seed"]);
    if !seeds.is_empty() {
        let t: Vec<(&str, &str)> = seeds.iter().map(|(n, i)| (n.as_str(), i.as_str())).collect();
        bindings.push(Binding::new("seed", "Seed", BindingKind::Seed, &t).default(Value::from(-1)));
    }
    for (key, label, name, low, high, kind) in [("steps", "Steps", "steps", 1.0, 200.0, BindingKind::Int), ("cfg", "CFG", "cfg", 0.0, 30.0, BindingKind::Float)] {
        let found = collect(&[name]);
        if let Some((node, input)) = found.first() {
            let t: Vec<(&str, &str)> = found.iter().map(|(n, i)| (n.as_str(), i.as_str())).collect();
            bindings.push(Binding::new(key, label, kind, &t).range(low, high).default(literal(&workflow[node], input).cloned().unwrap_or_default()));
        }
    }

    // Reference and starting images.
    let mut n = 0;
    for id in &ids {
        if class(&workflow[id]) == "LoadImage" && literal(&workflow[id], "image").is_some() {
            n += 1;
            let key = if n == 1 { "image".to_string() } else { format!("image_{n}") };
            bindings.push(Binding::new(&key, &format!("Image {n}"), BindingKind::Image, &[(id, "image")]));
        }
    }

    let outputs: Vec<String> = ids.iter().filter(|id| matches!(class(&workflow[id.as_str()]), "SaveImage" | "PreviewImage")).cloned().collect();
    (bindings, outputs)
}

/// The names ComfyUI offers for a binding's first target, for a dropdown. Empty when it offers none.
pub fn choices_for(workflow: &Value, binding: &Binding, object_info: &Value) -> Vec<String> {
    let Some(target) = binding.targets.first() else { return vec![] };
    let Some(class_type) = workflow.get(&target.node).and_then(|n| n.get("class_type")).and_then(Value::as_str) else { return vec![] };
    combo_choices(object_info, class_type, &target.input)
}
