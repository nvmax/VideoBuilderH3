//! LLM providers. LM Studio is used through the model that is already loaded: this crate never loads, unloads or
//! switches a model.

pub mod checks;
pub mod clean;
pub mod client;
pub mod config;
pub mod describe;
pub mod locations;

pub use client::{chat, describe_active, loaded_lm_studio_models, ActiveLlm, ChatReply, ChatRequest, LlmError, LoadedModel};
pub use config::LlmConfig;
