//! What the graph builders need from the outside world. The builders do no IO themselves, so the same code runs against
//! a real ComfyUI and project folder in the app and against fixed stand-ins in the tests.

use h3_core::timing::TimingPlan;
use serde_json::Value;

/// A problem the builders report. The text matches the pack's Python exception where one exists.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct GraphError {
    pub message: String,
}

impl GraphError {
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into() }
    }
}

impl From<h3_core::CoreError> for GraphError {
    fn from(e: h3_core::CoreError) -> Self {
        Self { message: e.message }
    }
}

pub type Result<T> = std::result::Result<T, GraphError>;

/// What is known about a scene's saved latent without loading it.
#[derive(Debug, Clone, Default)]
pub struct LatentFacts {
    pub exists: bool,
    pub token_count: i64,
    /// Frames past the scene's visible end inside the latent. `None` for latents saved before this was recorded.
    pub tail_padding_frames: Option<i64>,
}

pub trait GraphEnv {
    fn is_file(&self, path: &str) -> bool;
    fn is_dir(&self, path: &str) -> bool;
    fn ensure_dir(&self, path: &str);
    /// The `/object_info` entry of a node class, or `None` when ComfyUI does not have it.
    fn node_info(&self, class: &str) -> Option<Value>;
    /// File names ComfyUI lists for a model folder (`loras`, `vae`, `diffusion_models`, `unet`, `text_encoders`, `clip`,
    /// `latent_upscale_models`).
    fn model_choices(&self, category: &str) -> Vec<String>;
    /// ComfyUI's output folder.
    fn output_dir(&self) -> String;
    /// Length of a media file in seconds.
    fn probe_duration(&self, path: &str) -> std::result::Result<f64, String>;
    /// Cut the audio the plan asks for into the project's scene audio folder. Returns
    /// `{audio_path, start, duration, requested_duration, format}`.
    fn trim_audio(&self, source: &str, project: &str, scene: u32, timing: &TimingPlan) -> std::result::Result<Value, String>;
    fn latent_exists(&self, project: &str, scene: u32) -> bool;
    fn latent_path(&self, project: &str, scene: u32) -> String;
    fn latent_facts(&self, project: &str, scene: u32) -> LatentFacts;
    /// Whether the scene's first pass latent was saved by the same render as its final latent.
    fn pass1_latent_matches(&self, project: &str, scene: u32) -> bool;
    /// A seed for a negative (random) seed setting.
    fn random_seed(&self) -> u64;
    /// A saved RefMod by name, for the RefMod pipeline. `None` when there is no such RefMod.
    fn refmod_entry(&self, _name: &str) -> Option<crate::refmod::RefmodEntry> {
        None
    }
    /// Memory efficient Sage Attention needs the SageAttention package. Return the reason when it is missing.
    fn check_memory_efficient_sage_attention(&self) -> std::result::Result<(), String> {
        Ok(())
    }
}
