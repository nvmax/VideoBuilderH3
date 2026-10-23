//! The real ComfyUI, project folder and ffmpeg behind the graph builders.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use h3_comfy::{models::model_choices, ComfyDirs};
use h3_core::timing::TimingPlan;
use h3_graph::{GraphEnv, LatentFacts};
use h3_media::{audio, Tools};
use h3_project::latents;
use serde_json::Value;

/// Answers the graph builders' questions from a live ComfyUI (its `/object_info` and folders), the project on disk and
/// ffmpeg. Everything it needs from ComfyUI is read once up front, so the builders run without network calls.
pub struct LiveEnv {
    pub object_info: Value,
    pub dirs: ComfyDirs,
    pub tools: Tools,
}

impl LiveEnv {
    pub fn new(object_info: Value, dirs: ComfyDirs, tools: Tools) -> Self {
        Self { object_info, dirs, tools }
    }
}

impl GraphEnv for LiveEnv {
    fn is_file(&self, path: &str) -> bool {
        Path::new(path).is_file()
    }

    fn is_dir(&self, path: &str) -> bool {
        Path::new(path).is_dir()
    }

    fn ensure_dir(&self, path: &str) {
        let _ = std::fs::create_dir_all(path);
    }

    fn node_info(&self, class: &str) -> Option<Value> {
        self.object_info.get(class).cloned()
    }

    fn model_choices(&self, category: &str) -> Vec<String> {
        model_choices(&self.object_info, category)
    }

    fn output_dir(&self) -> String {
        self.dirs.output.clone()
    }

    fn probe_duration(&self, path: &str) -> Result<f64, String> {
        audio::probe_duration(&self.tools, path).map_err(|e| e.to_string())
    }

    fn trim_audio(&self, source: &str, project: &str, scene: u32, timing: &TimingPlan) -> Result<Value, String> {
        audio::trim_scene_audio(&self.tools, source, project, scene, timing).map_err(|e| e.to_string())
    }

    fn latent_exists(&self, project: &str, scene: u32) -> bool {
        let project = Path::new(project);
        project.is_dir() && project.join("latents").join(format!("scene_{scene:03}.latent")).is_file()
    }

    fn latent_path(&self, project: &str, scene: u32) -> String {
        Path::new(project).join("latents").join(format!("scene_{scene:03}.latent")).display().to_string()
    }

    fn latent_facts(&self, project: &str, scene: u32) -> LatentFacts {
        match latents::latent_info(Path::new(project), scene) {
            Ok(info) if info.exists => LatentFacts { exists: true, token_count: info.token_count, tail_padding_frames: info.tail_padding_frames },
            _ => LatentFacts::default(),
        }
    }

    fn pass1_latent_matches(&self, project: &str, scene: u32) -> bool {
        latents::pass1_latent_matches(Path::new(project), scene)
    }

    fn random_seed(&self) -> u64 {
        // A seed only has to differ between renders. The clock's nanoseconds mixed through a multiply do that.
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos()) as u64;
        nanos.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (nanos >> 17)
    }
}
