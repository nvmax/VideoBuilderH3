//! LLM providers. LM Studio is used through the model that is already loaded: this crate never loads, unloads or
//! switches a model there. The built-in Gemma runs inside ComfyUI through the VRGDG Gemma node, which loads its own GGUF.

pub mod beat_prompts;
pub mod checks;
pub mod clean;
pub mod comfy_gemma;
pub mod client;
pub mod config;
pub mod describe;
pub mod instructions;
pub mod location_cards;
pub mod locations;
pub mod own_server;
pub mod prompt_text;
pub mod prompts;
pub mod providers;
pub mod runner;
pub mod story_prompts;

pub use client::{chat, describe_active, list_lm_studio_models, loaded_lm_studio_models, ActiveLlm, ChatReply, ChatRequest, LlmError, LoadedModel};
pub use config::LlmConfig;
