//! ComfyUI API graph building from the H3 templates. Port of the pack's `runner/minimax_*.py`.
//!
//! The builders take the render payload (settings plus the scene's prompt, audio, references and timing) and return the
//! API graph and a summary. They reach the outside world only through [`GraphEnv`].

pub mod api;
pub mod env;
pub mod inputs;
pub mod patches;
pub mod refmod;
pub mod timestamp;
pub mod workflows;

pub use env::{GraphEnv, GraphError, LatentFacts, Result};
pub use workflows::{build, build_advanced, build_single, build_two_pass};
