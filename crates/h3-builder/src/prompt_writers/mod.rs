//! The prompt writers behind the Builder's LLM buttons: they build an instruction from the request, ask the selected runner,
//! and clean and check the reply. Ports of `llm/image_prompt_generation.py`, `llm/video_prompt_generation.py` and
//! `llm/builder_runner.py` (the repair step).
//!
//! The runner is a trait so the writers can be tested against canned replies. In the app it is [`RunnerLlm`]: the runner the
//! request's own settings select (`text_runner`, `lmstudio_*`, `own_server_*`, ...).

pub mod concepts;
pub mod enhance;
#[cfg(test)]
mod golden;
pub mod image_prompts;
pub mod images;
pub mod references;
pub mod repair;

use std::future::Future;
use std::pin::Pin;

use h3_core::pyval::{text_or_empty, to_float, to_int, truthy, Obj};
use h3_llm::runner::{run_text, run_vision, RunInfo};
use h3_llm::LlmConfig;
use serde_json::{json, Value};

/// A text question for the runner.
#[derive(Debug, Clone, PartialEq)]
pub struct TextCall {
    pub instruction: String,
    pub temperature: f64,
    pub top_p: f64,
    pub max_new_tokens: i64,
    pub label: String,
    pub preserve_paragraphs: bool,
    pub json_schema: Option<Value>,
}

impl TextCall {
    pub fn new(instruction: String, temperature: f64, top_p: f64, max_new_tokens: i64, label: &str) -> Self {
        Self { instruction, temperature, top_p, max_new_tokens, label: label.to_string(), preserve_paragraphs: false, json_schema: None }
    }

    pub fn paragraphs(mut self) -> Self {
        self.preserve_paragraphs = true;
        self
    }
}

/// A question about pictures.
pub struct VisionCall {
    pub instruction: String,
    pub images: Vec<image::DynamicImage>,
    pub temperature: f64,
    pub top_p: f64,
    pub max_new_tokens: i64,
}

/// What a runner answers with: the text and a description of how it was made (runner, model).
pub type LlmFuture<'a> = Pin<Box<dyn Future<Output = Result<(String, Obj), String>> + Send + 'a>>;

pub trait Llm: Sync {
    /// A text prompt. The reply is cleaned like `_run_builder_text_llm` does.
    fn text(&self, call: TextCall) -> LlmFuture<'_>;
    /// A picture prompt. The reply is raw.
    fn vision(&self, call: VisionCall) -> LlmFuture<'_>;
}

fn describe(info: &RunInfo) -> Obj {
    let mut map = Obj::new();
    map.insert("runner".into(), json!(info.runner));
    if !info.used_provider.is_empty() {
        map.insert("used_provider".into(), json!(info.used_provider));
    }
    map.insert("used_model".into(), json!(info.used_model));
    map.insert("unloaded".into(), json!(info.unloaded));
    map
}

/// The runner a request selects with its own settings.
pub struct RunnerLlm<'a> {
    pub payload: &'a Obj,
    pub base: &'a LlmConfig,
}

impl Llm for RunnerLlm<'_> {
    fn text(&self, call: TextCall) -> LlmFuture<'_> {
        Box::pin(async move {
            let tokens = call.max_new_tokens.clamp(0, i64::from(u32::MAX)) as u32;
            run_text(self.payload, self.base, &call.instruction, call.temperature, call.top_p, tokens, call.preserve_paragraphs, call.json_schema).await.map(|(text, info)| (text, describe(&info))).map_err(|e| e.to_string())
        })
    }

    fn vision(&self, call: VisionCall) -> LlmFuture<'_> {
        Box::pin(async move {
            let tokens = call.max_new_tokens.clamp(0, i64::from(u32::MAX)) as u32;
            run_vision(self.payload, self.base, &call.instruction, &call.images, call.temperature, call.top_p, tokens).await.map(|(text, info)| (text, describe(&info))).map_err(|e| e.to_string())
        })
    }
}

// -- reading the request the way the pack's functions do ------------------------------------------------------------------

/// `str(payload.get(key) or "").strip()`.
pub fn text(payload: &Obj, key: &str) -> String {
    text_or_empty(payload.get(key)).trim().to_string()
}

/// The first of several keys with text: `str(a or b or "").strip()`.
pub fn first_text(payload: &Obj, keys: &[&str]) -> String {
    keys.iter().map(|k| text(payload, k)).find(|t| !t.is_empty()).unwrap_or_default()
}

/// `bool(payload.get(key))`.
pub fn flag(payload: &Obj, key: &str) -> bool {
    payload.get(key).is_some_and(truthy)
}

/// A dict value, or an empty one.
pub fn object(payload: &Obj, key: &str) -> Obj {
    payload.get(key).and_then(Value::as_object).cloned().unwrap_or_default()
}

/// `float(payload.get(key) or default)`.
pub fn number(payload: &Obj, key: &str, default: f64) -> Result<f64, String> {
    match payload.get(key).filter(|v| truthy(v)) {
        None => Ok(default),
        Some(value) => to_float(value).ok_or_else(|| match value {
            Value::String(s) => format!("could not convert string to float: '{s}'"),
            _ => "float() argument must be a string or a real number".to_string(),
        }),
    }
}

/// `int(payload.get(key) or default)`.
pub fn whole(payload: &Obj, key: &str, default: i64) -> Result<i64, String> {
    match payload.get(key).filter(|v| truthy(v)) {
        None => Ok(default),
        Some(value) => to_int(value).ok_or_else(|| format!("invalid literal for int() with base 10: {}", match value {
            Value::String(s) => format!("'{s}'"),
            other => other.to_string(),
        })),
    }
}

/// The reply fields every writer adds: the runner and model it used.
pub fn run_fields(info: &Obj) -> Obj {
    let mut out = Obj::new();
    for key in ["runner", "used_model", "unloaded"] {
        if let Some(value) = info.get(key) {
            out.insert(key.into(), value.clone());
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod testing {
    //! A runner that answers from a list and remembers what it was asked.

    use std::sync::Mutex;

    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    pub enum Asked {
        Text(TextCall),
        Vision { instruction: String, sizes: Vec<(u32, u32)>, temperature: f64, top_p: f64, max_new_tokens: i64 },
    }

    pub struct Canned {
        replies: Mutex<Vec<String>>,
        pub asked: Mutex<Vec<Asked>>,
    }

    impl Canned {
        pub fn new(replies: &[&str]) -> Self {
            Self { replies: Mutex::new(replies.iter().map(|r| r.to_string()).collect()), asked: Mutex::new(Vec::new()) }
        }

        fn next(&self) -> Result<String, String> {
            let mut replies = self.replies.lock().unwrap();
            if replies.is_empty() { Err("The recorder ran out of canned replies.".into()) } else { Ok(replies.remove(0)) }
        }
    }

    impl Llm for Canned {
        fn text(&self, call: TextCall) -> LlmFuture<'_> {
            Box::pin(async move {
                self.asked.lock().unwrap().push(Asked::Text(call));
                let info = json!({"runner": "lm_studio", "used_model": "test-model", "unloaded": false}).as_object().cloned().unwrap();
                self.next().map(|t| (t, info))
            })
        }

        fn vision(&self, call: VisionCall) -> LlmFuture<'_> {
            Box::pin(async move {
                let sizes = call.images.iter().map(|i| (i.width(), i.height())).collect();
                self.asked.lock().unwrap().push(Asked::Vision { instruction: call.instruction, sizes, temperature: call.temperature, top_p: call.top_p, max_new_tokens: call.max_new_tokens });
                let info = json!({"runner": "lm_studio_vision", "used_model": "test-model", "unloaded": false}).as_object().cloned().unwrap();
                self.next().map(|t| (t, info))
            })
        }
    }
}
