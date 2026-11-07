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
            "loras" => &["moss_image_sft_8_steps_dmd_lora.safetensors", "moss\\moss_image_edit_lora.safetensors"],
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

// -- MOSS-Image Edit: the edit LoRA and ComfyUI's Text Encode MOSS-Image Edit (mirrors the pack's `MossImageEditTests`) ----------------

fn moss_edit(changes: Value) -> Result<Value, String> {
    let mut payload = obj(json!({
        "use_edit": true, "prompt": "Add the title text \"HEAT\". Keep the man unchanged.", "seed": 3,
        "image_ingredients": [{"path": format!("{FAKE_ROOT}\\refs\\frame0.png")}],
    }));
    payload.extend(obj(changes));
    build_image(&Env, "moss_image", &payload).map(|built| built["prompt"].clone()).map_err(|e| e.to_string())
}

fn pictures(count: usize) -> Value {
    Value::Array((0..count).map(|i| json!({"path": format!("{FAKE_ROOT}\\refs\\frame{i}.png")})).collect())
}

#[cfg(windows)]
#[test]
fn the_edit_graph_is_the_one_comfyui_ships_for_an_edit() {
    let graph = moss_edit(json!({})).unwrap();
    let classes: std::collections::BTreeMap<String, String> =
        graph.as_object().unwrap().iter().map(|(id, node)| (id.clone(), node["class_type"].as_str().unwrap().to_string())).collect();
    let expected = [
        ("1", "UNETLoader"), ("2", "CLIPLoader"), ("3", "VAELoader"), ("4", "LoraLoaderModelOnly"), ("5", "SkipLayerGuidanceDiTSimple"),
        ("20", "LoadImage"), ("30", "ImageScaleToTotalPixels"), ("40", "GetImageSize"), ("9", "EmptyFlux2LatentImage"),
        ("7", "TextEncodeMOSSImageEdit"), ("8", "TextEncodeMOSSImageEdit"), ("10", "KSampler"), ("11", "VAEDecode"), ("12", "PreviewImage"),
    ];
    assert_eq!(classes, expected.iter().map(|(id, class)| (id.to_string(), class.to_string())).collect());
    assert_eq!(graph["4"]["inputs"], json!({"model": ["1", 0], "lora_name": "moss\\moss_image_edit_lora.safetensors", "strength_model": 1.0}), "the LoRA's listed spelling");
    assert_eq!(graph["5"]["inputs"], json!({"model": ["4", 0], "double_layers": "", "single_layers": "22", "start_percent": 0.0, "end_percent": 1.0}));
    let sampler = &graph["10"]["inputs"];
    assert_eq!(sampler["model"], json!(["5", 0]));
    assert_eq!((sampler["steps"].clone(), sampler["cfg"].clone(), sampler["sampler_name"].clone(), sampler["scheduler"].clone(), sampler["denoise"].clone()),
               (json!(30), json!(4.0), json!("euler"), json!("normal"), json!(1.0)));
    assert_eq!(graph["9"]["inputs"], json!({"width": ["40", 0], "height": ["40", 1], "batch_size": 1}), "the result takes the picture's size");
    assert_eq!(graph["30"]["inputs"]["megapixels"], 1.0);
    for id in ["7", "8"] {
        let inputs = &graph[id]["inputs"];
        assert_eq!(inputs["edit_format"], "Moss-Image-Edit");
        assert_eq!(inputs["images.image_0"], json!(["30", 0]));
        assert_eq!(inputs["vae"], json!(["3", 0]), "without the VAE the picture only reaches the text encoder");
    }
    assert_eq!(graph["8"]["inputs"]["prompt"], "nsfw, worst quality, bad anatomy, watermark, jpeg artifact");
}

#[cfg(windows)]
#[test]
fn the_edit_takes_up_to_four_pictures() {
    let graph = moss_edit(json!({"image_ingredients": pictures(5)})).unwrap();
    for index in 0..4 {
        assert_eq!(graph[(30 + index).to_string()]["inputs"]["image"], json!([(20 + index).to_string(), 0]));
        assert_eq!(graph["7"]["inputs"][format!("images.image_{index}")], json!([(30 + index).to_string(), 0]));
    }
    assert!(graph.get("24").is_none());
    assert!(graph["7"]["inputs"].get("images.image_4").is_none());
    assert_eq!(graph["40"]["inputs"]["image"], json!(["30", 0]), "the size is the first picture's");
}

#[cfg(windows)]
#[test]
fn the_turbo_settings_of_text_to_image_do_not_reach_an_edit_and_it_wins_over_image_to_image() {
    let graph = moss_edit(json!({"lora_name": "moss_image_sft_8_steps_dmd_lora.safetensors", "denoise": 0.3, "use_image_to_image": true, "width": 1920})).unwrap();
    let loras: Vec<&Value> = graph.as_object().unwrap().values().filter(|node| node["class_type"] == "LoraLoaderModelOnly").map(|node| &node["inputs"]["lora_name"]).collect();
    assert_eq!(loras, [&json!("moss\\moss_image_edit_lora.safetensors")]);
    assert_eq!(graph["10"]["inputs"]["denoise"], 1.0);
    assert_eq!(graph["10"]["inputs"]["steps"], 30);
    assert!(graph.as_object().unwrap().values().all(|node| node["class_type"] != "ImageScale" && node["class_type"] != "VAEEncode"));
    assert_eq!(moss_edit(json!({"steps": 12, "cfg": 3})).unwrap()["10"]["inputs"]["steps"], 12);
}

#[cfg(windows)]
#[test]
fn skip_layer_guidance_and_the_format_can_be_changed() {
    let graph = moss_edit(json!({"skip_layer_guidance": false, "edit_format": "ai-toolkit"})).unwrap();
    assert!(graph.get("5").is_none());
    assert_eq!(graph["10"]["inputs"]["model"], json!(["4", 0]));
    assert_eq!(graph["7"]["inputs"]["edit_format"], "ai-toolkit");
    assert_eq!(moss_edit(json!({"edit_format": "nonsense"})).unwrap()["7"]["inputs"]["edit_format"], "Moss-Image-Edit");
    assert_eq!(moss_edit(json!({"negative_prompt": "blurry"})).unwrap()["8"]["inputs"]["prompt"], "blurry");
}

#[test]
fn an_edit_without_a_picture_or_a_prompt_is_refused() {
    let none = moss_edit(json!({"image_ingredients": []})).unwrap_err();
    assert!(none.contains("at least one reference image"), "{none}");
    let empty = moss_edit(json!({"prompt": "  "})).unwrap_err();
    assert!(empty.contains("Prompt text is empty"), "{empty}");
}

#[cfg(windows)]
#[test]
fn a_missing_edit_lora_says_which_file_it_needs() {
    let message = moss_edit(json!({"edit_lora_name": "not_there.safetensors"})).unwrap_err();
    assert!(message.contains("MOSS-Image Edit LoRA"), "{message}");
}
