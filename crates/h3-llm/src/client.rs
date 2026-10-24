//! Talking to the LLM. Port of the text paths of `llm/builder_runner.py` and `agent_api/llm_runtime.py`.
//!
//! LM Studio rule: a request that names a model that is not loaded makes LM Studio load it and unload the current one.
//! So the models LM Studio has loaded are read first (a read-only call), a request names one of those, and nothing is
//! sent when none is loaded.

use std::time::Duration;

use serde_json::{json, Value};

use crate::config::LlmConfig;

const CONTEXT_TEMPLATE_MARGIN_TOKENS: i64 = 64;
const MIN_OUTPUT_TOKENS: i64 = 256;

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    /// Nothing can be asked: LM Studio is down or has no model loaded, or a key is missing.
    #[error("{0}")]
    Unavailable(String),
    #[error("{0}")]
    Request(String),
    #[error("{0}")]
    Empty(String),
    #[error("{0}")]
    Context(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoadedModel {
    pub id: String,
    pub context_length: u32,
    pub vision: bool,
}

/// What a request would use right now.
#[derive(Debug, Clone, PartialEq)]
pub struct ActiveLlm {
    pub runner: String,
    pub model: String,
    pub context_length: u32,
    pub base_url: String,
    pub saved_model_setting: String,
}

#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub instruction: String,
    pub temperature: f64,
    pub top_p: f64,
    pub max_new_tokens: u32,
    /// Ask for JSON that fits this schema (`lm_studio` and `own_server` only).
    pub json_schema: Option<Value>,
    pub seed: Option<i64>,
    /// Pictures the model looks at, as `data:image/...;base64,...` URLs (`lm_studio` and `own_server` only).
    pub images: Vec<String>,
}

impl ChatRequest {
    pub fn with_images(mut self, images: Vec<String>) -> Self {
        self.images = images;
        self
    }

    pub fn text(instruction: &str, temperature: f64, top_p: f64, max_new_tokens: u32) -> Self {
        Self { instruction: instruction.to_string(), temperature, top_p, max_new_tokens, json_schema: None, seed: None, images: vec![] }
    }
}

#[derive(Debug, Clone)]
pub struct ChatReply {
    pub text: String,
    pub model: String,
    pub runner: String,
}

fn http(timeout: u32) -> reqwest::Client {
    reqwest::Client::builder().timeout(Duration::from_secs(u64::from(timeout.max(5)))).build().unwrap_or_default()
}

/// The LM Studio server address without a trailing `/v1`.
fn api_root(cfg: &LlmConfig) -> String {
    let base = cfg.lm_studio_base_url.trim().trim_end_matches('/');
    let base = if base.is_empty() { crate::config::LM_STUDIO_DEFAULT_BASE_URL } else { base };
    match base.strip_suffix("/v1") {
        Some(root) => root.trim_end_matches('/').to_string(),
        None => base.to_string(),
    }
}

fn bearer(request: reqwest::RequestBuilder, key: &str) -> reqwest::RequestBuilder {
    if key.trim().is_empty() { request } else { request.bearer_auth(key.trim()) }
}

/// Chat capable models LM Studio has loaded right now. Read-only: it cannot load or unload anything.
pub async fn loaded_lm_studio_models(cfg: &LlmConfig) -> Result<Vec<LoadedModel>, LlmError> {
    let root = api_root(cfg);
    let response = bearer(http(10).get(format!("{root}/api/v0/models")).header("Accept", "application/json"), &cfg.lm_studio_api_key)
        .send()
        .await
        .map_err(|_| LlmError::Unavailable(format!("Could not reach LM Studio at {root}. Start LM Studio's local server and load a model.")))?;
    if !response.status().is_success() {
        return Err(LlmError::Unavailable(format!("LM Studio could not report its loaded models ({}). Update LM Studio, or load a model and try again.", response.status().as_u16())));
    }
    let data: Value = response.json().await.map_err(|_| LlmError::Unavailable("LM Studio sent a reply that could not be read.".into()))?;
    Ok(data
        .get("data")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|m| m.get("state").and_then(Value::as_str) == Some("loaded") && matches!(m.get("type").and_then(Value::as_str), Some("llm" | "vlm")))
                .map(|m| LoadedModel {
                    id: m.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
                    context_length: m.get("loaded_context_length").and_then(Value::as_u64).unwrap_or(0) as u32,
                    vision: m.get("type").and_then(Value::as_str) == Some("vlm"),
                })
                .collect()
        })
        .unwrap_or_default())
}

/// The loaded model to use: the saved one when it is loaded, else the first loaded one.
async fn pick_lm_studio_model(cfg: &LlmConfig) -> Result<LoadedModel, LlmError> {
    let loaded = loaded_lm_studio_models(cfg).await?;
    if loaded.is_empty() {
        return Err(LlmError::Unavailable("LM Studio has no model loaded. Load one in LM Studio; the app never loads or switches models.".into()));
    }
    let wanted = cfg.lm_studio_model.trim();
    Ok(loaded.iter().find(|m| m.id == wanted).cloned().unwrap_or_else(|| loaded[0].clone()))
}

/// What the app would use right now, without calling the model.
pub async fn describe_active(cfg: &LlmConfig) -> Result<ActiveLlm, LlmError> {
    match cfg.runner.as_str() {
        "lm_studio" => {
            let model = pick_lm_studio_model(cfg).await?;
            let context = if model.context_length > 0 { cfg.lm_studio_context_limit.min(model.context_length) } else { cfg.lm_studio_context_limit };
            Ok(ActiveLlm { runner: "lm_studio".into(), model: model.id, context_length: context, base_url: api_root(cfg), saved_model_setting: cfg.lm_studio_model.clone() })
        }
        "comfyui_gemma" => {
            let found = crate::comfy_gemma::files(cfg).await?;
            let model = crate::comfy_gemma::pick_model(&cfg.comfy_gemma_model, &found)?;
            Ok(ActiveLlm { runner: "comfyui_gemma".into(), model, context_length: cfg.comfy_gemma_n_ctx, base_url: cfg.comfy_url.clone(), saved_model_setting: cfg.comfy_gemma_model.clone() })
        }
        "own_server" => Ok(ActiveLlm { runner: "own_server".into(), model: cfg.own_server_model.clone(), context_length: 0, base_url: cfg.own_server_url.clone(), saved_model_setting: cfg.own_server_model.clone() }),
        other => Ok(ActiveLlm { runner: other.into(), model: cfg.api_model.clone(), context_length: 0, base_url: String::new(), saved_model_setting: cfg.api_model.clone() }),
    }
}

fn estimate_tokens(text: &str) -> i64 {
    (text.chars().count() / 3) as i64 + 1
}

fn fit_output(wanted: i64, prompt_tokens: i64, context: i64) -> Result<i64, LlmError> {
    let available = context - prompt_tokens - CONTEXT_TEMPLATE_MARGIN_TOKENS;
    if available < MIN_OUTPUT_TOKENS {
        return Err(LlmError::Context(format!(
            "The prompt (about {prompt_tokens} tokens) leaves no room for a reply in the {context}-token LM Studio context window. Raise the context limit in the LLM settings, or load the model with a longer context."
        )));
    }
    Ok(wanted.min(available))
}

fn native_output_text(data: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    for item in data.get("output").and_then(Value::as_array).into_iter().flatten() {
        if !item.get("type").and_then(Value::as_str).is_some_and(|t| t.eq_ignore_ascii_case("message")) {
            continue;
        }
        match item.get("content") {
            Some(Value::String(s)) => parts.push(s.clone()),
            Some(Value::Array(blocks)) => {
                for block in blocks {
                    match block {
                        Value::String(s) => parts.push(s.clone()),
                        Value::Object(o) => {
                            if let Some(t) = o.get("text").or_else(|| o.get("content")).and_then(Value::as_str).filter(|t| !t.is_empty()) {
                                parts.push(t.to_string());
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    parts.iter().map(|p| p.trim()).filter(|p| !p.is_empty()).collect::<Vec<_>>().join("\n").trim().to_string()
}

/// The text of an OpenAI style chat reply.
pub fn message_text(data: &Value) -> String {
    let Some(choice) = data.get("choices").and_then(Value::as_array).and_then(|c| c.first()) else { return String::new() };
    let message = choice.get("message").cloned().unwrap_or(Value::Null);
    let mut text = match message.get("content") {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|i| match i {
                Value::String(s) => s.clone(),
                Value::Object(o) => o.get("text").or_else(|| o.get("content")).and_then(Value::as_str).unwrap_or("").to_string(),
                _ => String::new(),
            })
            .collect::<String>()
            .trim()
            .to_string(),
        _ => String::new(),
    };
    if text.is_empty() {
        for key in ["reasoning_content", "refusal"] {
            if let Some(extra) = message.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()) {
                return extra.trim().to_string();
            }
        }
        text = choice.get("text").and_then(Value::as_str).unwrap_or("").trim().to_string();
    }
    text
}

async fn send_json(request: reqwest::RequestBuilder, body: &Value, what: &str) -> Result<Value, LlmError> {
    let response = request.json(body).send().await.map_err(|e| LlmError::Request(format!("{what} could not be reached: {e}")))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(LlmError::Request(format!("{what} request failed ({}): {}", status.as_u16(), text.chars().take(600).collect::<String>())));
    }
    serde_json::from_str(&text).map_err(|_| LlmError::Request(format!("{what} sent a reply that is not JSON.")))
}

async fn lm_studio_chat(cfg: &LlmConfig, req: &ChatRequest) -> Result<ChatReply, LlmError> {
    let model = pick_lm_studio_model(cfg).await?;
    let root = api_root(cfg);
    let context = i64::from(if model.context_length > 0 { cfg.lm_studio_context_limit.min(model.context_length) } else { cfg.lm_studio_context_limit });
    // The saved output limit wins over the size the caller asks for, as in the pack.
    let configured = i64::from(cfg.lm_studio_output_limit).clamp(64, 262_144);
    let output = if req.images.is_empty() { fit_output(configured, estimate_tokens(&req.instruction), context)? } else { configured };
    let client = http(if req.images.is_empty() { cfg.lm_studio_timeout } else { cfg.lm_studio_timeout.max(300) });
    if let Some(schema) = &req.json_schema {
        // The native chat endpoint rejects response_format, so schema calls use the OpenAI compatible one.
        let mut body = json!({
            "model": model.id,
            "messages": [{"role": "user", "content": req.instruction}],
            "temperature": req.temperature,
            "top_p": req.top_p,
            "max_tokens": output,
            "stream": false,
            "response_format": {"type": "json_schema", "json_schema": {"name": "response", "strict": true, "schema": schema}},
        });
        if let Some(seed) = req.seed {
            body["seed"] = json!(seed);
        }
        let data = send_json(bearer(client.post(format!("{root}/v1/chat/completions")), &cfg.lm_studio_api_key), &body, "LM Studio").await?;
        let text = message_text(&data);
        if text.is_empty() {
            return Err(LlmError::Empty("LM Studio returned empty text.".into()));
        }
        return Ok(ChatReply { text, model: model.id, runner: "lm_studio".into() });
    }
    let input = if req.images.is_empty() {
        json!(req.instruction)
    } else {
        let mut blocks = vec![json!({"type": "text", "content": req.instruction})];
        blocks.extend(req.images.iter().map(|url| json!({"type": "image", "data_url": url})));
        Value::Array(blocks)
    };
    let mut body = json!({
        "model": model.id,
        "input": input,
        "temperature": req.temperature,
        "top_p": req.top_p,
        "context_length": context,
        "max_output_tokens": output,
        "store": false,
        "stream": false,
    });
    let reasoning = Some(cfg.lm_studio_reasoning.trim().to_lowercase()).filter(|r| !r.is_empty()).unwrap_or_else(|| if req.images.is_empty() { String::new() } else { "off".into() });
    if matches!(reasoning.as_str(), "off" | "low" | "medium" | "high" | "on") {
        body["reasoning"] = json!(reasoning);
    }
    if let Some(seed) = req.seed {
        body["seed"] = json!(seed);
    }
    let url = format!("{root}/api/v1/chat");
    let result = send_json(bearer(client.post(&url), &cfg.lm_studio_api_key), &body, "LM Studio").await;
    let data = match result {
        Err(LlmError::Request(m)) if m.contains("(404)") || m.contains("(405)") => {
            return Err(LlmError::Request("LM Studio's native /api/v1/chat endpoint is required to apply per-request context and output limits. Update LM Studio and make sure its Local Server is running.".into()))
        }
        // Some LM Studio versions reject `seed`. Ask again without it.
        Err(LlmError::Request(m)) if body.get("seed").is_some() && m.to_lowercase().contains("unrecognized_keys") && m.to_lowercase().contains("seed") => {
            body.as_object_mut().map(|b| b.remove("seed"));
            send_json(bearer(client.post(&url), &cfg.lm_studio_api_key), &body, "LM Studio").await?
        }
        other => other?,
    };
    let text = native_output_text(&data);
    if text.is_empty() {
        return Err(LlmError::Empty("LM Studio returned empty text.".into()));
    }
    Ok(ChatReply { text, model: model.id, runner: "lm_studio".into() })
}

/// The user message: plain text, or text and pictures as content parts.
fn user_content(req: &ChatRequest) -> Value {
    if req.images.is_empty() {
        return json!(req.instruction);
    }
    let mut parts = vec![json!({"type": "text", "text": req.instruction})];
    parts.extend(req.images.iter().map(|url| json!({"type": "image_url", "image_url": {"url": url}})));
    Value::Array(parts)
}

async fn openai_compatible_chat(base: &str, model: &str, key: &str, timeout: u32, output_limit: u32, req: &ChatRequest, label: &str) -> Result<ChatReply, LlmError> {
    let base = base.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err(LlmError::Unavailable(format!("The {label} address is empty.")));
    }
    if model.trim().is_empty() {
        return Err(LlmError::Unavailable(format!("Enter the {label} model name in the LLM settings.")));
    }
    let max_tokens = i64::from(output_limit.min(req.max_new_tokens.max(output_limit))).clamp(64, 262_144);
    let mut body = json!({
        "model": model.trim(),
        "messages": [{"role": "user", "content": user_content(req)}],
        "temperature": req.temperature,
        "top_p": req.top_p,
        "max_tokens": max_tokens,
        "stream": false,
    });
    if let Some(schema) = &req.json_schema {
        body["response_format"] = json!({"type": "json_schema", "json_schema": {"name": "response", "strict": true, "schema": schema}});
    }
    let data = send_json(bearer(http(timeout).post(format!("{base}/chat/completions")), key), &body, label).await?;
    let text = message_text(&data);
    if text.is_empty() {
        return Err(LlmError::Empty(format!("{label} returned empty text.")));
    }
    Ok(ChatReply { text, model: model.trim().to_string(), runner: label.to_string() })
}

async fn anthropic_chat(cfg: &LlmConfig, req: &ChatRequest) -> Result<ChatReply, LlmError> {
    if !req.images.is_empty() {
        return Err(LlmError::Unavailable("Picture input is not available for Anthropic in this app yet. Use LM Studio, an own server or OpenAI.".into()));
    }
    if cfg.api_key.trim().is_empty() || cfg.api_model.trim().is_empty() {
        return Err(LlmError::Unavailable("Enter the Anthropic API key and model name in the LLM settings.".into()));
    }
    let body = json!({"model": cfg.api_model.trim(), "max_tokens": req.max_new_tokens.max(64), "temperature": req.temperature, "top_p": req.top_p, "messages": [{"role": "user", "content": req.instruction}]});
    let request = http(cfg.own_server_timeout).post("https://api.anthropic.com/v1/messages").header("x-api-key", cfg.api_key.trim()).header("anthropic-version", "2023-06-01");
    let data = send_json(request, &body, "Anthropic").await?;
    let text: String = data.get("content").and_then(Value::as_array).map(|blocks| blocks.iter().filter_map(|b| b.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n")).unwrap_or_default();
    if text.trim().is_empty() {
        return Err(LlmError::Empty("Anthropic returned empty text.".into()));
    }
    Ok(ChatReply { text: text.trim().to_string(), model: cfg.api_model.clone(), runner: "anthropic".into() })
}

async fn google_chat(cfg: &LlmConfig, req: &ChatRequest) -> Result<ChatReply, LlmError> {
    if !req.images.is_empty() {
        return Err(LlmError::Unavailable("Picture input is not available for Google in this app yet. Use LM Studio, an own server or OpenAI.".into()));
    }
    if cfg.api_key.trim().is_empty() || cfg.api_model.trim().is_empty() {
        return Err(LlmError::Unavailable("Enter the Google API key and model name in the LLM settings.".into()));
    }
    let url = format!("https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent", cfg.api_model.trim());
    let body = json!({
        "contents": [{"role": "user", "parts": [{"text": req.instruction}]}],
        "generationConfig": {"temperature": req.temperature, "topP": req.top_p, "maxOutputTokens": req.max_new_tokens.max(64)},
    });
    let data = send_json(http(cfg.own_server_timeout).post(url).header("x-goog-api-key", cfg.api_key.trim()), &body, "Google").await?;
    let text: String = data
        .pointer("/candidates/0/content/parts")
        .and_then(Value::as_array)
        .map(|parts| parts.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join(""))
        .unwrap_or_default();
    if text.trim().is_empty() {
        return Err(LlmError::Empty("Google returned empty text.".into()));
    }
    Ok(ChatReply { text: text.trim().to_string(), model: cfg.api_model.clone(), runner: "google".into() })
}

/// Ask the configured LLM one question and return its raw reply.
pub async fn chat(cfg: &LlmConfig, req: &ChatRequest) -> Result<ChatReply, LlmError> {
    match cfg.runner.as_str() {
        "lm_studio" => lm_studio_chat(cfg, req).await,
        "own_server" => openai_compatible_chat(&cfg.own_server_url, &cfg.own_server_model, &cfg.own_server_api_key, cfg.own_server_timeout, cfg.own_server_output_limit, req, "own server").await,
        "comfyui_gemma" => crate::comfy_gemma::chat(cfg, req).await,
        "openai" => openai_compatible_chat("https://api.openai.com/v1", &cfg.api_model, &cfg.api_key, cfg.own_server_timeout, req.max_new_tokens, req, "OpenAI").await,
        "anthropic" => anthropic_chat(cfg, req).await,
        "google" => google_chat(cfg, req).await,
        other => Err(LlmError::Unavailable(format!("Unknown LLM runner '{other}'. Use lm_studio, own_server, comfyui_gemma, openai, anthropic or google."))),
    }
}
