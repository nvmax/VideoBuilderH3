//! A stand-in ComfyUI for tests that only need to know which node classes exist.

use h3_core::timing::TimingPlan;
use h3_graph::chunked_pass2::CHUNK_CLASS;
use h3_graph::{GraphEnv, LatentFacts};
use serde_json::{json, Value};

/// A ComfyUI with the pack's chunk node installed or not, and nothing else the graph builders ask about.
pub struct StubEnv {
    pub has_chunk_node: bool,
}

impl GraphEnv for StubEnv {
    fn is_file(&self, _path: &str) -> bool {
        false
    }
    fn is_dir(&self, _path: &str) -> bool {
        false
    }
    fn ensure_dir(&self, _path: &str) {}
    fn node_info(&self, class: &str) -> Option<Value> {
        (class == CHUNK_CLASS && self.has_chunk_node).then(|| json!({}))
    }
    fn model_choices(&self, _category: &str) -> Vec<String> {
        Vec::new()
    }
    fn output_dir(&self) -> String {
        String::new()
    }
    fn probe_duration(&self, _path: &str) -> Result<f64, String> {
        Err("no media".into())
    }
    fn trim_audio(&self, _source: &str, _project: &str, _scene: u32, _timing: &TimingPlan) -> Result<Value, String> {
        Err("no media".into())
    }
    fn latent_exists(&self, _project: &str, _scene: u32) -> bool {
        false
    }
    fn latent_path(&self, _project: &str, _scene: u32) -> String {
        String::new()
    }
    fn latent_facts(&self, _project: &str, _scene: u32) -> LatentFacts {
        LatentFacts::default()
    }
    fn pass1_latent_matches(&self, _project: &str, _scene: u32) -> bool {
        false
    }
    fn random_seed(&self) -> u64 {
        1
    }
}
