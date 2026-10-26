//! The image graphs the pack's own patch functions made (`tools/make_image_fixtures.py`), rebuilt in Rust from the same
//! request and compared whole.
#![cfg_attr(not(windows), allow(dead_code, unused_imports))] // the tests that use these helpers only run on Windows

use std::fs;
use std::path::PathBuf;

use h3_core::pyval::Obj;
use h3_core::timing::TimingPlan;
use h3_graph::image::build_image;
use h3_graph::{GraphEnv, LatentFacts};
use serde_json::Value;

const FAKE_ROOT: &str = "Z:\\h3fixture";

struct Env;

impl GraphEnv for Env {
    fn is_file(&self, path: &str) -> bool {
        path.starts_with(FAKE_ROOT) || std::path::Path::new(path).is_file()
    }
    fn is_dir(&self, _path: &str) -> bool {
        false
    }
    fn ensure_dir(&self, _path: &str) {}
    fn node_info(&self, _class: &str) -> Option<Value> {
        None
    }
    fn model_choices(&self, category: &str) -> Vec<String> {
        let names: &[&str] = match category {
            "loras" => &["style\\film_look.safetensors", "style\\soft_light.safetensors"],
            // The models the fixtures use are installed.
            "diffusion_models" => &["krea2_turbo_fp8_scaled.safetensors", "z_image_turbo_bf16.safetensors", "moss_image_sft_bf16.safetensors", "k.safetensors"],
            "text_encoders" => &["qwen3vl_4b_fp8_scaled.safetensors", "qwen_3_4b.safetensors", "qwen3.5_9b_int8_convrot.safetensors", "z.safetensors"],
            "vae" => &["qwen_image_vae.safetensors", "ae.safetensors", "flux2-vae.safetensors"],
            _ => &[],
        };
        names.iter().map(|s| s.to_string()).collect()
    }
    fn output_dir(&self) -> String {
        format!("{FAKE_ROOT}\\comfy\\output")
    }
    fn input_dir(&self) -> String {
        format!("{FAKE_ROOT}\\comfy\\input")
    }
    fn probe_duration(&self, _path: &str) -> Result<f64, String> {
        Ok(0.0)
    }
    fn trim_audio(&self, _source: &str, _project: &str, _scene: u32, _timing: &TimingPlan) -> Result<Value, String> {
        Err("no audio".into())
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
        42
    }
    fn ensure_placeholder_image(&self) -> Result<String, String> {
        Ok("vrgdg_placeholder_i2i.png".into())
    }
    fn prepare_load_image(&self, path: &str, data: &str, _name: &str) -> Result<String, String> {
        Ok(if path.is_empty() && data.is_empty() { String::new() } else { "vrgdg_i2i_fixed.png".into() })
    }
}

fn fixture(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/image").join(format!("{name}.json"));
    serde_json::from_str(&fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))).unwrap()
}

fn run(file: &str, model: &str) {
    let cases = fixture(file);
    let cases = cases["cases"].as_array().unwrap();
    assert!(!cases.is_empty());
    for (index, case) in cases.iter().enumerate() {
        let payload: Obj = case["payload"].as_object().unwrap().clone();
        let built = build_image(&Env, model, &payload).unwrap_or_else(|e| panic!("{file} case {index}: {e}"));
        assert_eq!(built["prompt"], case["graph"], "{file} case {index} differs from the pack's graph");
    }
}

#[test]
fn zimage_graphs_match_the_pack() {
    run("zimage", "zimage");
}

#[test]
fn ernie_graphs_match_the_pack() {
    run("ernie", "ernie_image");
}

// Its fixtures hold Windows paths, so it only runs on Windows.
#[cfg(windows)]
#[test]
fn flux_klein_graphs_match_the_pack() {
    run("flux_klein", "flux_klein");
}

#[test]
fn krea2_graphs_match_the_pack() {
    run("krea2", "krea2");
}

#[test]
fn krea2_two_pass_graphs_match_the_pack() {
    run("krea2_2pass", "krea2_2pass");
}

#[test]
fn moss_image_graphs_match_the_pack() {
    run("moss_image", "moss_image");
}

// Its fixtures hold Windows paths, so it only runs on Windows.
#[cfg(windows)]
#[test]
fn nano_banana_graphs_match_the_pack() {
    run("nb_image", "nb_image");
}

#[test]
fn z_upscale_enhance_graphs_match_the_pack() {
    run("z_upscale_enhance", "z_upscale_enhance");
}

#[test]
fn a_missing_model_is_named() {
    let payload: Obj = serde_json::json!({"prompt": "x", "krea_unet_name": "nope.safetensors"}).as_object().unwrap().clone();
    let error = build_image(&Env, "krea2", &payload).unwrap_err().to_string();
    assert_eq!(error, "Krea2 diffusion model 'nope.safetensors' was not found in ComfyUI/models/diffusion_models. Install the model there, refresh/restart ComfyUI, then try again.");
    assert_eq!(build_image(&Env, "krea2", &serde_json::json!({}).as_object().unwrap().clone()).unwrap_err().to_string(), "Prompt text is empty.");
}
