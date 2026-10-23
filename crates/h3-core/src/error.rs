//! Error type of the pure core. `message` matches the pack's Python exception text so fixtures can compare it.

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct CoreError {
    pub message: String,
    /// Machine readable name of the problem (the pack's `ShotPromptError.code`). `None` for a plain `ValueError`.
    pub code: Option<String>,
    /// Prompt length for `MINIMAX_H3_PROMPT_TOO_LONG`, else 0.
    pub length: i64,
}

impl CoreError {
    pub fn value(message: impl Into<String>) -> Self {
        Self { message: message.into(), code: None, length: 0 }
    }

    /// A shot prompt problem with its code.
    pub fn shot(message: impl Into<String>, code: &str) -> Self {
        Self { message: message.into(), code: Some(code.to_string()), length: 0 }
    }
}
