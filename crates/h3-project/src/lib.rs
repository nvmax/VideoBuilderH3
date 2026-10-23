//! Project session model, app config and crash-safe file writes.
//!
//! This crate reads and writes the same project folders as the Python Video Builder, byte for byte where it can, so a
//! project opens in both apps.

pub mod assets;
pub mod atomic;
pub mod backups;
pub mod config;
pub mod derived;
pub mod images;
pub mod journal;
pub mod latents;
pub mod layout;
pub use h3_core::pyjson;
pub mod scaffold;
pub mod scene_ops;
pub mod session;
pub mod takes;

pub use atomic::{atomic_write_json, atomic_write_text};
pub use config::AppConfig;
pub use layout::Stamp;
pub use session::{load_session, persist_session, save_session, LoadedSession, SaveInput, SaveOutcome};
