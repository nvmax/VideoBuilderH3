//! Error type of the pure core. `message` matches the pack's Python exception text so fixtures can compare it.

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct CoreError {
    pub message: String,
}

impl CoreError {
    pub fn value(message: impl Into<String>) -> Self {
        Self { message: message.into() }
    }
}
