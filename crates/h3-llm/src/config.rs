//! The LLM runner settings, saved for every project in the app's config file.

use serde::{Deserialize, Serialize};

pub const LM_STUDIO_DEFAULT_BASE_URL: &str = "http://127.0.0.1:1234/v1";

/// `runner` is `lm_studio`, `own_server` (any OpenAI compatible server), `comfyui_gemma`, `llm_api` (the provider is `api_provider`), `openai`, `anthropic` or `google`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LlmConfig {
    /// `lm_studio`, `own_server`, `comfyui_gemma`, `openai`, `anthropic` or `google`.
    pub runner: String,
    pub lm_studio_base_url: String,
    /// The model to prefer when it is loaded. The loaded model is used when it is not.
    pub lm_studio_model: String,
    pub lm_studio_api_key: String,
    pub lm_studio_context_limit: u32,
    pub lm_studio_output_limit: u32,
    /// `off`, `low`, `medium`, `high` or `on`. Empty leaves it to LM Studio.
    pub lm_studio_reasoning: String,
    pub lm_studio_timeout: u32,
    pub own_server_url: String,
    pub own_server_model: String,
    pub own_server_api_key: String,
    pub own_server_timeout: u32,
    pub own_server_output_limit: u32,
    pub api_model: String,
    pub api_key: String,
    /// The hosted API of `llm_api`: `openai`, `anthropic`, `google`, `grok`, `deepseek`, `openrouter` or `apifreellm`.
    pub api_provider: String,
    /// A model name typed by hand. It wins over `api_model`.
    pub api_custom_model: String,
    /// The Gemma GGUF model of `comfyui_gemma`. Empty uses the first one ComfyUI lists.
    pub comfy_gemma_model: String,
    /// The mmproj file that lets Gemma look at pictures. Empty uses the first one ComfyUI lists.
    pub comfy_gemma_mmproj: String,
    pub comfy_gemma_n_ctx: u32,
    /// Keep the model loaded after a request. By default it leaves the GPU, so a render has the memory.
    pub comfy_gemma_keep_loaded: bool,
    pub comfy_gemma_timeout: u32,
    /// The ComfyUI address `comfyui_gemma` talks to. Set from the app's own setting, never saved here.
    #[serde(skip)]
    pub comfy_url: String,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            runner: "lm_studio".into(),
            lm_studio_base_url: LM_STUDIO_DEFAULT_BASE_URL.into(),
            lm_studio_model: String::new(),
            lm_studio_api_key: String::new(),
            lm_studio_context_limit: 32768,
            lm_studio_output_limit: 8192,
            lm_studio_reasoning: String::new(),
            lm_studio_timeout: 180,
            own_server_url: "http://127.0.0.1:8000/v1".into(),
            own_server_model: String::new(),
            own_server_api_key: String::new(),
            own_server_timeout: 360,
            own_server_output_limit: 8192,
            api_model: String::new(),
            api_key: String::new(),
            api_provider: "openai".into(),
            api_custom_model: String::new(),
            comfy_gemma_model: String::new(),
            comfy_gemma_mmproj: String::new(),
            comfy_gemma_n_ctx: 32768,
            comfy_gemma_keep_loaded: false,
            comfy_gemma_timeout: 900,
            comfy_url: String::new(),
        }
    }
}

impl LlmConfig {
    /// The copy the API shows: keys are blanked, and only said to be set.
    pub fn redacted(&self) -> serde_json::Value {
        let mut value = serde_json::to_value(self).unwrap_or_default();
        for key in ["lm_studio_api_key", "own_server_api_key", "api_key"] {
            let set = value.get(key).and_then(|v| v.as_str()).is_some_and(|s| !s.is_empty());
            value[key] = serde_json::json!("");
            value[format!("{key}_set")] = serde_json::json!(set);
        }
        value
    }
}
