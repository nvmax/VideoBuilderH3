//! Image presets: ComfyUI API workflows with bindings, import, and generation.
//!
//! The pack hard codes one patch function per image model. Here a preset carries its own bindings, so any model that has
//! a ComfyUI workflow works: import the workflow saved in API format, check the suggested bindings, generate.

pub mod builtin;
pub mod detect;
pub mod preset;
pub mod run;
pub mod store;

pub use builtin::builtin_presets;
pub use detect::{choices_for, parse_api_workflow, suggest_bindings};
pub use preset::{apply_preset, ensure_placeholder, Applied, ApplyCtx, Binding, BindingKind, ImagePreset, Op, PresetOrigin, Target};
pub use run::{generate, missing_nodes, Generated, ImageEvent};
pub use store::PresetStore;

#[derive(Debug, thiserror::Error)]
pub enum ImageError {
    /// The preset or its workflow is wrong.
    #[error("{0}")]
    Preset(String),
    /// A value the caller sent is wrong.
    #[error("{0}")]
    Input(String),
    #[error("This workflow needs ComfyUI nodes that are not installed: {}.", .0.join(", "))]
    MissingNodes(Vec<String>),
    #[error("Preset '{0}' was not found.")]
    NotFound(String),
    #[error("ComfyUI finished prompt {0} but it produced no image.")]
    NoImage(String),
    #[error(transparent)]
    Comfy(#[from] h3_comfy::ComfyError),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}
