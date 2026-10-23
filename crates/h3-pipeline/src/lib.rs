//! Scene render, render all, build full video and stitch.
//!
//! `render` prepares a scene exactly as the pack's orchestrator does, builds the graph, runs it in ComfyUI and finds the
//! clip it made.

pub mod context;
pub mod env;
pub mod finish;
pub mod minimax_prompts;
pub mod refmod_library;
pub mod render;
pub mod scene_video;
pub mod stitch_plan;
pub mod video_ops;

pub use context::connect;
pub use env::LiveEnv;
pub use finish::{randomize_seeds, SEED_FIELDS, batch_targets, finish_scene, render_all, render_scene_full, stitch_project, BatchOptions, BatchResult, FinishOptions, RunContext, SceneHook, SceneResult};
pub use render::{prepare_scene_render, render_scene, Prepared, RenderError, RenderEvent, RenderOutcome, RenderParams};
