//! What can go wrong while driving Chrome. The messages are written for the person who reads them in the Thumbnail dialog.

use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq)]
pub enum BrowserError {
    /// Chrome is not there: not installed, not started, or the debug port is not open.
    #[error("{0}")]
    Chrome(String),
    /// The connection to Chrome broke.
    #[error("The connection to Chrome was lost.")]
    Closed,
    /// Chrome refused a DevTools call.
    #[error("Chrome refused {method}: {message}")]
    Protocol { method: String, message: String },
    /// Something did not happen in time. The text says what we waited for.
    #[error("{0}")]
    Timeout(String),
    /// A page did not look the way the automation expects (a button or box was not found, a sign-in is needed).
    #[error("{0}")]
    Page(String),
    /// A file could not be read or written.
    #[error("{0}")]
    File(String),
}

impl BrowserError {
    pub fn page(message: impl Into<String>) -> Self {
        BrowserError::Page(message.into())
    }

    pub fn timeout(message: impl Into<String>) -> Self {
        BrowserError::Timeout(message.into())
    }
}

pub type Result<T> = std::result::Result<T, BrowserError>;
