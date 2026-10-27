//! The RefMod pipeline for the MiniMax H3 graph builders. Port of `runner/minimax_refmod.py`.
//!
//! When a render says `pipeline == "refmod"` the built graph is rewired: the reference media node and the reference
//! inputs of the Reference to Video node are removed, and the guiders read their conditioning from *Text Encode with
//! RefMods*, which shows every saved RefMod to the text encoder with a numbered label (`<Video 1>`, `<Picture 1>`,
//! `<Audio 1>`). The scene audio becomes an audio RefMod so `<Audio 1>` still exists.

use h3_core::pyval::{py_str, text_or_empty, to_float, truthy, Obj};
use h3_core::refmod::{assign_labels, token_report};
use serde_json::{json, Value};

use crate::env::{GraphError, Result};

pub const LOADER_SLOTS: usize = 8;
pub const MAX_REFERENCES: usize = h3_core::scene::MAX_SCENE_REFERENCES;
const REFERENCE_INPUT_PREFIXES: [&str; 4] = ["ref_images.", "ref_videos.", "ref_video_audios.", "ref_audios."];

/// What the RefMod library knows about one saved RefMod.
#[derive(Debug, Clone, PartialEq)]
pub struct RefmodEntry {
    pub name: String,
    pub kind: String,
    pub tokens: i64,
}

pub fn refmod_requested(payload: &Obj) -> bool {
    text_or_empty(payload.get("pipeline")).trim().to_lowercase() == "refmod"
}

fn new_id(prompt: &Obj, base: usize) -> String {
    let mut number = base;
    while prompt.contains_key(&number.to_string()) {
        number += 1;
    }
    number.to_string()
}

struct Checked {
    name: String,
    strength: f64,
    kind: String,
    tokens: i64,
    display_name: String,
    category: String,
    prompt_label: String,
}

fn validate_references(payload: &Obj, find: &dyn Fn(&str) -> Option<RefmodEntry>) -> Result<Vec<Checked>> {
    let Some(references) = payload.get("refmod_references").and_then(Value::as_array).filter(|a| !a.is_empty()) else {
        return Err(GraphError::new("The RefMod pipeline needs at least one RefMod reference for this scene."));
    };
    if references.len() > MAX_REFERENCES {
        return Err(GraphError::new(format!("A scene can use at most {MAX_REFERENCES} RefMods ({} were given).", references.len())));
    }
    let mut checked = Vec::new();
    for (index, reference) in references.iter().enumerate() {
        let position = index + 1;
        let Some(reference) = reference.as_object() else { return Err(GraphError::new(format!("RefMod reference {position} is not an object."))) };
        let name = Some(py_str(reference.get("name").filter(|v| truthy(v)))).filter(|n| n != "None").map(|n| n.trim().to_string()).unwrap_or_default();
        let entry = find(&name).ok_or_else(|| GraphError::new(format!("RefMod '{name}' was not found in models/refmods. Pick it again in the Reference Builder or remove the card.")))?;
        let strength = match reference.get("strength") {
            None => 1.0,
            Some(v) => to_float(v).ok_or_else(|| GraphError::new(format!("RefMod '{name}' has a strength that is not a number.")))?,
        };
        if !(0.0..=1.0).contains(&strength) {
            return Err(GraphError::new(format!("RefMod '{name}' strength must be between 0 and 1.")));
        }
        let shown = reference.get("display_name").filter(|v| truthy(v)).map(|v| py_str(Some(v)));
        checked.push(Checked {
            name: name.clone(),
            strength,
            kind: entry.kind,
            tokens: entry.tokens,
            display_name: shown.unwrap_or(name),
            category: reference.get("category").filter(|v| truthy(v)).map(|v| py_str(Some(v))).unwrap_or_default(),
            prompt_label: reference.get("label").filter(|v| truthy(v)).map(|v| py_str(Some(v)).trim().to_string()).unwrap_or_default(),
        });
    }
    Ok(checked)
}

fn loader_node(slots: &[&Checked], title: &str) -> Value {
    let mut inputs = Obj::new();
    inputs.insert("show_info".into(), json!(false));
    for index in 1..=LOADER_SLOTS {
        let slot = slots.get(index - 1);
        inputs.insert(format!("mod_{index}"), json!(slot.map_or("(none)", |s| s.name.as_str())));
        inputs.insert(format!("strength_{index}"), json!(slot.map_or(1.0, |s| s.strength)));
        inputs.insert(format!("copies_{index}"), json!(1));
    }
    json!({"class_type": "MiniMaxH3RefModsLoader", "inputs": inputs, "_meta": {"title": title}})
}

fn class_of(node: &Value) -> &str {
    node.get("class_type").and_then(Value::as_str).unwrap_or("")
}

/// Swap the labels the prompt was written with for the labels the render gives, when they differ. Returns old to new.
fn relabel_prompt(prompt: &mut Obj, prompt_node: &[Value], references: &[Checked], labels: &[String]) -> Vec<(String, String)> {
    let mut mapping: Vec<(String, String)> = Vec::new();
    for (item, label) in references.iter().zip(labels) {
        if !item.prompt_label.is_empty() && !label.is_empty() && item.prompt_label != *label {
            match mapping.iter_mut().find(|(old, _)| *old == item.prompt_label) {
                Some(slot) => slot.1 = label.clone(),
                None => mapping.push((item.prompt_label.clone(), label.clone())),
            }
        }
    }
    let key = py_str(prompt_node.first());
    let Some(node) = prompt.get_mut(&key).and_then(Value::as_object_mut) else { return vec![] };
    let Some(value) = node.get("inputs").and_then(|i| i.get("value")).and_then(Value::as_str).map(str::to_string) else { return vec![] };
    if mapping.is_empty() {
        return vec![];
    }
    // Two steps, so swapping <Picture 1> and <Video 1> does not turn both into the same label.
    let mut text = value;
    for (index, (old, _)) in mapping.iter().enumerate() {
        text = text.replace(old, &format!("\u{0}refmod{index}\u{0}"));
    }
    for (index, (_, new)) in mapping.iter().enumerate() {
        text = text.replace(&format!("\u{0}refmod{index}\u{0}"), new);
    }
    if let Some(inputs) = node.get_mut("inputs").and_then(Value::as_object_mut) {
        inputs.insert("value".into(), json!(text));
    }
    mapping
}

/// Python's `repr` of `[(id, input name), ...]`.
fn dangling_repr(items: &[(String, String)]) -> String {
    format!("[{}]", items.iter().map(|(a, b)| format!("('{a}', '{b}')")).collect::<Vec<_>>().join(", "))
}

/// Rewire a built MiniMax H3 graph to the RefMod pipeline, in place. Returns the summary the pack reports.
pub fn apply_refmod_pipeline(prompt: &mut Obj, payload: &Obj, find: &dyn Fn(&str) -> Option<RefmodEntry>) -> Result<Value> {
    let references = validate_references(payload, find)?;
    let reference_nodes: Vec<String> = prompt.iter().filter(|(_, n)| class_of(n) == "MiniMaxH3ReferenceToVideo").map(|(k, _)| k.clone()).collect();
    if reference_nodes.len() != 1 {
        return Err(GraphError::new("The MiniMax H3 graph changed: the RefMod pipeline needs one Reference to Video node."));
    }
    let r2v = reference_nodes[0].clone();
    let media_nodes: Vec<String> = prompt.iter().filter(|(_, n)| class_of(n) == "VRGDG_MiniMaxH3ReferenceMediaFromPaths").map(|(k, _)| k.clone()).collect();
    let prompt_node: Vec<Value> = match prompt[&r2v]["inputs"].get("prompt") {
        Some(Value::Array(a)) if a.len() == 2 => a.clone(),
        _ => return Err(GraphError::new("The MiniMax H3 graph changed: the Reference to Video prompt is not linked to the prompt node.")),
    };

    // No images, videos or reference audio go into Reference to Video any more. It only makes the empty AV latent.
    if let Some(inputs) = prompt.get_mut(&r2v).and_then(|n| n.get_mut("inputs")).and_then(Value::as_object_mut) {
        let doomed: Vec<String> = inputs.keys().filter(|k| REFERENCE_INPUT_PREFIXES.iter().any(|p| k.starts_with(p))).cloned().collect();
        for key in doomed {
            inputs.shift_remove(&key);
        }
        inputs.insert("prompt".into(), json!(" "));
    }
    for key in &media_nodes {
        prompt.shift_remove(key);
    }

    let mut loaders: Vec<String> = Vec::new();
    for (n, chunk) in references.chunks(LOADER_SLOTS).enumerate() {
        let id = new_id(prompt, 9301 + loaders.len());
        let slots: Vec<&Checked> = chunk.iter().collect();
        prompt.insert(id.clone(), loader_node(&slots, &format!("RefMods for this scene ({})", n + 1)));
        loaders.push(id);
    }

    let mut combine_inputs = Obj::new();
    for (i, id) in loaders.iter().enumerate() {
        combine_inputs.insert(format!("mods_{}", i + 1), json!([id, 0]));
    }
    let audio_node = prompt.iter().find(|(_, n)| class_of(n) == "VHS_LoadAudio").map(|(k, _)| k.clone()).unwrap_or_default();
    let audio_vae = prompt[&r2v]["inputs"].get("audio_vae").cloned();
    let audio_mode = Some(text_or_empty(payload.get("audio_mode").filter(|v| truthy(v)))).filter(|m| !m.is_empty()).unwrap_or_else(|| "input_audio".into());
    let with_audio = !audio_node.is_empty() && audio_mode != "built_in_audio" && audio_vae.as_ref().is_some_and(truthy);
    if with_audio {
        let extract_id = new_id(prompt, 9310);
        prompt.insert(
            extract_id.clone(),
            json!({
                "class_type": "MiniMaxH3RefModAudioExtract",
                "inputs": {
                    "audio": [audio_node, 0], "audio_vae": audio_vae, "name": "scene_audio", "max_seconds": 60.0,
                    "max_tokens": 0, "budget_policy": "error", "concept_type": "singing", "description": "",
                    "subfolder": "", "save": false
                },
                "_meta": {"title": "Scene audio as an audio RefMod"}
            }),
        );
        combine_inputs.insert(format!("mods_{}", loaders.len() + 1), json!([extract_id, 0]));
    }
    let combine_id = new_id(prompt, 9320);
    prompt.insert(combine_id.clone(), json!({"class_type": "VRGDG_RefModCombine", "inputs": combine_inputs, "_meta": {"title": "RefMods and scene audio"}}));

    let clip = prompt.iter().find(|(_, n)| class_of(n) == "CLIPLoader").map(|(k, _)| k.clone()).unwrap_or_default();
    let video_vae = prompt[&r2v]["inputs"].get("vae").cloned().filter(truthy);
    let Some(video_vae) = video_vae.filter(|_| !clip.is_empty()) else {
        return Err(GraphError::new("The MiniMax H3 graph changed: the RefMod pipeline needs a text encoder and the video VAE."));
    };
    let encode_id = new_id(prompt, 9330);
    prompt.insert(
        encode_id.clone(),
        json!({
            "class_type": "MiniMaxH3RefModTextEncode",
            "inputs": {"clip": [clip, 0], "mods": [combine_id, 0], "prompt": prompt_node, "reference_fps": 24.0, "max_total_tokens": 0, "vae": video_vae},
            "_meta": {"title": "Text Encode with RefMods (labels the references)"}
        }),
    );
    let mut rewired = 0;
    let wanted = json!([r2v, 0]);
    for node in prompt.values_mut() {
        if let Some(inputs) = node.get_mut("inputs").and_then(Value::as_object_mut) {
            if inputs.get("conditioning") == Some(&wanted) {
                inputs.insert("conditioning".into(), json!([encode_id, 0]));
                rewired += 1;
            }
        }
    }
    if rewired == 0 {
        return Err(GraphError::new("The MiniMax H3 graph changed: no guider reads the Reference to Video conditioning."));
    }

    let mut dangling: Vec<(String, String)> = Vec::new();
    for (key, node) in prompt.iter() {
        for (name, value) in node.get("inputs").and_then(Value::as_object).into_iter().flatten() {
            if let Value::Array(link) = value {
                if link.len() == 2 {
                    if let Some(target) = link[0].as_str() {
                        if !prompt.contains_key(target) {
                            dangling.push((key.clone(), name.clone()));
                        }
                    }
                }
            }
        }
    }
    if !dangling.is_empty() {
        return Err(GraphError::new(format!("The RefMod rewire left dangling links: {}", dangling_repr(&dangling))));
    }

    let items: Vec<Obj> = references
        .iter()
        .map(|r| {
            let mut o = Obj::new();
            for (k, v) in [("strength", json!(r.strength)), ("kind", json!(r.kind)), ("name", json!(r.display_name)), ("mod_name", json!(r.name)), ("tokens", json!(r.tokens)), ("key", json!("")), ("card_id", json!("")), ("category", json!(r.category)), ("reference_type", json!("")), ("description", json!("")), ("wears", json!(""))] {
                o.insert(k.into(), v);
            }
            o
        })
        .collect();
    let labelled = assign_labels(&items, with_audio);
    let labels: Vec<String> = labelled.iter().map(|e| e.get("label").and_then(Value::as_str).unwrap_or("").to_string()).collect();
    let report = token_report(&labelled);
    if let Some(imbalance) = report.get("imbalance").filter(|v| !v.is_null()) {
        eprintln!("[VRGDG-RS RefMod] {} is {}x lighter than {} and may be duplicated.", imbalance["weak"].as_str().unwrap_or(""), imbalance["ratio"], imbalance["strong"].as_str().unwrap_or(""));
    }
    if report["over_limit"].as_bool().unwrap_or(false) {
        eprintln!("[VRGDG-RS RefMod] Scene uses {} RefMod tokens (above {}), so it may not render or may crash.", report["total"], report["limit"]);
    }
    let mut text = text_or_empty(payload.get("prompt").filter(|v| truthy(v)));
    let relabelled = relabel_prompt(prompt, &prompt_node, &references, &labels);
    if !relabelled.is_empty() {
        text = prompt[&py_str(prompt_node.first())]["inputs"]["value"].as_str().unwrap_or("").to_string();
        eprintln!("[VRGDG-RS RefMod] Prompt labels updated to match the saved RefMods: {relabelled:?}");
    }
    let mut relabelled_map = Obj::new();
    for (old, new) in &relabelled {
        relabelled_map.insert(old.clone(), json!(new));
    }
    Ok(json!({
        "references": references.iter().map(|r| json!({"name": r.name, "strength": r.strength, "kind": r.kind, "tokens": r.tokens})).collect::<Vec<_>>(),
        "labels": labels,
        // <Audio 1> is only named in multi-performer prompts, so it is not reported.
        "labels_missing_from_prompt": labels.iter().filter(|l| !l.is_empty() && !l.starts_with("<Audio") && !text.contains(l.as_str())).collect::<Vec<_>>(),
        "tokens": references.iter().filter(|r| r.strength > 0.0).map(|r| r.tokens).sum::<i64>(),
        "token_report": report,
        "scene_audio_mod": with_audio,
        "guiders_rewired": rewired,
        "labels_relabelled": relabelled_map,
    }))
}
