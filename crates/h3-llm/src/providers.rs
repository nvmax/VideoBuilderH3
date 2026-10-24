//! The hosted LLM APIs the LLM Runner offers (OpenAI, Anthropic, Google, Grok, DeepSeek, OpenRouter, APIFreeLLM). Port of
//! `VRGDG_LLM_Multi` in `llm/api.py`. The calls send the prompt and the pictures only: like the pack, they pass no
//! temperature or token limits (Anthropic's reply is capped at 1024 tokens).

use std::time::Duration;

use serde_json::{json, Value};

const MODELS: &str = include_str!("../data/llm_api_models.json");

/// The label each provider has in the LLM Runner.
const LABELS: [(&str, &str); 7] = [
    ("openai", "OpenAI"),
    ("anthropic", "Anthropic"),
    ("google", "Google Gemini"),
    ("grok", "Grok"),
    ("deepseek", "DeepSeek"),
    ("openrouter", "OpenRouter"),
    ("apifreellm", "APIFreeLLM"),
];

fn data() -> Value {
    serde_json::from_str(MODELS).unwrap_or(Value::Null)
}

/// `xai` is the same provider as `grok`.
pub fn canonical(provider: &str) -> String {
    let name = provider.trim().to_lowercase();
    if name == "xai" { "grok".into() } else { name }
}

/// `GET /llm_api_choices`: every provider with its models and default model.
pub fn choices() -> Value {
    let data = data();
    let models = data.get("providers").and_then(Value::as_object).cloned().unwrap_or_default();
    let defaults = data.get("default_model").and_then(Value::as_object).cloned().unwrap_or_default();
    let providers: Vec<Value> = models
        .iter()
        .filter(|(id, _)| id.as_str() != "xai" && !id.trim().is_empty())
        .map(|(id, list)| {
            let list: Vec<String> = list.as_array().map(|a| a.iter().filter_map(Value::as_str).map(|m| m.trim().to_string()).filter(|m| !m.is_empty()).collect()).unwrap_or_default();
            let label = LABELS.iter().find(|(key, _)| key == id).map_or(id.as_str(), |(_, label)| label);
            let default = defaults.get(id).and_then(Value::as_str).map(str::to_string).or_else(|| list.first().cloned()).unwrap_or_default();
            json!({"id": id, "label": label, "models": list, "default_model": default})
        })
        .collect();
    json!({"providers": providers})
}

fn provider_models(provider: &str) -> Vec<String> {
    data().pointer(&format!("/providers/{provider}")).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default()
}

fn default_model(provider: &str) -> Option<String> {
    data().pointer(&format!("/default_model/{provider}")).and_then(Value::as_str).map(str::to_string)
}

/// `_resolve_model`: the custom model, else the chosen one when the provider lists it, else the provider's default.
pub fn resolve_model(provider: &str, selected: &str, custom: &str) -> Result<String, String> {
    let provider = canonical(provider);
    if !custom.trim().is_empty() {
        return Ok(custom.trim().to_string());
    }
    let allowed = provider_models(&provider);
    if allowed.iter().any(|m| m == selected) {
        return Ok(selected.to_string());
    }
    if let Some(default) = default_model(&provider) {
        return Ok(default);
    }
    allowed.first().cloned().ok_or_else(|| format!("No models configured for provider '{provider}'"))
}

/// What a call answered with.
#[derive(Debug, Clone, PartialEq)]
pub struct ApiReply {
    pub text: String,
    pub provider: String,
    pub model: String,
}

/// `_normalize_error`.
fn normalize_error(provider: &str, message: &str) -> String {
    let low = message.to_lowercase();
    if provider == "deepseek" && low.contains("http 402") && low.contains("insufficient balance") {
        return "error: DeepSeek account has insufficient balance (HTTP 402). Add credits or switch provider/model.".into();
    }
    if matches!(provider, "xai" | "grok") && low.contains("http 403") && low.contains("1010") {
        return "error: xAI request blocked (HTTP 403 code 1010). Usually account/region/firewall edge blocking. Verify xAI API access, try a different network/IP, or use OpenRouter with a Grok model.".into();
    }
    format!("error: {message}")
}

async fn post_json(url: &str, headers: &[(&str, String)], body: &Value, timeout: u64) -> Result<Value, String> {
    let mut request = reqwest::Client::new().post(url).timeout(Duration::from_secs(timeout)).header("Accept", "application/json").header("User-Agent", "VRGDG-LLM-Multi/1.0 (+ComfyUI)");
    for (name, value) in headers {
        request = request.header(*name, value);
    }
    let response = request.json(body).send().await.map_err(|e| format!("Network error: {e}"))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("HTTP {}: {text}", status.as_u16()));
    }
    serde_json::from_str(&text).map_err(|e| format!("Network error: reply is not JSON: {e}"))
}

/// The text of an OpenAI style reply: a string, or a list of text parts.
fn openai_text(data: &Value) -> Result<String, String> {
    let choices = data.get("choices").and_then(Value::as_array).filter(|c| !c.is_empty()).ok_or_else(|| format!("No choices in response: {data}"))?;
    let content = choices[0].pointer("/message/content").cloned().unwrap_or(Value::Null);
    let text = match content {
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_object)
            .map(|item| {
                let kind = item.get("type").and_then(Value::as_str).unwrap_or("");
                if matches!(kind, "text" | "output_text") {
                    item.get("text").and_then(Value::as_str).unwrap_or("").to_string()
                } else {
                    item.get("content").and_then(Value::as_str).unwrap_or("").to_string()
                }
            })
            .collect::<String>(),
        Value::String(s) => s,
        Value::Null => String::new(),
        other => other.to_string(),
    };
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err(format!("Empty model text response: {data}"));
    }
    Ok(text)
}

async fn openai_compatible(base: &str, key: &str, model: &str, prompt: &str, images: &[String], extra: &[(&str, String)]) -> Result<String, String> {
    let content = if images.is_empty() {
        json!(prompt)
    } else {
        let mut parts = vec![json!({"type": "text", "text": prompt})];
        parts.extend(images.iter().map(|url| json!({"type": "image_url", "image_url": {"url": url}})));
        Value::Array(parts)
    };
    let body = json!({"model": model, "messages": [{"role": "user", "content": content}]});
    let mut headers = vec![("Authorization", format!("Bearer {key}")), ("Content-Type", "application/json".to_string())];
    headers.extend(extra.iter().map(|(n, v)| (*n, v.clone())));
    openai_text(&post_json(&format!("{base}/chat/completions"), &headers, &body, 90).await?)
}

/// `(media type, base64)` of a `data:` URL.
fn split_data_url(url: &str) -> (String, String) {
    match url.strip_prefix("data:").and_then(|rest| rest.split_once(',')) {
        Some((meta, data)) => (meta.split(';').next().unwrap_or("image/png").to_string(), data.to_string()),
        None => ("image/png".into(), url.to_string()),
    }
}

async fn anthropic(key: &str, model: &str, prompt: &str, images: &[String]) -> Result<String, String> {
    let mut content = vec![json!({"type": "text", "text": prompt})];
    for url in images {
        let (media, data) = split_data_url(url);
        content.push(json!({"type": "image", "source": {"type": "base64", "media_type": media, "data": data}}));
    }
    let body = json!({"model": model, "max_tokens": 1024, "messages": [{"role": "user", "content": content}]});
    let headers = [("x-api-key", key.to_string()), ("anthropic-version", "2023-06-01".to_string()), ("content-type", "application/json".to_string())];
    let data = post_json("https://api.anthropic.com/v1/messages", &headers, &body, 90).await?;
    let text: String = data.get("content").and_then(Value::as_array).map(|items| items.iter().filter(|i| i.get("type").and_then(Value::as_str) == Some("text")).filter_map(|i| i.get("text").and_then(Value::as_str)).collect()).unwrap_or_default();
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err(format!("Empty Anthropic response: {data}"));
    }
    Ok(text)
}

async fn google(key: &str, model: &str, prompt: &str, images: &[String]) -> Result<String, String> {
    let mut parts: Vec<Value> = images
        .iter()
        .map(|url| {
            let (media, data) = split_data_url(url);
            json!({"inlineData": {"mimeType": media, "data": data}})
        })
        .collect();
    if !prompt.trim().is_empty() {
        parts.push(json!({"text": prompt}));
    }
    let url = format!("https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent", model.trim());
    let body = json!({"contents": [{"role": "user", "parts": parts}]});
    let data = post_json(&url, &[("x-goog-api-key", key.trim().to_string()), ("Content-Type", "application/json".to_string())], &body, 120).await.map_err(|e| e.replacen("HTTP", "Google REST HTTP", 1))?;
    let text: String = data
        .get("candidates")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|c| c.pointer("/content/parts").and_then(Value::as_array))
        .flatten()
        .filter_map(|p| p.get("text").and_then(Value::as_str))
        .collect();
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("Empty Google response".into());
    }
    Ok(text)
}

async fn apifreellm(key: &str, model: &str, prompt: &str) -> Result<String, String> {
    let mut body = json!({"message": prompt});
    if !model.trim().is_empty() {
        body["model"] = json!(model.trim());
    }
    let headers = [("Authorization", format!("Bearer {key}")), ("Content-Type", "application/json".to_string())];
    let data = post_json("https://apifreellm.com/api/v1/chat", &headers, &body, 90).await?;
    for name in ["response", "message", "text", "output", "content"] {
        match data.get(name) {
            Some(Value::String(s)) if !s.trim().is_empty() => return Ok(s.trim().to_string()),
            Some(Value::Object(o)) => {
                if let Some(text) = o.get("content").and_then(Value::as_str).filter(|t| !t.trim().is_empty()) {
                    return Ok(text.trim().to_string());
                }
            }
            _ => {}
        }
    }
    if let Some(text) = data.pointer("/choices/0/message/content").and_then(Value::as_str).filter(|t| !t.trim().is_empty()) {
        return Ok(text.trim().to_string());
    }
    Err(format!("Unexpected apifreellm response format: {data}"))
}

/// `generate_text` for one provider. `images` are `data:` URLs. A failure reads `error: ...` like the pack's status text.
pub async fn generate(provider: &str, selected_model: &str, custom_model: &str, api_key: &str, prompt: &str, images: &[String]) -> Result<ApiReply, String> {
    let provider = canonical(provider);
    if api_key.trim().is_empty() {
        return Err("API key missing".into());
    }
    if prompt.trim().is_empty() {
        return Err("Prompt is empty".into());
    }
    if provider_models(&provider).is_empty() {
        return Err(format!("Unsupported provider: {provider}"));
    }
    let model = resolve_model(&provider, selected_model, custom_model)?;
    let key = api_key.trim();
    let result = match provider.as_str() {
        "openai" => openai_compatible("https://api.openai.com/v1", key, &model, prompt, images, &[]).await,
        "grok" => openai_compatible("https://api.x.ai/v1", key, &model, prompt, images, &[]).await,
        "deepseek" => openai_compatible("https://api.deepseek.com", key, &model, prompt, images, &[]).await,
        "openrouter" => openai_compatible("https://openrouter.ai/api/v1", key, &model, prompt, images, &[("HTTP-Referer", "https://localhost".to_string()), ("X-Title", "VRGDG_LLM_Multi".to_string())]).await,
        "anthropic" => anthropic(key, &model, prompt, images).await,
        "google" => google(key, &model, prompt, images).await,
        _ => apifreellm(key, &model, prompt).await,
    };
    match result {
        Ok(text) => Ok(ApiReply { text, provider, model }),
        Err(message) => Err(normalize_error(&provider, &message)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_choices_list_every_provider_with_a_default() {
        let all = choices();
        let providers = all["providers"].as_array().unwrap();
        let ids: Vec<&str> = providers.iter().map(|p| p["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["openai", "anthropic", "google", "grok", "deepseek", "openrouter", "apifreellm"]);
        let openai = &providers[0];
        assert_eq!(openai["label"], "OpenAI");
        assert_eq!(openai["default_model"], "gpt-6-luna");
        assert!(openai["models"].as_array().unwrap().iter().any(|m| m == "gpt-4o"));
        assert!(providers[5]["models"].as_array().unwrap().len() > 100);
        assert_eq!(providers[6]["default_model"], "apifreellm");
    }

    #[test]
    fn a_model_is_the_custom_one_the_chosen_one_or_the_default() {
        assert_eq!(resolve_model("openai", "gpt-4o", "").unwrap(), "gpt-4o");
        assert_eq!(resolve_model("openai", "gpt-4o", "my-model ").unwrap(), "my-model");
        assert_eq!(resolve_model("openai", "nope", "").unwrap(), "gpt-6-luna");
        assert_eq!(resolve_model("xai", "", "").unwrap(), "grok-4.6");
        assert_eq!(canonical(" XAI "), "grok");
    }

    #[test]
    fn errors_read_like_the_packs_status_text() {
        assert_eq!(normalize_error("deepseek", "HTTP 402: {\"error\":\"Insufficient Balance\"}"), "error: DeepSeek account has insufficient balance (HTTP 402). Add credits or switch provider/model.");
        assert!(normalize_error("grok", "HTTP 403: error code: 1010").contains("blocked (HTTP 403 code 1010)"));
        assert_eq!(normalize_error("openai", "HTTP 401: bad key"), "error: HTTP 401: bad key");
    }

    #[test]
    fn openai_replies_are_read_from_a_string_or_parts() {
        assert_eq!(openai_text(&json!({"choices": [{"message": {"content": " hi "}}]})).unwrap(), "hi");
        assert_eq!(openai_text(&json!({"choices": [{"message": {"content": [{"type": "text", "text": "a"}, {"type": "output_text", "text": "b"}, {"content": "c"}]}}]})).unwrap(), "abc");
        assert!(openai_text(&json!({"choices": []})).unwrap_err().starts_with("No choices in response"));
        assert!(openai_text(&json!({"choices": [{"message": {"content": ""}}]})).unwrap_err().starts_with("Empty model text response"));
    }
}
