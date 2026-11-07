//! The image workflows the Thumbnail dialog sends a frame through (mirrors the pack's `tests/test_thumbnail_engines.py`): model names that
//! become what ComfyUI lists, Flux/Klein, and MOSS-Image image to image.
#![cfg_attr(not(windows), allow(dead_code, unused_imports))] // the fake files use Windows paths

use h3_core::pyval::Obj;
use h3_core::timing::TimingPlan;
use h3_graph::image::build_image;
use h3_graph::{GraphEnv, LatentFacts};
use serde_json::{json, Value};

const FAKE_ROOT: &str = "Z:\\h3fixture";

/// A ComfyUI that keeps its models in the models folders themselves (no `flux` sub folder).
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
            "diffusion_models" => &["flux-2-klein-4b-fp8.safetensors", "moss_image_sft_bf16.safetensors", "Other\\Model.safetensors"],
            "text_encoders" => &["qwen_3_4b.safetensors", "qwen3.5_9b_int8_convrot.safetensors"],
            "vae" => &["flux2-vae.safetensors"],
            "loras" => &["moss_image_sft_8_steps_dmd_lora.safetensors"],
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

fn obj(value: Value) -> Obj {
    value.as_object().cloned().unwrap()
}

fn flux(changes: Value) -> Result<Value, String> {
    let mut payload = obj(json!({
        "prompt": "Recrop the attached image.", "image_ingredients": [{"path": format!("{FAKE_ROOT}\\refs\\frame.png")}],
        "unet_name": "flux\\flux-2-klein-4b-fp8.safetensors", "clip_name": "qwen_3_4b.safetensors", "vae_name": "flux\\flux2-vae.safetensors",
        "width": 1280, "height": 720, "seed": 5,
    }));
    payload.extend(obj(changes));
    build_image(&Env, "flux_klein", &payload).map(|built| built["prompt"].clone()).map_err(|e| e.to_string())
}

fn moss(changes: Value) -> Result<Value, String> {
    let mut payload = obj(json!({"prompt": "A neon portrait.", "width": 1280, "height": 720, "seed": 3, "steps": 8, "cfg": 1}));
    payload.extend(obj(changes));
    build_image(&Env, "moss_image", &payload).map(|built| built["prompt"].clone()).map_err(|e| e.to_string())
}

#[cfg(windows)]
#[test]
fn flux_klein_default_names_become_what_comfyui_lists() {
    let graph = flux(json!({})).unwrap();
    assert_eq!(graph["1068"]["inputs"]["unet_name"], "flux-2-klein-4b-fp8.safetensors");
    assert_eq!(graph["1066"]["inputs"]["clip_name"], "qwen_3_4b.safetensors");
    assert_eq!(graph["1064"]["inputs"]["vae_name"], "flux2-vae.safetensors");
}

#[cfg(windows)]
#[test]
fn a_flux_klein_model_that_is_not_installed_says_which_one() {
    let message = flux(json!({"vae_name": "flux\\missing-vae.safetensors"})).unwrap_err();
    assert!(message.contains("Flux/Klein VAE 'flux\\missing-vae.safetensors' was not found in ComfyUI/models/vae"), "{message}");
}

#[test]
fn moss_text_to_image_is_unchanged() {
    let graph = moss(json!({})).unwrap();
    assert_eq!(graph["9"]["class_type"], "EmptyFlux2LatentImage");
    assert_eq!(graph["10"]["inputs"]["latent_image"], json!(["9", 0]));
    assert_eq!(graph["10"]["inputs"]["denoise"], 1.0);
    for node in ["13", "14", "15"] {
        assert!(graph.get(node).is_none(), "node {node} belongs to image to image");
    }
}

#[test]
fn moss_image_to_image_encodes_the_picture_and_samples_from_it_at_the_chosen_denoise() {
    let graph = moss(json!({"use_image_to_image": true, "image_to_image_path": "C:/frame.png", "denoise": 0.4})).unwrap();
    assert_eq!(graph["13"], json!({"class_type": "LoadImage", "inputs": {"image": "vrgdg_i2i_fixed.png"}}));
    assert_eq!(graph["14"]["class_type"], "ImageScale");
    assert_eq!((graph["14"]["inputs"]["width"].clone(), graph["14"]["inputs"]["height"].clone(), graph["14"]["inputs"]["crop"].clone()), (json!(1280), json!(720), json!("center")));
    assert_eq!(graph["15"], json!({"class_type": "VAEEncode", "inputs": {"pixels": ["14", 0], "vae": ["3", 0]}}));
    assert_eq!(graph["10"]["inputs"]["latent_image"], json!(["15", 0]));
    assert_eq!(graph["10"]["inputs"]["denoise"], 0.4);
    assert!(graph.get("9").is_none(), "there is no empty latent when the picture is the start");
}

#[test]
fn moss_denoise_defaults_to_065_and_is_kept_in_range() {
    let denoise = |extra: Value| {
        let mut changes = obj(json!({"use_image_to_image": true, "image_to_image_path": "a"}));
        changes.extend(obj(extra));
        moss(Value::Object(changes)).unwrap()["10"]["inputs"]["denoise"].clone()
    };
    assert_eq!(denoise(json!({})), 0.65);
    assert_eq!(denoise(json!({"denoise": 0})), 0.05);
    assert_eq!(denoise(json!({"denoise": 7})), 1.0);
}

#[test]
fn moss_image_to_image_without_a_picture_is_refused() {
    let message = moss(json!({"use_image_to_image": true})).unwrap_err();
    assert!(message.contains("no source image was provided"), "{message}");
}

#[test]
fn moss_names_become_what_comfyui_lists() {
    let graph = moss(json!({"vae_name": "flux\\flux2-vae.safetensors", "lora_name": "moss_image_sft_8_steps_dmd_lora.safetensors"})).unwrap();
    assert_eq!(graph["3"]["inputs"]["vae_name"], "flux2-vae.safetensors");
    assert_eq!(graph["4"]["inputs"]["lora_name"], "moss_image_sft_8_steps_dmd_lora.safetensors");
    let case = moss(json!({"unet_name": "other/model.SAFETENSORS"})).unwrap();
    assert_eq!(case["1"]["inputs"]["unet_name"], "Other\\Model.safetensors", "slash direction and case do not matter, the listed spelling comes back");
}
