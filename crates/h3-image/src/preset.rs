//! An image preset: a ComfyUI API workflow plus the bindings that say which inputs the app fills in.
//!
//! A binding has a name the app and the API use (`prompt`, `seed`, `width`...), and one or more targets, the node inputs
//! it writes. That is all a new image model needs: import its workflow, point the bindings at its nodes, and every
//! scene and reference card can use it.

use std::path::{Path, PathBuf};

use h3_core::pyval::{to_float, truthy, Obj};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::ImageError;

/// A node input a binding writes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Target {
    pub node: String,
    pub input: String,
}

impl Target {
    pub fn new(node: &str, input: &str) -> Self {
        Self { node: node.into(), input: input.into() }
    }
}

/// A change to the graph, run when a binding is on or off (an image given or not, a switch).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// Set an input to a value.
    Set { node: String, input: String, value: Value },
    /// Connect an input to another node's output.
    Link { node: String, input: String, from_node: String, from_slot: u32 },
    /// Remove a node.
    Remove { node: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingKind {
    Text,
    Int,
    Float,
    Bool,
    /// One of a list of names (model files). The list comes from the first target's input in ComfyUI.
    Choice,
    /// A seed. A negative value is replaced by a random one.
    Seed,
    /// Any number of image files. Targets get a JSON array of absolute paths.
    Images,
    /// One image file, copied into ComfyUI's input folder. Targets get the file name.
    Image,
    /// Up to twenty LoRAs, `[{"name": ..., "strength": ...}]`. The first target's node is the LoRA node.
    Loras,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Binding {
    pub key: String,
    pub label: String,
    pub kind: BindingKind,
    pub targets: Vec<Target>,
    /// Value used when the caller sends none. `null` keeps what the workflow has.
    #[serde(default)]
    pub default: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// `Images`/`Image`/`Bool`: run when there is a value (true).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub when_set: Vec<Op>,
    /// `Images`/`Image`/`Bool`: run when there is none (false).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub when_unset: Vec<Op>,
    /// `Image`: with no file, targets get a small placeholder image so the node still loads.
    #[serde(default)]
    pub placeholder: bool,
    /// `Loras`: the LoRA node has a first and a second pass strength per slot.
    #[serde(default)]
    pub two_pass: bool,
}

impl Binding {
    pub fn new(key: &str, label: &str, kind: BindingKind, targets: &[(&str, &str)]) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind,
            targets: targets.iter().map(|(n, i)| Target::new(n, i)).collect(),
            default: Value::Null,
            min: None,
            max: None,
            when_set: vec![],
            when_unset: vec![],
            placeholder: false,
            two_pass: false,
        }
    }

    pub fn default(mut self, value: Value) -> Self {
        self.default = value;
        self
    }

    pub fn range(mut self, min: f64, max: f64) -> Self {
        self.min = Some(min);
        self.max = Some(max);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresetOrigin {
    Builtin,
    Custom,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImagePreset {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub origin: PresetOrigin,
    /// The workflow in ComfyUI's API format.
    pub workflow: Value,
    pub bindings: Vec<Binding>,
    /// Nodes whose images are the result. Empty means the last image any node produced.
    #[serde(default)]
    pub output_nodes: Vec<String>,
}

/// What the graph needs from the machine it runs on.
pub struct ApplyCtx {
    /// ComfyUI's input folder, where reference and image to image files are copied.
    pub input_dir: PathBuf,
    /// A new random number for a random seed.
    pub random: Box<dyn FnMut() -> u64 + Send>,
}

/// A graph ready to queue.
#[derive(Debug, Clone)]
pub struct Applied {
    pub graph: Value,
    /// The seed the render used, when the preset has a seed binding.
    pub seed: Option<u64>,
}

const MAX_LORA_SLOTS: usize = 20;
pub const PLACEHOLDER_NAME: &str = "vrgdg_placeholder_i2i.png";

fn graph_input<'a>(graph: &'a mut Value, node: &str, input: &str) -> Result<&'a mut Value, ImageError> {
    let entry = graph
        .get_mut(node)
        .ok_or_else(|| ImageError::Preset(format!("The workflow has no node {node}.")))?;
    let inputs = entry
        .get_mut("inputs")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| ImageError::Preset(format!("Node {node} has no inputs.")))?;
    Ok(inputs.entry(input.to_string()).or_insert(Value::Null))
}

fn set(graph: &mut Value, node: &str, input: &str, value: Value) -> Result<(), ImageError> {
    *graph_input(graph, node, input)? = value;
    Ok(())
}

fn run_ops(graph: &mut Value, ops: &[Op]) -> Result<(), ImageError> {
    for op in ops {
        match op {
            Op::Set { node, input, value } => set(graph, node, input, value.clone())?,
            Op::Link { node, input, from_node, from_slot } => set(graph, node, input, json!([from_node, from_slot]))?,
            Op::Remove { node } => {
                if let Some(map) = graph.as_object_mut() {
                    map.remove(node);
                }
            }
        }
    }
    Ok(())
}

fn number(value: &Value) -> Option<f64> {
    to_float(value)
}

fn clamp(binding: &Binding, value: f64) -> f64 {
    let low = binding.min.unwrap_or(f64::MIN);
    let high = binding.max.unwrap_or(f64::MAX);
    value.clamp(low, high)
}

/// Make sure the placeholder image exists in the input folder and return its name.
pub fn ensure_placeholder(input_dir: &Path) -> Result<String, ImageError> {
    std::fs::create_dir_all(input_dir)?;
    let path = input_dir.join(PLACEHOLDER_NAME);
    if path.metadata().map(|m| m.len() > 0).unwrap_or(false) {
        return Ok(PLACEHOLDER_NAME.to_string());
    }
    image::RgbImage::from_pixel(64, 64, image::Rgb([127, 127, 127])).save_with_format(&path, image::ImageFormat::Png).map_err(|e| ImageError::Other(e.to_string()))?;
    Ok(PLACEHOLDER_NAME.to_string())
}

/// Copy a file into ComfyUI's input folder under a new name and return that name.
fn copy_to_input(source: &Path, input_dir: &Path, counter: usize) -> Result<String, ImageError> {
    if !source.is_file() {
        return Err(ImageError::Input(format!("Image file not found: {}", source.display())));
    }
    std::fs::create_dir_all(input_dir)?;
    let ext = source.extension().map(|e| format!(".{}", e.to_string_lossy().to_lowercase())).unwrap_or_else(|| ".png".into());
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let name = format!("vrgdg_img_{stamp}_{counter}{ext}");
    std::fs::copy(source, input_dir.join(&name))?;
    Ok(name)
}

fn text_list(value: &Value) -> Vec<String> {
    match value {
        Value::Array(items) => items
            .iter()
            .filter_map(|v| match v {
                Value::String(s) => Some(s.trim().to_string()),
                Value::Object(o) => o.get("path").and_then(Value::as_str).map(|s| s.trim().to_string()),
                _ => None,
            })
            .filter(|s| !s.is_empty())
            .collect(),
        Value::String(s) => s.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect(),
        _ => vec![],
    }
}

/// Fill the preset's workflow with `values` (binding key to value). Bindings run in their listed order.
pub fn apply_preset(preset: &ImagePreset, values: &Obj, ctx: &mut ApplyCtx) -> Result<Applied, ImageError> {
    let mut graph = preset.workflow.clone();
    let mut seed_used = None;
    let mut copies = 0usize;
    for binding in &preset.bindings {
        let given = values.get(&binding.key).filter(|v| !v.is_null());
        let value = given.cloned().unwrap_or_else(|| binding.default.clone());
        match binding.kind {
            BindingKind::Text | BindingKind::Choice => {
                if value.is_null() {
                    continue;
                }
                let text = match &value {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                if binding.key == "prompt" && text.trim().is_empty() {
                    return Err(ImageError::Input("Prompt text is empty.".into()));
                }
                for t in &binding.targets {
                    set(&mut graph, &t.node, &t.input, json!(text))?;
                }
            }
            BindingKind::Int | BindingKind::Float => {
                let Some(n) = number(&value) else { continue };
                let n = clamp(binding, n);
                for t in &binding.targets {
                    let v = if binding.kind == BindingKind::Int { json!(n.round() as i64) } else { json!(n) };
                    set(&mut graph, &t.node, &t.input, v)?;
                }
            }
            BindingKind::Bool => {
                if value.is_null() {
                    continue;
                }
                let on = truthy(&value);
                for t in &binding.targets {
                    set(&mut graph, &t.node, &t.input, json!(on))?;
                }
                run_ops(&mut graph, if on { &binding.when_set } else { &binding.when_unset })?;
            }
            BindingKind::Seed => {
                let wanted = number(&value).unwrap_or(-1.0);
                // Seeds stay below 2^53 so they survive a trip through JSON as a number.
                let seed = if wanted < 0.0 { (ctx.random)() % (1u64 << 53) } else { (wanted as u64).min((1u64 << 53) - 1) };
                for t in &binding.targets {
                    set(&mut graph, &t.node, &t.input, json!(seed))?;
                }
                seed_used = Some(seed);
            }
            BindingKind::Images => {
                let paths: Vec<String> = text_list(&value)
                    .into_iter()
                    .map(|p| {
                        let path = Path::new(&p);
                        if path.is_file() {
                            Ok(std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()).display().to_string())
                        } else {
                            Err(ImageError::Input(format!("Reference image not found: {p}")))
                        }
                    })
                    .collect::<Result<_, _>>()?;
                if paths.is_empty() {
                    run_ops(&mut graph, &binding.when_unset)?;
                } else {
                    let encoded = serde_json::to_string(&paths).unwrap_or_default();
                    for t in &binding.targets {
                        set(&mut graph, &t.node, &t.input, json!(encoded))?;
                    }
                    run_ops(&mut graph, &binding.when_set)?;
                }
            }
            BindingKind::Image => {
                let source = text_list(&value).into_iter().next();
                match source {
                    Some(path) => {
                        copies += 1;
                        let name = copy_to_input(Path::new(&path), &ctx.input_dir, copies)?;
                        for t in &binding.targets {
                            set(&mut graph, &t.node, &t.input, json!(name))?;
                        }
                        run_ops(&mut graph, &binding.when_set)?;
                    }
                    None => {
                        if binding.placeholder {
                            let name = ensure_placeholder(&ctx.input_dir)?;
                            for t in &binding.targets {
                                set(&mut graph, &t.node, &t.input, json!(name))?;
                            }
                        }
                        run_ops(&mut graph, &binding.when_unset)?;
                    }
                }
            }
            BindingKind::Loras => {
                let Some(target) = binding.targets.first() else { continue };
                let list: Vec<(String, f64)> = match &value {
                    Value::Array(items) => items
                        .iter()
                        .filter_map(|v| {
                            let name = v.get("name").and_then(Value::as_str)?.trim().to_string();
                            let strength = v.get("strength").and_then(number).unwrap_or(1.0);
                            (!name.is_empty() && name != "[none]").then_some((name, strength))
                        })
                        .take(MAX_LORA_SLOTS)
                        .collect(),
                    _ => vec![],
                };
                let node = &target.node;
                set(&mut graph, node, "use_custom_loras", json!(!list.is_empty()))?;
                set(&mut graph, node, "lora_count", json!(list.len()))?;
                for slot in 1..=MAX_LORA_SLOTS {
                    let (name, strength) = list.get(slot - 1).cloned().unwrap_or(("[none]".to_string(), 1.0));
                    set(&mut graph, node, &format!("lora_{slot}"), json!(name))?;
                    if binding.two_pass {
                        set(&mut graph, node, &format!("first_pass_strength_{slot}"), json!(strength))?;
                        set(&mut graph, node, &format!("second_pass_strength_{slot}"), json!(strength))?;
                    } else {
                        set(&mut graph, node, &format!("strength_{slot}"), json!(strength))?;
                    }
                }
            }
        }
    }
    Ok(Applied { graph, seed: seed_used })
}

/// The class types a preset's workflow uses, so a missing custom node is reported before anything is queued.
pub fn class_types(workflow: &Value) -> Vec<String> {
    let mut found: Vec<String> = workflow
        .as_object()
        .map(|nodes| nodes.values().filter_map(|n| n.get("class_type").and_then(Value::as_str).map(str::to_string)).collect())
        .unwrap_or_default();
    found.sort();
    found.dedup();
    found
}
