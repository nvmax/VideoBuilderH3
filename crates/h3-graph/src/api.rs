//! API graph helpers. Port of the node lookup and wiring helpers in `runner/api_graph.py`. A graph is a JSON object of
//! node id to `{class_type, inputs, _meta}`, and a link is `[node id, output index]`.

use h3_core::pyval::{to_int, Obj};
use serde_json::{json, Value};

use crate::env::{GraphEnv, GraphError, Result};

/// A link to output `index` of node `id`.
pub fn link(id: &str, index: i64) -> Value {
    json!([id, index])
}

/// `[id, index]` when the value is a two item list, the shape of a link.
pub fn as_link(value: &Value) -> Option<(String, i64)> {
    let list = value.as_array()?;
    if list.len() != 2 {
        return None;
    }
    let id = match &list[0] {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    Some((id, to_int(&list[1]).unwrap_or(0)))
}

pub fn node<'a>(prompt: &'a Obj, id: &str) -> Option<&'a Obj> {
    prompt.get(id).and_then(Value::as_object)
}

pub fn node_mut<'a>(prompt: &'a mut Obj, id: &str) -> Option<&'a mut Obj> {
    prompt.get_mut(id).and_then(Value::as_object_mut)
}

pub fn inputs<'a>(prompt: &'a Obj, id: &str) -> Option<&'a Obj> {
    node(prompt, id).and_then(|n| n.get("inputs")).and_then(Value::as_object)
}

pub fn class_of<'a>(prompt: &'a Obj, id: &str) -> &'a str {
    node(prompt, id).and_then(|n| n.get("class_type")).and_then(Value::as_str).unwrap_or("")
}

/// Set one input of a node. The node must exist.
pub fn set_input(prompt: &mut Obj, id: &str, name: &str, value: Value) -> Result<()> {
    let Some(n) = node_mut(prompt, id) else {
        return Err(GraphError::new(format!("'API prompt node {id} was not found.'")));
    };
    let entry = n.entry("inputs").or_insert_with(|| json!({}));
    if let Some(map) = entry.as_object_mut() {
        map.insert(name.to_string(), value);
    }
    Ok(())
}

/// Set an input when the node exists. Returns whether it did.
pub fn set_optional_input(prompt: &mut Obj, id: &str, name: &str, value: Value) -> bool {
    set_input(prompt, id, name, value).is_ok()
}

/// Id of the first node of a class, else `fallback` when that id is in the graph.
pub fn node_id_by_class(prompt: &Obj, class: &str, fallback: Option<&str>) -> Result<String> {
    for (id, n) in prompt {
        if n.get("class_type").and_then(Value::as_str) == Some(class) {
            return Ok(id.clone());
        }
    }
    match fallback {
        Some(id) if prompt.contains_key(id) => Ok(id.to_string()),
        _ => Err(GraphError::new(format!("'API prompt node class {class} was not found.'"))),
    }
}

/// Id of the first node of a class (and title, when given), else the first fallback id of that class, else `None`.
pub fn optional_node_id_by_class(prompt: &Obj, class: &str, title: &str, fallback_ids: &[&str]) -> Option<String> {
    for (id, n) in prompt {
        let Some(map) = n.as_object() else { continue };
        if map.get("class_type").and_then(Value::as_str).map(str::trim) != Some(class) {
            continue;
        }
        let node_title = map.get("_meta").and_then(|m| m.get("title")).and_then(Value::as_str).unwrap_or("").trim();
        if !title.is_empty() && node_title != title {
            continue;
        }
        return Some(id.clone());
    }
    fallback_ids
        .iter()
        .find(|id| node(prompt, id).is_some_and(|_| class_of(prompt, id).trim() == class))
        .map(|id| id.to_string())
}

/// Point every input that reads `old` at `new`. Returns how many were changed.
pub fn replace_input_refs(prompt: &mut Obj, old: (&str, i64), new: (&str, i64)) -> usize {
    let mut replaced = 0;
    for n in prompt.values_mut() {
        let Some(map) = n.get_mut("inputs").and_then(Value::as_object_mut) else { continue };
        for value in map.values_mut() {
            if as_link(value).is_some_and(|(id, i)| id == old.0 && i == old.1) {
                *value = link(new.0, new.1);
                replaced += 1;
            }
        }
    }
    replaced
}

/// The decode node that feeds the final video: walk back from the video combine node through image inputs.
pub fn find_final_vae_decode_id(prompt: &Obj) -> Result<String> {
    let combine = ["353", "142"]
        .iter()
        .find(|c| class_of(prompt, c) == "VHS_VideoCombine")
        .map(|c| c.to_string());
    let combine = match combine {
        Some(c) => c,
        None => node_id_by_class(prompt, "VHS_VideoCombine", Some("142"))?,
    };
    if prompt.contains_key(&combine) {
        let first = inputs(prompt, &combine).and_then(|i| i.get("images")).and_then(Value::as_array).and_then(|a| a.first());
        if let Some(first) = first {
            let text = |v: &Value| match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            let mut current = text(first);
            let mut visited: Vec<String> = Vec::new();
            while !current.is_empty() && prompt.contains_key(&current) && !visited.contains(&current) {
                visited.push(current.clone());
                if matches!(class_of(prompt, &current), "VAEDecode" | "MiniMaxH3AVDecodeT8") {
                    return Ok(current);
                }
                let ins = inputs(prompt, &current);
                let upstream = ins.and_then(|i| i.get("images").filter(|v| h3_core::pyval::truthy(v)).or_else(|| i.get("image")));
                match upstream.and_then(Value::as_array).and_then(|a| a.first()) {
                    Some(next) => current = text(next),
                    None => break,
                }
            }
        }
    }
    node_id_by_class(prompt, "VAEDecode", Some("122"))
}

/// Names of every input the installed node declares (required then optional).
pub fn node_input_names(env: &dyn GraphEnv, class: &str) -> Vec<String> {
    let Some(info) = env.node_info(class) else { return vec![] };
    let mut names = Vec::new();
    for section in ["required", "optional"] {
        if let Some(map) = info.get("input").and_then(|i| i.get(section)).and_then(Value::as_object) {
            names.extend(map.keys().cloned());
        }
    }
    names
}

/// Keep the given inputs, add extras only when the installed node declares them, and fill inputs a newer node version
/// added from that node's own defaults so the prompt still validates.
pub fn compat_node_inputs(env: &dyn GraphEnv, class: &str, required: Obj, extra_defaults: &Obj) -> Obj {
    let mut inputs = required;
    let names = node_input_names(env, class);
    for (key, value) in extra_defaults {
        if names.contains(key) {
            inputs.insert(key.clone(), value.clone());
        }
    }
    if let Some(info) = env.node_info(class) {
        for section in ["required", "optional"] {
            let Some(map) = info.get("input").and_then(|i| i.get(section)).and_then(Value::as_object) else { continue };
            for (name, spec) in map {
                let default = spec.as_array().and_then(|a| a.get(1)).and_then(|o| o.get("default"));
                if let Some(default) = default.filter(|d| !d.is_null()) {
                    inputs.entry(name.clone()).or_insert_with(|| default.clone());
                }
            }
        }
    }
    inputs
}
