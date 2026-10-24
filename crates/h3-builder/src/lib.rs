//! What the Video Builder front end asks the server to do. The front end is the pack's own (copied unchanged), so each
//! function here does what the pack's handler behind the same route does, with the same request and reply shapes.
//!
//! The functions take the request body as a JSON object and return the reply fields. The HTTP layer in `h3-api` wraps
//! them in the pack's `{"ok": true, ...}` and `{"ok": false, "error": ...}` envelopes.

pub mod assets;
pub mod audio_mask;
pub mod env;
pub mod error_log;
pub mod files;
pub mod media;
pub mod payload;
pub mod projects;
pub mod runner;
pub mod system;

pub use env::Env;
