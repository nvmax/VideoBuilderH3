//! The Custom Server runner: any OpenAI compatible server (vLLM, llama.cpp, a tunnel to a rented GPU). Port of the own server
//! functions in `llm/builder_runner.py`.

use std::time::Duration;

use fancy_regex::Regex;
use serde_json::{json, Value};

use crate::client::{message_text, ChatReply, ChatRequest, LlmError};
use crate::config::LlmConfig;

const CONTEXT_TEMPLATE_MARGIN_TOKENS: i64 = 64;
const MIN_OUTPUT_TOKENS: i64 = 256;

/// `_own_server_v1_root`: the `.../v1` address of a server URL typed by a person.
pub fn v1_root(raw: &str) -> Result<String, LlmError> {
    let url = raw.trim();
    if url.is_empty() {
        return Err(LlmError::Unavailable("Own server URL is empty. Open LLM Runner and paste your server URL.".into()));
    }
    let (scheme, rest) = match url.split_once("://") {
        Some((scheme, rest)) if matches!(scheme.to_lowercase().as_str(), "http" | "https") => (scheme.to_lowercase(), rest),
        _ => return Err(LlmError::Unavailable("Own server URL must start with http:// or https://.".into())),
    };
    let rest = rest.split(['?', '#']).next().unwrap_or("");
    let (host, path) = match rest.find('/') {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, ""),
    };
    if host.is_empty() {
        return Err(LlmError::Unavailable("Own server URL is missing a host.".into()));
    }
    let mut path = path.trim_end_matches('/').to_string();
    for suffix in ["/chat/completions", "/completions", "/models"] {
        if path.to_lowercase().ends_with(suffix) {
            path.truncate(path.len() - suffix.len());
            break;
        }
    }
    let v1 = if path.to_lowercase().ends_with("/v1") {
        path
    } else if !path.is_empty() {
        format!("{path}/v1")
    } else {
        "/v1".to_string()
    };
    Ok(format!("{scheme}://{host}{v1}"))
}

fn timeout_seconds(cfg: &LlmConfig, floor: u32) -> u64 {
    u64::from(cfg.own_server_timeout.clamp(15, 600).max(floor))
}

async fn request_json(cfg: &LlmConfig, url: &str, body: Option<&Value>, timeout: u64) -> Result<Value, LlmError> {
    let client = reqwest::Client::builder().timeout(Duration::from_secs(timeout)).build().unwrap_or_default();
    let mut request = match body {
        Some(_) => client.post(url).header("Content-Type", "application/json"),
        None => client.get(url),
    };
    request = request.header("Accept", "application/json").header("User-Agent", "VRGDG-OwnServer/1.0 (+ComfyUI)");
    if !cfg.own_server_api_key.trim().is_empty() {
        request = request.bearer_auth(cfg.own_server_api_key.trim());
    }
    if let Some(body) = body {
        request = request.json(body);
    }
    let response = request.send().await.map_err(|e| {
        let text = format!("{e:?}").to_lowercase();
        if text.contains("certificate") || text.contains("invalidcert") || text.contains("tls") {
            LlmError::Request(format!("TLS certificate validation failed for own server at {url}. Install a trusted certificate or use HTTP only for a server on a trusted local network."))
        } else {
            LlmError::Request(format!("Could not connect to own server at {url}. Check the URL, local bind address, or Cloudflare tunnel."))
        }
    })?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        let reason = status.canonical_reason().unwrap_or("");
        return Err(LlmError::Request(format!("Own server request failed ({}): {}", status.as_u16(), if text.trim().is_empty() { reason } else { &text })));
    }
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(&text).map_err(|e| LlmError::Request(format!("Own server request failed (200): {e}")))
}

fn user_messages(req: &ChatRequest) -> Value {
    if req.images.is_empty() {
        return json!([{"role": "user", "content": req.instruction}]);
    }
    let mut content = vec![json!({"type": "text", "text": req.instruction})];
    content.extend(req.images.iter().take(4).map(|url| json!({"type": "image_url", "image_url": {"url": url}})));
    json!([{"role": "user", "content": content}])
}

/// Ask the server one question. Retries once when the server says how much room is left, or wants `max_completion_tokens`.
pub async fn chat(cfg: &LlmConfig, req: &ChatRequest) -> Result<ChatReply, LlmError> {
    let root = v1_root(if cfg.own_server_url.trim().is_empty() { "http://127.0.0.1:8000/v1" } else { &cfg.own_server_url })?;
    let model = cfg.own_server_model.trim().to_string();
    if model.is_empty() {
        return Err(LlmError::Unavailable("Own server model name is missing. Open LLM Runner and enter the served model id.".into()));
    }
    let mut body = json!({
        "model": model,
        "messages": user_messages(req),
        "temperature": req.temperature,
        "top_p": req.top_p,
        "stream": false,
        "max_tokens": i64::from(cfg.own_server_output_limit.clamp(64, 262_144)),
    });
    if let Some(schema) = &req.json_schema {
        body["response_format"] = json!({"type": "json_schema", "json_schema": {"name": "response", "strict": true, "schema": schema}});
    }
    let url = format!("{root}/chat/completions");
    let timeout = timeout_seconds(cfg, 15);
    let data = match request_json(cfg, &url, Some(&body), timeout).await {
        Ok(data) => data,
        Err(LlmError::Request(message)) => {
            let low = message.to_lowercase();
            let room = Regex::new(r"max_tokens \(at most (\d+)").ok().and_then(|r| r.captures(&low).ok().flatten()).or_else(|| Regex::new(r"at most (\d+) here").ok().and_then(|r| r.captures(&low).ok().flatten())).and_then(|c| c.get(1).and_then(|m| m.as_str().parse::<i64>().ok()));
            if let (true, Some(room)) = (low.contains("exceeds the context"), room) {
                let available = room - CONTEXT_TEMPLATE_MARGIN_TOKENS;
                if available < MIN_OUTPUT_TOKENS {
                    return Err(LlmError::Request(message));
                }
                body["max_tokens"] = json!(available);
                request_json(cfg, &url, Some(&body), timeout).await?
            } else if low.contains("max_tokens") && (low.contains("unknown") || low.contains("unsupported") || low.contains("400")) {
                let limit = body["max_tokens"].clone();
                if let Some(map) = body.as_object_mut() {
                    map.remove("max_tokens");
                }
                body["max_completion_tokens"] = limit;
                request_json(cfg, &url, Some(&body), timeout).await?
            } else {
                return Err(LlmError::Request(message));
            }
        }
        Err(other) => return Err(other),
    };
    let text = message_text(&data);
    if text.is_empty() {
        return Err(LlmError::Empty("Own server returned empty text.".into()));
    }
    Ok(ChatReply { text, model, runner: "own_server".into() })
}

/// `_list_own_server_models`: the model ids the server serves, and its `/v1` address.
pub async fn list_models(cfg: &LlmConfig) -> Result<(Vec<String>, String), LlmError> {
    let root = v1_root(if cfg.own_server_url.trim().is_empty() { "http://127.0.0.1:8000/v1" } else { &cfg.own_server_url })?;
    let data = request_json(cfg, &format!("{root}/models"), None, timeout_seconds(cfg, 15).min(45)).await?;
    let mut models = Vec::new();
    for item in data.get("data").and_then(Value::as_array).into_iter().flatten() {
        let id = match item {
            Value::String(s) => s.trim().to_string(),
            Value::Object(o) => o.get("id").or_else(|| o.get("name")).and_then(Value::as_str).unwrap_or("").trim().to_string(),
            _ => String::new(),
        };
        if !id.is_empty() {
            models.push(id);
        }
    }
    Ok((models, root))
}

/// `_own_server_image_size`: each side a multiple of 32 between 256 and 2048.
pub fn image_size(width: Option<f64>, height: Option<f64>) -> String {
    let side = |value: Option<f64>| -> i64 { (((value.unwrap_or(1024.0).trunc()) / 32.0).round() as i64 * 32).clamp(256, 2048) };
    format!("{}x{}", side(width), side(height))
}

/// `_generate_own_server_image`: one picture from the server's `/v1/images/generations`, as a PNG data URL.
pub async fn generate_image(cfg: &LlmConfig, prompt: &str, width: Option<f64>, height: Option<f64>, image_model: &str, seed: Option<i64>, steps: Option<i64>) -> Result<(String, String), LlmError> {
    use base64::Engine;
    let root = v1_root(if cfg.own_server_url.trim().is_empty() { "http://127.0.0.1:8000/v1" } else { &cfg.own_server_url })?;
    if prompt.trim().is_empty() {
        return Err(LlmError::Unavailable("Image prompt is empty.".into()));
    }
    let size = image_size(width, height);
    let mut body = json!({"prompt": prompt.trim(), "size": size, "n": 1, "response_format": "b64_json"});
    if !image_model.trim().is_empty() {
        body["model"] = json!(image_model.trim());
    }
    if let Some(seed) = seed {
        body["seed"] = json!(seed);
    }
    if let Some(steps) = steps.filter(|s| *s != 0) {
        body["steps"] = json!(steps);
    }
    let timeout = timeout_seconds(cfg, 120);
    let data = request_json(cfg, &format!("{root}/images/generations"), Some(&body), timeout).await?;
    let item = data.pointer("/data/0").cloned().unwrap_or(Value::Null);
    if let Some(b64) = item.get("b64_json").and_then(Value::as_str).map(str::trim).filter(|b| !b.is_empty()) {
        return Ok((format!("data:image/png;base64,{b64}"), size));
    }
    if let Some(url) = item.get("url").and_then(Value::as_str).map(str::trim).filter(|u| !u.is_empty()) {
        let mut request = reqwest::Client::new().get(url).timeout(Duration::from_secs(timeout));
        if !cfg.own_server_api_key.trim().is_empty() {
            request = request.bearer_auth(cfg.own_server_api_key.trim());
        }
        let bytes = request.send().await.map_err(|e| LlmError::Request(e.to_string()))?.bytes().await.map_err(|e| LlmError::Request(e.to_string()))?;
        return Ok((format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)), size));
    }
    Err(LlmError::Empty("Custom server returned no image. Check that it supports /v1/images/generations.".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_server_url_becomes_its_v1_address() {
        for (raw, expected) in [
            ("http://127.0.0.1:8000", "http://127.0.0.1:8000/v1"),
            ("http://host/v1/", "http://host/v1"),
            ("https://host/proxy/chat/completions", "https://host/proxy/v1"),
            ("https://host/v1/models?x=1", "https://host/v1"),
            ("HTTPS://Host/api", "https://Host/api/v1"),
        ] {
            assert_eq!(v1_root(raw).unwrap(), expected, "{raw}");
        }
        assert!(v1_root("").unwrap_err().to_string().starts_with("Own server URL is empty."));
        assert_eq!(v1_root("ftp://x").unwrap_err().to_string(), "Own server URL must start with http:// or https://.");
        assert_eq!(v1_root("http:///v1").unwrap_err().to_string(), "Own server URL is missing a host.");
    }

    #[test]
    fn image_sizes_are_multiples_of_32_inside_the_limits() {
        assert_eq!(image_size(None, None), "1024x1024");
        assert_eq!(image_size(Some(100.0), Some(5000.0)), "256x2048");
        assert_eq!(image_size(Some(1000.0), Some(1500.0)), "992x1504");
    }
}
