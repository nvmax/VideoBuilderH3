//! The built-in Gemma: a Gemma GGUF model that runs inside ComfyUI, through the VRGDG Gemma node
//! (`VRGDG_SuperGemmaGGUFChat` of the comfyui-vrgamedevgirl pack). The app queues a small graph (the node and a
//! `PreviewAny` that shows its text), waits for it and reads the text, so no separate LLM server is needed. Models are the
//! `.gguf` files under ComfyUI's `models/LLM` folder. Pictures are uploaded to ComfyUI and passed to the node with the
//! mmproj file of the model.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use h3_comfy::{extract_texts, ComfyClient, ComfyError};
use serde_json::{json, Map, Value};

use crate::client::{ChatReply, ChatRequest, LlmError};
use crate::config::LlmConfig;

pub const NODE: &str = "VRGDG_SuperGemmaGGUFChat";
const MAX_IMAGES: usize = 24;

/// The files the node offers, as ComfyUI reports them.
#[derive(Debug, Clone, PartialEq)]
pub struct GemmaFiles {
    /// Gemma GGUF models. Empty when ComfyUI has none.
    pub models: Vec<String>,
    /// mmproj files, needed for pictures. Empty when ComfyUI has none.
    pub mmproj: Vec<String>,
    /// What the node lists for mmproj even when there is none. ComfyUI only accepts a listed value.
    pub mmproj_raw_first: String,
    /// Whether the node has the `gpu_device` input. The node of an older comfyui-vrgamedevgirl pack does not, and a GPU choice needs it.
    pub supports_gpu_choice: bool,
}

fn unavailable(message: impl Into<String>) -> LlmError {
    LlmError::Unavailable(message.into())
}

fn comfy_error(e: ComfyError, timeout: u32) -> LlmError {
    match e {
        ComfyError::Queue { message, .. } => {
            if message.contains(NODE) {
                unavailable(format!("ComfyUI does not have the Gemma node ({NODE}). Install the comfyui-vrgamedevgirl node pack and restart ComfyUI."))
            } else {
                unavailable(format!("ComfyUI rejected the Gemma request: {message}"))
            }
        }
        ComfyError::Execution(message) => unavailable(format!("Gemma in ComfyUI failed: {message}")),
        ComfyError::Timeout { .. } => unavailable(format!("Gemma in ComfyUI did not answer within {timeout} seconds. ComfyUI may be busy with a render, or the model is too large to load.")),
        other => unavailable(other.to_string()),
    }
}

fn client(cfg: &LlmConfig) -> Result<ComfyClient, LlmError> {
    if cfg.comfy_url.trim().is_empty() {
        return Err(unavailable("The ComfyUI address is not set. Set it in the setup."));
    }
    Ok(ComfyClient::new(cfg.comfy_url.trim()))
}

fn listed(info: &Value, field: &str) -> Vec<String> {
    info.pointer(&format!("/{NODE}/input/required/{field}/0"))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default()
}

/// Read the Gemma and mmproj files from the node's description. Placeholders such as `[No Gemma GGUF found...]` are dropped.
pub fn files_from_node_info(info: &Value) -> Result<GemmaFiles, LlmError> {
    if info.get(NODE).is_none() {
        return Err(unavailable(format!("ComfyUI does not have the Gemma node ({NODE}). Install the comfyui-vrgamedevgirl node pack and restart ComfyUI.")));
    }
    let real = |all: Vec<String>| -> Vec<String> { all.into_iter().filter(|f| !f.starts_with("[No ")).collect() };
    let raw_mmproj = listed(info, "mmproj_file");
    let supports_gpu_choice = info.pointer(&format!("/{NODE}/input/optional/gpu_device")).is_some();
    Ok(GemmaFiles { models: real(listed(info, "model_file")), mmproj_raw_first: raw_mmproj.first().cloned().unwrap_or_default(), mmproj: real(raw_mmproj), supports_gpu_choice })
}

/// The Gemma models and mmproj files ComfyUI has. Reads one node description. Nothing is loaded.
pub async fn files(cfg: &LlmConfig) -> Result<GemmaFiles, LlmError> {
    let info = client(cfg)?.object_info_for(NODE).await.map_err(|e| comfy_error(e, cfg.comfy_gemma_timeout))?;
    files_from_node_info(&info)
}

/// A GPU chosen in the LLM Runner needs the `gpu_device` input of the node. Without it ComfyUI would ignore the choice, so say so instead.
pub fn check_gpu_choice(cfg: &LlmConfig, files: &GemmaFiles) -> Result<(), LlmError> {
    if cfg.comfy_gemma_gpu_device >= 0 && !files.supports_gpu_choice {
        return Err(unavailable(format!(
            "GPU {} is chosen in the LLM Runner, but the comfyui-vrgamedevgirl pack in ComfyUI is too old to put a model on a GPU. Update the pack and restart ComfyUI, or choose Automatic.",
            cfg.comfy_gemma_gpu_device
        )));
    }
    Ok(())
}

/// The model to use: the saved one when ComfyUI lists it, else the first.
pub fn pick_model(saved: &str, files: &GemmaFiles) -> Result<String, LlmError> {
    let saved = saved.trim();
    if let Some(found) = files.models.iter().find(|m| m.as_str() == saved) {
        return Ok(found.clone());
    }
    files.models.first().cloned().ok_or_else(|| unavailable("ComfyUI has no Gemma GGUF model. Put a .gguf file with \"gemma\" in its name in ComfyUI's models/LLM folder and restart ComfyUI."))
}

fn pick_mmproj(saved: &str, files: &GemmaFiles) -> String {
    files.mmproj.iter().find(|m| m.as_str() == saved.trim()).or_else(|| files.mmproj.first()).cloned().unwrap_or_else(|| files.mmproj_raw_first.clone())
}

/// The graph for one request: a LoadImage per picture, the Gemma node and a node that shows its text.
pub fn build_graph(cfg: &LlmConfig, req: &ChatRequest, model: &str, mmproj: &str, image_names: &[String]) -> Value {
    let mut graph = Map::new();
    let mut node = Map::new();
    let count = image_names.len().min(MAX_IMAGES);
    for (i, name) in image_names.iter().take(count).enumerate() {
        let id = format!("image{}", i + 1);
        graph.insert(id.clone(), json!({"class_type": "LoadImage", "inputs": {"image": name}}));
        node.insert(format!("image{}", i + 1), json!([id, 0]));
    }
    let mut inputs = json!({
        "model_file": model,
        "mmproj_file": mmproj,
        "task_preset": "custom",
        "custom_instructions": req.instruction,
        "user_input": "",
        "trigger_word": "",
        "image_count": count,
        "advanced": true,
        "unload_after_run": !cfg.comfy_gemma_keep_loaded,
        "n_ctx": cfg.comfy_gemma_n_ctx.clamp(512, 262_144),
        "n_gpu_layers": 99,
        "n_threads": 8,
        "chat_format": "",
        "temperature": req.temperature.clamp(0.0, 2.0),
        "top_p": req.top_p.clamp(0.0, 1.0),
        "max_new_tokens": req.max_new_tokens.clamp(32, 32_000),
    });
    if cfg.comfy_gemma_gpu_device >= 0 {
        inputs["gpu_device"] = json!(cfg.comfy_gemma_gpu_device);
    }
    if let Some(object) = inputs.as_object_mut() {
        object.extend(node);
    }
    graph.insert("gemma".into(), json!({"class_type": NODE, "inputs": inputs}));
    graph.insert("show".into(), json!({"class_type": "PreviewAny", "inputs": {"source": ["gemma", 0]}}));
    Value::Object(graph)
}

/// `data:image/png;base64,...` as the file bytes and an extension.
fn decode_data_url(url: &str) -> Result<(Vec<u8>, &'static str), LlmError> {
    use base64::Engine;
    let rest = url.strip_prefix("data:").ok_or_else(|| LlmError::Request("A picture for Gemma must be a data URL.".into()))?;
    let (meta, data) = rest.split_once(',').ok_or_else(|| LlmError::Request("A picture for Gemma is not a valid data URL.".into()))?;
    let extension = if meta.contains("jpeg") || meta.contains("jpg") {
        "jpg"
    } else if meta.contains("webp") {
        "webp"
    } else {
        "png"
    };
    let bytes = base64::engine::general_purpose::STANDARD.decode(data.trim()).map_err(|e| LlmError::Request(format!("A picture for Gemma could not be decoded: {e}")))?;
    Ok((bytes, extension))
}

async fn upload(cfg: &LlmConfig, index: usize, url: &str) -> Result<String, LlmError> {
    let (bytes, extension) = decode_data_url(url)?;
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let filename = format!("h3_gemma_{nanos}_{index}.{extension}");
    let mime = match extension {
        "jpg" => "image/jpeg",
        "webp" => "image/webp",
        _ => "image/png",
    };
    let part = reqwest::multipart::Part::bytes(bytes).file_name(filename.clone()).mime_str(mime).map_err(|e| LlmError::Request(e.to_string()))?;
    let form = reqwest::multipart::Form::new().part("image", part).text("type", "input").text("overwrite", "true");
    let base = cfg.comfy_url.trim().trim_end_matches('/');
    let response = reqwest::Client::new()
        .post(format!("{base}/upload/image"))
        .multipart(form)
        .timeout(Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| unavailable(format!("Could not upload a picture to ComfyUI: {e}")))?;
    if !response.status().is_success() {
        return Err(unavailable(format!("ComfyUI refused a picture upload ({}).", response.status().as_u16())));
    }
    let data: Value = response.json().await.unwrap_or(Value::Null);
    Ok(data.get("name").and_then(Value::as_str).unwrap_or(&filename).to_string())
}

/// Ask the Gemma in ComfyUI. The model comes from the saved choice or the first Gemma GGUF ComfyUI has.
pub async fn chat(cfg: &LlmConfig, req: &ChatRequest) -> Result<ChatReply, LlmError> {
    let client = client(cfg)?;
    let found = files(cfg).await?;
    check_gpu_choice(cfg, &found)?;
    let model = pick_model(&cfg.comfy_gemma_model, &found)?;
    let mut names: Vec<String> = Vec::new();
    if !req.images.is_empty() {
        if found.mmproj.is_empty() {
            return Err(unavailable("A picture needs a Gemma mmproj file. Put a .gguf file with \"mmproj\" in its name next to the model in ComfyUI's models/LLM folder and restart ComfyUI."));
        }
        for (i, url) in req.images.iter().take(MAX_IMAGES).enumerate() {
            names.push(upload(cfg, i, url).await?);
        }
    }
    let graph = build_graph(cfg, req, &model, &pick_mmproj(&cfg.comfy_gemma_mmproj, &found), &names);
    let queued = client.queue_prompt(&graph).await.map_err(|e| comfy_error(e, cfg.comfy_gemma_timeout))?;
    let cancel = Arc::new(AtomicBool::new(false));
    let history = client
        .wait_for_prompt(&queued.prompt_id, Duration::from_secs(u64::from(cfg.comfy_gemma_timeout.max(30))), Duration::from_millis(500), &cancel, |_| {})
        .await
        .map_err(|e| comfy_error(e, cfg.comfy_gemma_timeout))?;
    let text = extract_texts(&history, &queued.prompt_id).join("\n").trim().to_string();
    if text.is_empty() {
        return Err(LlmError::Empty("Gemma in ComfyUI answered with no text.".into()));
    }
    Ok(ChatReply { text, model, runner: "comfyui_gemma".into() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node_info() -> Value {
        json!({NODE: {"input": {"required": {
            "model_file": [["gemma-4-E4B-it-Q8_0.gguf", "gemma-4-26B-it.gguf"], {}],
            "mmproj_file": [["mmproj-gemma-4-E4B-it-BF16.gguf"], {}],
        }}}})
    }

    #[test]
    fn files_come_from_the_node_and_placeholders_are_dropped() {
        let files = files_from_node_info(&node_info()).unwrap();
        assert_eq!(files.models, ["gemma-4-E4B-it-Q8_0.gguf", "gemma-4-26B-it.gguf"]);
        assert_eq!(files.mmproj, ["mmproj-gemma-4-E4B-it-BF16.gguf"]);
        let none = json!({NODE: {"input": {"required": {"model_file": [["[No Gemma GGUF found in models/LLM]"], {}], "mmproj_file": [["[No mmproj GGUF found in models/LLM]"], {}]}}}});
        let empty = files_from_node_info(&none).unwrap();
        assert!(empty.models.is_empty() && empty.mmproj.is_empty());
        assert_eq!(empty.mmproj_raw_first, "[No mmproj GGUF found in models/LLM]");
        assert!(pick_model("", &empty).unwrap_err().to_string().contains("models/LLM"));
        assert!(files_from_node_info(&json!({})).unwrap_err().to_string().contains(NODE));
    }

    #[test]
    fn a_gpu_choice_goes_into_the_request_only_when_one_is_chosen_and_an_old_node_is_noticed() {
        let req = ChatRequest::text("x", 0.6, 0.95, 100);
        let automatic = build_graph(&LlmConfig::default(), &req, "m.gguf", "p.gguf", &[]);
        assert!(automatic["gemma"]["inputs"].get("gpu_device").is_none(), "automatic leaves the request as it always was");
        let pinned = build_graph(&LlmConfig { comfy_gemma_gpu_device: 1, ..LlmConfig::default() }, &req, "m.gguf", "p.gguf", &[]);
        assert_eq!(pinned["gemma"]["inputs"]["gpu_device"], 1);

        let old = files_from_node_info(&node_info()).unwrap();
        assert!(!old.supports_gpu_choice);
        let current = json!({NODE: {"input": {
            "required": {"model_file": [["g.gguf"], {}], "mmproj_file": [["p.gguf"], {}]},
            "optional": {"gpu_device": ["INT", {"default": -1}]},
        }}});
        let new = files_from_node_info(&current).unwrap();
        assert!(new.supports_gpu_choice);

        let chosen = LlmConfig { comfy_gemma_gpu_device: 0, ..LlmConfig::default() };
        assert!(check_gpu_choice(&LlmConfig::default(), &old).is_ok(), "automatic works with an old node");
        assert!(check_gpu_choice(&chosen, &new).is_ok());
        let refused = check_gpu_choice(&chosen, &old).unwrap_err().to_string();
        assert!(refused.contains("GPU 0") && refused.contains("Update the pack"), "{refused}");
    }

    #[test]
    fn the_saved_model_is_used_when_listed_else_the_first() {
        let files = files_from_node_info(&node_info()).unwrap();
        assert_eq!(pick_model("gemma-4-26B-it.gguf", &files).unwrap(), "gemma-4-26B-it.gguf");
        assert_eq!(pick_model("gone.gguf", &files).unwrap(), "gemma-4-E4B-it-Q8_0.gguf");
        assert_eq!(pick_model("", &files).unwrap(), "gemma-4-E4B-it-Q8_0.gguf");
    }

    #[test]
    fn the_graph_has_the_node_pictures_and_a_text_output() {
        let cfg = LlmConfig::default();
        let req = ChatRequest::text("Write one prompt.", 0.6, 0.95, 4000);
        let graph = build_graph(&cfg, &req, "m.gguf", "p.gguf", &[]);
        assert_eq!(graph["gemma"]["class_type"], NODE);
        assert_eq!(graph["gemma"]["inputs"]["custom_instructions"], "Write one prompt.");
        assert_eq!((graph["gemma"]["inputs"]["task_preset"].as_str(), graph["gemma"]["inputs"]["image_count"].as_i64()), (Some("custom"), Some(0)));
        assert_eq!(graph["gemma"]["inputs"]["unload_after_run"], true, "the model leaves the GPU unless it is kept");
        assert_eq!(graph["show"]["class_type"], "PreviewAny");
        assert_eq!(graph["show"]["inputs"]["source"], json!(["gemma", 0]));

        let keep = LlmConfig { comfy_gemma_keep_loaded: true, ..cfg };
        let two = build_graph(&keep, &ChatRequest::text("x", 9.0, 5.0, 10), "m.gguf", "p.gguf", &["a.png".into(), "b.png".into()]);
        assert_eq!(two["gemma"]["inputs"]["image2"], json!(["image2", 0]));
        assert_eq!(two["image1"]["inputs"]["image"], "a.png");
        assert_eq!((two["gemma"]["inputs"]["unload_after_run"].clone(), two["gemma"]["inputs"]["image_count"].clone()), (json!(false), json!(2)));
        // Values are kept inside what the node accepts.
        assert_eq!((two["gemma"]["inputs"]["temperature"].as_f64(), two["gemma"]["inputs"]["top_p"].as_f64(), two["gemma"]["inputs"]["max_new_tokens"].as_i64()), (Some(2.0), Some(1.0), Some(32)));
    }

    #[test]
    fn pictures_are_decoded_from_data_urls() {
        let (bytes, ext) = decode_data_url("data:image/jpeg;base64,aGk=").unwrap();
        assert_eq!((bytes.as_slice(), ext), (&b"hi"[..], "jpg"));
        assert!(decode_data_url("not a url").is_err());
        assert!(decode_data_url("data:image/png;base64,@@@").is_err());
    }
}
