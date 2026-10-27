//! What the Video Builder front end asks the server to do. The front end is the pack's own (copied unchanged), so each
//! function here does what the pack's handler behind the same route does, with the same request and reply shapes.
//!
//! The functions take the request body as a JSON object and return the reply fields. The HTTP layer in `h3-api` wraps
//! them in the pack's `{"ok": true, ...}` and `{"ok": false, "error": ...}` envelopes.

pub mod assets;
pub mod audio_mask;
pub mod capcut;
pub mod drafts;
pub mod elevenlabs;
pub mod env;
pub mod error_log;
pub mod export_video;
pub mod files;
pub mod overlay;
pub mod overlay_library;
#[cfg(test)]
mod golden_support;
pub mod instructions;
pub mod linux_dialog;
pub mod media;
pub mod payload;
pub mod post_process;
pub mod projects;
pub mod refmod_studio;
pub mod prompt_writers;
pub mod render_log;
pub mod runner;
pub mod story;
pub mod system;
pub mod transcribe;

pub use env::Env;
