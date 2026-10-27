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
    /// The GPU of the local model, as ComfyUI numbers its GPUs: `-1` lets llama.cpp use every GPU ComfyUI sees, `0` and up puts the whole model on that one.
    pub comfy_gemma_gpu_device: i32,
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
            comfy_gemma_gpu_device: -1,
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

    /// The runner settings of this configuration as a Builder request carries them (`text_runner`, `lmstudio_*`, `own_server_*`, `llm_api_*`, ...), so a
    /// writer that reads its runner from the request runs on the LLM the app is set to use. `runner_config` turns them back into this configuration.
    pub fn runner_payload(&self) -> serde_json::Map<String, serde_json::Value> {
        use serde_json::json;
        let fields = match self.runner.as_str() {
            "lm_studio" => json!({
                "text_runner": "lm_studio", "lmstudio_base_url": self.lm_studio_base_url, "lmstudio_model": self.lm_studio_model, "lmstudio_api_key": self.lm_studio_api_key,
                "lmstudio_context_limit": self.lm_studio_context_limit, "lmstudio_output_token_limit": self.lm_studio_output_limit,
                "lmstudio_reasoning": self.lm_studio_reasoning, "lmstudio_timeout": self.lm_studio_timeout,
            }),
            "own_server" => json!({
                "text_runner": "own_server", "own_server_url": self.own_server_url, "own_server_model": self.own_server_model, "own_server_api_key": self.own_server_api_key,
                "own_server_timeout": self.own_server_timeout, "own_server_output_token_limit": self.own_server_output_limit,
            }),
            "comfyui_gemma" => json!({"text_runner": "builtin", "gemma_model_file": self.comfy_gemma_model, "mmproj_file": self.comfy_gemma_mmproj, "n_ctx": self.comfy_gemma_n_ctx, "gpu_device": self.comfy_gemma_gpu_device}),
            hosted => json!({
                "text_runner": "llm_api", "llm_api_provider": if hosted == "llm_api" { self.api_provider.as_str() } else { hosted },
                "llm_api_model": self.api_model, "llm_api_custom_model": self.api_custom_model, "llm_api_key": self.api_key,
            }),
        };
        fields.as_object().cloned().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::{runner_config, runner_kind};

    /// Every runner comes back from its payload as the same configuration.
    #[test]
    fn a_configuration_survives_the_request_it_is_carried_in() {
        let base = LlmConfig { comfy_url: "http://127.0.0.1:8188".into(), ..LlmConfig::default() };
        let lm_studio = LlmConfig { runner: "lm_studio".into(), lm_studio_base_url: "http://box:1234/v1".into(), lm_studio_model: "gemma-4".into(), lm_studio_api_key: "k".into(), lm_studio_context_limit: 16384, lm_studio_output_limit: 6000, lm_studio_reasoning: "off".into(), lm_studio_timeout: 99, ..base.clone() };
        let back = runner_config(&lm_studio.runner_payload(), &base, 1200);
        assert_eq!(back, lm_studio);

        let own = LlmConfig { runner: "own_server".into(), own_server_url: "http://box:8000/v1".into(), own_server_model: "m".into(), own_server_api_key: "k".into(), own_server_timeout: 120, own_server_output_limit: 4000, ..base.clone() };
        assert_eq!(runner_config(&own.runner_payload(), &base, 1200), own);

        let gemma = LlmConfig { runner: "comfyui_gemma".into(), comfy_gemma_model: "g.gguf".into(), comfy_gemma_mmproj: "p.gguf".into(), comfy_gemma_n_ctx: 32768, comfy_gemma_gpu_device: 1, ..base.clone() };
        let back = runner_config(&gemma.runner_payload(), &base, 1200);
        assert_eq!((back.runner.as_str(), back.comfy_gemma_model.as_str(), back.comfy_gemma_mmproj.as_str(), back.comfy_gemma_n_ctx, back.comfy_gemma_gpu_device), ("comfyui_gemma", "g.gguf", "p.gguf", 32768, 1));

        for hosted in ["openai", "anthropic", "google"] {
            let api = LlmConfig { runner: hosted.into(), api_model: "model-1".into(), api_custom_model: "custom".into(), api_key: "k".into(), ..base.clone() };
            let payload = api.runner_payload();
            assert_eq!(runner_kind(&payload), "llm_api");
            let back = runner_config(&payload, &base, 1200);
            assert_eq!((back.api_provider.as_str(), back.api_model.as_str(), back.api_custom_model.as_str(), back.api_key.as_str()), (hosted, "model-1", "custom", "k"));
        }
    }
}
