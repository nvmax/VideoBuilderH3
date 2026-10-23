//! Scene render, render all, build full video and stitch.
//!
//! `render` prepares a scene exactly as the pack's orchestrator does, builds the graph, runs it in ComfyUI and finds the
//! clip it made.

pub mod env;
pub mod render;

pub use env::LiveEnv;
pub use render::{prepare_scene_render, render_scene, Prepared, RenderError, RenderEvent, RenderOutcome, RenderParams};
