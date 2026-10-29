//! Transitions between two pictures, and effects and filters for one: the [gl-transitions](https://gl-transitions.com) collection and the app's own
//! (`custom` for transitions, `looks` for effects and filters), drawn with wgpu.
//!
//! `catalog` lists the transitions with their parameters, and `TransitionRenderer::render` draws one frame of one of them from two
//! RGBA pictures and a progress. `look_catalog` and `TransitionRenderer::render_look` do the same for an effect or a filter and one picture. The renderer knows
//! nothing about video files: the caller decodes the frames and encodes the result. The same shaders run in the app's WebGL preview from the GLSL ES 3.00
//! sources (`catalog::glsl3`, `catalog::look_glsl3`).

pub mod catalog;
pub mod custom;
pub mod entry;
pub mod library;
pub mod looks;
pub mod render;

pub use catalog::{catalog, glsl3, look, look_catalog, look_glsl3, params_from_json, transition};
pub use entry::{Entry, Kind, Origin, ParamDef, ParamType, Params, Value};
pub use render::TransitionRenderer;

#[derive(Debug, thiserror::Error)]
pub enum TransitionError {
    #[error("Unknown transition `{0}`.")]
    Unknown(String),
    #[error("Unknown {kind} `{name}`.")]
    UnknownLook { kind: &'static str, name: String },
    #[error("{0}")]
    Params(String),
    #[error("No graphics adapter could be opened: {0}")]
    NoAdapter(String),
    #[error("The graphics device failed: {0}")]
    Gpu(String),
    #[error("The pictures must be {expected} bytes (width x height x 4), got {got}.")]
    PictureSize { expected: usize, got: usize },
}
