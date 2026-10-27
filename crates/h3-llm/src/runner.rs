//! The LLM Runner of the Video Builder: which runner a request uses, how its limits are read, and running a text or picture
//! prompt on it. Port of `llm/builder_runner.py`.
//!
//! Every Builder request carries the runner settings in its body (`text_runner`, `lmstudio_*`, `own_server_*`,
//! `llm_api_*`, ...). They are turned into an [`LlmConfig`] for each call. The pack's "Gemma Local" and "Qwen Local" run a
//! model inside the pack. Here both run the Gemma GGUF in ComfyUI. LM Studio is used through the model it has loaded.

use h3_core::pyval::{text_or_empty, to_int, truthy, Obj};
use serde_json::Value;

use crate::checks::clean_visual_text;
use crate::clean::{clean_plain_text, strip_thinking_text};
use crate::client::{chat, ChatRequest, LlmError};
use crate::config::{LlmConfig, LM_STUDIO_DEFAULT_BASE_URL};

const OWN_SERVER_DEFAULT_BASE_URL: &str = "http://127.0.0.1:8000/v1";
/// Gemma Local fails with a smaller window, so its context and output limits never go below this.
pub const GEMMA_LOCAL_MIN_TOKENS: i64 = 16384;

/// What a call used, for the replies that report it.
#[derive(Debug, Clone, PartialEq)]
pub struct RunInfo {
    pub runner: String,
    pub used_model: String,
    pub used_provider: String,
    /// The model left the GPU after the call (the Gemma in ComfyUI, unless it is kept loaded).
    pub unloaded: bool,
}

fn text(payload: &Obj, key: &str) -> String {
    text_or_empty(payload.get(key)).trim().to_string()
}

/// The first of the keys that has text.
fn first_text(payload: &Obj, keys: &[&str]) -> String {
    keys.iter().map(|k| text(payload, k)).find(|t| !t.is_empty()).unwrap_or_default()
}

/// The first of the keys that is set at all (a value other than null or empty text).
fn first_set<'a>(payload: &'a Obj, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().filter_map(|k| payload.get(*k)).find(|v| !v.is_null() && v.as_str() != Some(""))
}

/// `_llm_runner_from_payload`: `builtin`, `qwen_local`, `lm_studio`, `llm_api` or `own_server`.
pub fn runner_kind(payload: &Obj) -> &'static str {
    let raw = first_text(payload, &["text_runner", "text_gemma_runner", "gemma_runner"]).to_lowercase();
    let raw = if raw.is_empty() { "builtin".to_string() } else { raw };
    match raw.as_str() {
        "lmstudio" | "lm-studio" | "lm_studio" => "lm_studio",
        "llmapi" | "llm-api" | "llm_api" | "api" | "outside_api" | "outside_llm_api" => "llm_api",
        "ownserver" | "own-server" | "own_server" | "custom_openai" | "openai_compatible" | "custom_server" | "my_server" => "own_server",
        "qwen" | "qwen_local" | "qwen-local" | "qwen_gguf" | "qwen_gguf_local" => "qwen_local",
        _ => "builtin",
    }
}

/// `_llm_runner_display_name`.
pub fn display_name(payload: &Obj, vision: bool) -> String {
    let name = match runner_kind(payload) {
        "qwen_local" => "Qwen Local",
        "lm_studio" => "LM Studio",
        "llm_api" => "LLM API",
        "own_server" => "Custom Server",
        _ => "Gemma Local",
    };
    if vision { format!("{name} vision") } else { name.to_string() }
}

/// `_normalized_token_limit`: `int(value)`, the default when it is not a number, kept between the limits.
fn token_limit(value: Option<&Value>, default: i64, minimum: i64, maximum: i64) -> i64 {
    value.and_then(to_int).unwrap_or(default).clamp(minimum, maximum)
}

/// `_gemma_local_context_limit`.
fn gemma_context_limit(payload: &Obj) -> i64 {
    token_limit(payload.get("n_ctx"), GEMMA_LOCAL_MIN_TOKENS, GEMMA_LOCAL_MIN_TOKENS, 262_144)
}

/// `_runner_output_token_limit`: the output limit the user set for the runner, else what the caller asks for.
pub fn output_token_limit(payload: &Obj, requested: i64) -> i64 {
    let kind = runner_kind(payload);
    let mut configured = match kind {
        "lm_studio" => first_set(payload, &["lmstudio_output_token_limit", "lm_studio_output_token_limit"]),
        "own_server" => first_set(payload, &["own_server_output_token_limit"]),
        "builtin" => first_set(payload, &["gemma_output_token_limit"]),
        _ => None,
    };
    if matches!(kind, "builtin" | "lm_studio" | "own_server") && configured.is_none() {
        configured = first_set(payload, &["llm_max_tokens"]);
    }
    if kind == "builtin" {
        let context = gemma_context_limit(payload);
        let value = configured.and_then(to_int).unwrap_or(requested);
        return value.clamp(GEMMA_LOCAL_MIN_TOKENS, context);
    }
    match configured {
        Some(_) => token_limit(configured, requested, 64, 262_144),
        None => token_limit(Some(&Value::from(requested)), 1200, 64, 262_144),
    }
}

/// `_lm_studio_reasoning_mode` without a default.
fn lm_studio_reasoning(payload: &Obj) -> String {
    let configured = first_text(payload, &["lmstudio_reasoning", "lm_studio_reasoning"]).to_lowercase();
    if matches!(configured.as_str(), "off" | "low" | "medium" | "high" | "on") { configured } else { String::new() }
}

/// `_own_server_timeout`: seconds, between 15 and 600.
fn own_server_timeout(payload: &Obj) -> u32 {
    let value = payload.get("own_server_timeout").filter(|v| truthy(v)).and_then(h3_core::pyval::to_float).unwrap_or(360.0);
    value.clamp(15.0, 600.0) as u32
}

/// `normalize_gpu_device`: the GPU number the local model goes on (as ComfyUI numbers its GPUs), or -1 for automatic when it is missing or not a number from 0 up.
fn gpu_device(payload: &Obj) -> i32 {
    payload.get("gpu_device").and_then(to_int).filter(|number| *number >= 0).map_or(-1, |number| number.min(63) as i32)
}

/// The configuration a request runs with, from the runner settings in its body. `requested` is the output size the caller
/// asks for, used when the runner has no limit of its own.
pub fn runner_config(payload: &Obj, base: &LlmConfig, requested: i64) -> LlmConfig {
    let mut cfg = base.clone();
    let output = output_token_limit(payload, requested).clamp(0, i64::from(u32::MAX)) as u32;
    match runner_kind(payload) {
        "lm_studio" => {
            cfg.runner = "lm_studio".into();
            let url = text(payload, "lmstudio_base_url");
            cfg.lm_studio_base_url = if url.is_empty() { LM_STUDIO_DEFAULT_BASE_URL.into() } else { url };
            cfg.lm_studio_model = first_text(payload, &["lmstudio_model", "model_file"]);
            cfg.lm_studio_api_key = text(payload, "lmstudio_api_key");
            cfg.lm_studio_context_limit = token_limit(first_set(payload, &["lmstudio_context_limit", "lm_studio_context_limit"]), 32768, 512, 262_144) as u32;
            cfg.lm_studio_output_limit = output;
            cfg.lm_studio_reasoning = lm_studio_reasoning(payload);
            cfg.lm_studio_timeout = payload.get("lmstudio_timeout").filter(|v| truthy(v)).and_then(to_int).unwrap_or(180).max(5) as u32;
        }
        "own_server" => {
            cfg.runner = "own_server".into();
            let url = first_text(payload, &["own_server_url", "base_url"]);
            cfg.own_server_url = if url.is_empty() { OWN_SERVER_DEFAULT_BASE_URL.into() } else { url };
            cfg.own_server_model = first_text(payload, &["own_server_model", "model", "model_file"]);
            cfg.own_server_api_key = first_text(payload, &["own_server_api_key", "own_server_key", "api_key", "own_server_api_key_project"]);
            cfg.own_server_timeout = own_server_timeout(payload);
            cfg.own_server_output_limit = output;
        }
        "llm_api" => {
            cfg.runner = "llm_api".into();
            let provider = first_text(payload, &["llm_api_provider", "provider"]);
            cfg.api_provider = crate::providers::canonical(if provider.is_empty() { "openai" } else { &provider });
            cfg.api_model = first_text(payload, &["llm_api_model", "model"]);
            cfg.api_custom_model = first_text(payload, &["llm_api_custom_model", "custom_model"]);
            cfg.api_key = first_text(payload, &["llm_api_key", "api_key", "llm_api_key_project"]);
        }
        kind => {
            cfg.runner = "comfyui_gemma".into();
            cfg.comfy_gemma_model = if kind == "qwen_local" { first_text(payload, &["qwen_model_file", "model_file"]) } else { first_text(payload, &["gemma_model_file", "model_file"]) };
            cfg.comfy_gemma_mmproj = first_text(payload, &["mmproj_file", "gemma_mmproj_file", "qwen_mmproj_file"]);
            cfg.comfy_gemma_n_ctx = gemma_context_limit(payload) as u32;
            cfg.comfy_gemma_gpu_device = gpu_device(payload);
        }
    }
    cfg
}

fn info(cfg: &LlmConfig, kind: &str, reply_model: String, vision: bool) -> RunInfo {
    let runner = match (kind, vision) {
        ("lm_studio", true) => "lm_studio_vision".to_string(),
        ("llm_api", true) => "llm_api_vision".to_string(),
        ("own_server", true) => "own_server_vision".to_string(),
        (other, _) => other.to_string(),
    };
    let used_provider = match kind {
        "llm_api" => cfg.api_provider.clone(),
        "own_server" => "own_server".to_string(),
        _ => String::new(),
    };
    RunInfo { runner, used_model: reply_model, used_provider, unloaded: matches!(kind, "builtin" | "qwen_local") && !cfg.comfy_gemma_keep_loaded }
}

/// Run a text prompt on the runner the request selected and return the cleaned reply.
///
/// With `json_schema` the reply is the raw JSON text (the prose cleaners would cut it apart). LM Studio and the custom
/// server are held to the schema. The Gemma in ComfyUI cannot be, so its reply is also returned raw for the caller to read
/// with its JSON repair. `preserve_paragraphs` keeps the whole reply instead of its first paragraph.
#[allow(clippy::too_many_arguments)]
pub async fn run_text(
    payload: &Obj,
    base: &LlmConfig,
    instruction: &str,
    temperature: f64,
    top_p: f64,
    max_new_tokens: u32,
    preserve_paragraphs: bool,
    json_schema: Option<Value>,
) -> Result<(String, RunInfo), LlmError> {
    let kind = runner_kind(payload);
    let cfg = runner_config(payload, base, i64::from(max_new_tokens));
    let output = output_token_limit(payload, i64::from(max_new_tokens)).clamp(32, 262_144) as u32;
    let constrained = json_schema.is_some() && kind != "llm_api";
    let mut request = ChatRequest::text(instruction, temperature, top_p, output);
    if constrained && matches!(kind, "lm_studio" | "own_server") {
        request.json_schema = json_schema;
    }
    request.seed = payload.get("seed").and_then(to_int);
    let reply = chat(&cfg, &request).await?;
    let cleaned = if constrained {
        reply.text.trim().to_string()
    } else if preserve_paragraphs {
        clean_plain_text(&reply.text)
    } else {
        clean_visual_text(&reply.text)
    };
    Ok((cleaned, info(&cfg, kind, reply.model, false)))
}

/// Run a prompt about pictures on the selected runner. The reply is raw: callers clean it. Each runner gets the pictures the
/// way the pack sends them: LM Studio small JPEGs (512 high), a custom server and Gemma larger ones (1024 high), a hosted API
/// the first four as they are.
pub async fn run_vision(payload: &Obj, base: &LlmConfig, instruction: &str, images: &[image::DynamicImage], temperature: f64, top_p: f64, max_new_tokens: u32) -> Result<(String, RunInfo), LlmError> {
    let kind = runner_kind(payload);
    let cfg = runner_config(payload, base, i64::from(max_new_tokens));
    if images.is_empty() {
        return Err(LlmError::Unavailable(match kind {
            "llm_api" => "LLM API vision needs at least one image reference.".to_string(),
            "own_server" => "Own server vision needs at least one image reference.".to_string(),
            _ => "LM Studio vision needs at least one image.".to_string(),
        }));
    }
    if kind == "llm_api" {
        if cfg.api_key.is_empty() {
            return Err(LlmError::Unavailable("LLM API key is missing. Open LLM Runner and paste your API key.".into()));
        }
        if cfg.api_model.is_empty() && cfg.api_custom_model.is_empty() {
            return Err(LlmError::Unavailable("LLM API vision model is missing. Open LLM Runner and select a vision-capable API model before running Video All with image reference.".into()));
        }
        if matches!(cfg.api_provider.as_str(), "deepseek" | "apifreellm") {
            return Err(LlmError::Unavailable(format!("{} is not configured for image/vision input here. Choose a vision-capable LLM API provider/model, or use LM Studio/Gemma Local vision.", cfg.api_provider)));
        }
    }
    let urls = match kind {
        "llm_api" => images.iter().take(4).map(png_data_url).collect::<Result<Vec<_>, _>>()?,
        "own_server" => images.iter().take(4).map(|i| image_data_url(i, 1024, 90)).collect::<Result<Vec<_>, _>>()?,
        "lm_studio" => images.iter().map(|i| image_data_url(i, 512, 88)).collect::<Result<Vec<_>, _>>()?,
        _ => images.iter().map(|i| image_data_url(i, 1024, 90)).collect::<Result<Vec<_>, _>>()?,
    };
    let output = output_token_limit(payload, i64::from(max_new_tokens)).clamp(32, 262_144) as u32;
    let mut request = ChatRequest::text(instruction, temperature, top_p, output).with_images(urls);
    request.seed = payload.get("seed").and_then(to_int);
    let reply = chat(&cfg, &request).await?;
    Ok((reply.text, info(&cfg, kind, reply.model, true)))
}

/// `_strip_builder_thinking_text` for callers that clean vision replies.
pub fn strip_thinking(text: &str) -> String {
    strip_thinking_text(text)
}

/// `data:image/jpeg;base64,...` of an image at most `max_height` high. Port of `_pil_image_to_data_url`.
pub fn image_data_url(image: &image::DynamicImage, max_height: u32, quality: u8) -> Result<String, LlmError> {
    use base64::Engine;
    let rgb = image.to_rgb8();
    let rgb = if rgb.height() > max_height {
        let width = ((f64::from(rgb.width())) * (f64::from(max_height) / f64::from(rgb.height().max(1)))).floor().max(1.0) as u32;
        image::imageops::resize(&rgb, width, max_height, image::imageops::FilterType::Lanczos3)
    } else {
        rgb
    };
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, quality).encode_image(&rgb).map_err(|e| LlmError::Request(format!("A picture could not be prepared: {e}")))?;
    Ok(format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}

/// `data:image/png;base64,...` of an image at full size.
pub fn png_data_url(image: &image::DynamicImage) -> Result<String, LlmError> {
    use base64::Engine;
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.to_rgb8().write_to(&mut bytes, image::ImageFormat::Png).map_err(|e| LlmError::Request(format!("A picture could not be prepared: {e}")))?;
    Ok(format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes.into_inner())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn payload(value: Value) -> Obj {
        value.as_object().cloned().unwrap()
    }

    #[test]
    fn the_runner_name_is_read_like_the_pack() {
        for (name, kind) in [("LM-Studio", "lm_studio"), ("api", "llm_api"), ("custom_server", "own_server"), ("qwen", "qwen_local"), ("nonsense", "builtin"), ("", "builtin")] {
            assert_eq!(runner_kind(&payload(json!({"text_runner": name}))), kind, "{name}");
        }
        assert_eq!(runner_kind(&payload(json!({}))), "builtin");
        assert_eq!(runner_kind(&payload(json!({"gemma_runner": "lmstudio"}))), "lm_studio");
        assert_eq!(display_name(&payload(json!({"text_runner": "own_server"})), true), "Custom Server vision");
        assert_eq!(display_name(&payload(json!({})), false), "Gemma Local");
    }

    #[test]
    fn output_limits_follow_the_runner() {
        let lm = payload(json!({"text_runner": "lm_studio", "lmstudio_output_token_limit": 8192}));
        assert_eq!(output_token_limit(&lm, 1200), 8192);
        assert_eq!(output_token_limit(&payload(json!({"text_runner": "lm_studio"})), 1200), 1200);
        assert_eq!(output_token_limit(&payload(json!({"text_runner": "lm_studio", "llm_max_tokens": "300"})), 1200), 300);
        assert_eq!(output_token_limit(&payload(json!({"text_runner": "lm_studio", "lmstudio_output_token_limit": 5})), 1200), 64);
        assert_eq!(output_token_limit(&payload(json!({"text_runner": "llm_api"})), 99_999_999), 262_144);
        // Gemma Local output never goes below 16384 or above its context.
        assert_eq!(output_token_limit(&payload(json!({})), 1200), 16384);
        assert_eq!(output_token_limit(&payload(json!({"gemma_output_token_limit": 20000, "n_ctx": 32768})), 1200), 20000);
        assert_eq!(output_token_limit(&payload(json!({"gemma_output_token_limit": 90000, "n_ctx": 32768})), 1200), 32768);
    }

    #[test]
    fn the_settings_of_each_runner_land_in_the_config() {
        let base = LlmConfig { comfy_url: "http://127.0.0.1:8188".into(), ..LlmConfig::default() };
        let lm = runner_config(&payload(json!({"text_runner": "lm_studio", "lmstudio_base_url": "http://h:1/v1", "lmstudio_model": "m", "lmstudio_context_limit": 100, "lmstudio_reasoning": "HIGH", "lmstudio_output_token_limit": 4000})), &base, 1200);
        assert_eq!((lm.runner.as_str(), lm.lm_studio_base_url.as_str(), lm.lm_studio_model.as_str(), lm.lm_studio_context_limit, lm.lm_studio_output_limit, lm.lm_studio_reasoning.as_str()), ("lm_studio", "http://h:1/v1", "m", 512, 4000, "high"));
        let own = runner_config(&payload(json!({"text_runner": "own_server", "own_server_url": " https://x/v1 ", "own_server_model": "gpt", "own_server_key": "k", "own_server_timeout": 5})), &base, 1200);
        assert_eq!((own.runner.as_str(), own.own_server_url.as_str(), own.own_server_model.as_str(), own.own_server_api_key.as_str(), own.own_server_timeout, own.own_server_output_limit), ("own_server", "https://x/v1", "gpt", "k", 15, 1200));
        let api = runner_config(&payload(json!({"text_runner": "llm_api", "llm_api_provider": "xAI", "llm_api_model": "grok-4.6", "llm_api_key": "k"})), &base, 1200);
        assert_eq!((api.runner.as_str(), api.api_provider.as_str(), api.api_model.as_str(), api.api_key.as_str()), ("llm_api", "grok", "grok-4.6", "k"));
        let gemma = runner_config(&payload(json!({"gemma_model_file": "g.gguf", "mmproj_file": "p.gguf"})), &base, 1200);
        assert_eq!((gemma.runner.as_str(), gemma.comfy_gemma_model.as_str(), gemma.comfy_gemma_mmproj.as_str(), gemma.comfy_gemma_n_ctx, gemma.comfy_url.as_str()), ("comfyui_gemma", "g.gguf", "p.gguf", 16384, "http://127.0.0.1:8188"));
    }

    #[test]
    fn the_gpu_of_a_local_model_comes_from_the_request_and_is_automatic_without_one() {
        let base = LlmConfig::default();
        let gpu = |value: Value| runner_config(&payload(value), &base, 1200).comfy_gemma_gpu_device;
        assert_eq!(gpu(json!({"gemma_model_file": "g.gguf"})), -1, "no choice is automatic");
        assert_eq!(gpu(json!({"gemma_model_file": "g.gguf", "gpu_device": 1})), 1);
        assert_eq!(gpu(json!({"text_runner": "qwen_local", "qwen_model_file": "q.gguf", "gpu_device": "0"})), 0, "Qwen Local takes it too, and a number may come as text");
        assert_eq!(gpu(json!({"gemma_model_file": "g.gguf", "gpu_device": -1})), -1);
        assert_eq!(gpu(json!({"gemma_model_file": "g.gguf", "gpu_device": "auto"})), -1);
        assert_eq!(gpu(json!({"gemma_model_file": "g.gguf", "gpu_device": null})), -1);
        assert_eq!(gpu(json!({"gemma_model_file": "g.gguf", "gpu_device": 500})), 63, "a number is kept inside the range llama.cpp takes");
        assert_eq!(gpu(json!({"text_runner": "lm_studio", "gpu_device": 1})), -1, "a server runner has no GPU of its own to choose");
    }

    #[tokio::test]
    async fn vision_on_a_hosted_api_needs_a_key_a_model_and_a_capable_provider() {
        let base = LlmConfig::default();
        let one = || vec![image::DynamicImage::ImageRgb8(image::RgbImage::new(2, 2))];
        let run = |value: Value, images: Vec<image::DynamicImage>| {
            let base = base.clone();
            async move { run_vision(&payload(value), &base, "look", &images, 0.2, 0.9, 100).await.unwrap_err().to_string() }
        };
        assert_eq!(run(json!({"text_runner": "llm_api"}), vec![]).await, "LLM API vision needs at least one image reference.");
        assert_eq!(run(json!({"text_runner": "llm_api"}), one()).await, "LLM API key is missing. Open LLM Runner and paste your API key.");
        assert!(run(json!({"text_runner": "llm_api", "llm_api_key": "k"}), one()).await.starts_with("LLM API vision model is missing."));
        assert!(run(json!({"text_runner": "llm_api", "llm_api_key": "k", "llm_api_model": "deepseek-chat", "llm_api_provider": "deepseek"}), one()).await.starts_with("deepseek is not configured for image/vision input here."));
    }

    #[test]
    fn pictures_become_small_jpeg_data_urls() {
        let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(200, 1000, image::Rgb([10, 200, 30])));
        let url = image_data_url(&image, 500, 88).unwrap();
        assert!(url.starts_with("data:image/jpeg;base64,"));
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD.decode(url.trim_start_matches("data:image/jpeg;base64,")).unwrap();
        let back = image::load_from_memory(&bytes).unwrap();
        assert_eq!((back.width(), back.height()), (100, 500));
    }
}
