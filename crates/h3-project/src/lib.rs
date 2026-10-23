//! Project session model, app config and crash-safe file writes.

pub mod atomic;
pub mod config;

pub use atomic::{atomic_write_json, atomic_write_text};
pub use config::AppConfig;
