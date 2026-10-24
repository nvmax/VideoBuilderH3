//! ComfyUI graphs for the Video Builder's Image tab: Z-Image, Krea 2 (single and two pass), MOSS-Image, Ernie, Flux Klein,
//! NanoBanana and the Z upscale and enhance pass. Port of `runner/image_workflows.py`. Each builder fills one of the pack's
//! API templates from the Builder's request, or builds the graph directly (MOSS-Image).

use std::path::{Path, PathBuf};

use h3_core::pyval::{bool_payload, float_payload, int_payload, round_half_even, text_or_empty, to_int, truthy, Obj};
use serde_json::{json, Value};

use crate::api::{link, node_id_by_class, set_input};
use crate::env::{GraphEnv, GraphError, Result};
use crate::inputs::{json_list_text, template};

const MAX_LORA_SLOTS: i64 = 20;
const NONE_LORA: &str = "[none]";

fn fail<T>(message: impl Into<String>) -> Result<T> {
    Err(GraphError::new(message))
}

fn text(payload: &Obj, key: &str) -> String {
    text_or_empty(payload.get(key))
}

/// `str(payload.get(key, "") or "").strip()`.
fn trimmed(payload: &Obj, key: &str) -> String {
    text(payload, key).trim().to_string()
}

fn first_text(payload: &Obj, keys: &[&str], default: &str) -> String {
    keys.iter().map(|k| trimmed(payload, k)).find(|v| !v.is_empty()).unwrap_or_else(|| default.to_string())
}

/// `_int_payload(payload, "seed", default, 0, 0xFFFFFFFFFFFFFFFF)`.
fn seed_of(payload: &Obj, default: u64) -> u64 {
    match payload.get("seed") {
        None => default,
        Some(Value::Number(n)) => n.as_u64().or_else(|| n.as_i64().map(|i| i.max(0) as u64)).or_else(|| n.as_f64().filter(|f| f.is_finite()).map(|f| f.max(0.0) as u64)).unwrap_or(default),
        Some(other) => to_int(other).map_or(default, |i| i.max(0) as u64),
    }
}

/// The seed to use: the saved one, or a new one when the seed mode is random.
fn pick_seed(env: &dyn GraphEnv, payload: &Obj, default: u64) -> u64 {
    let seed = seed_of(payload, default);
    if matches!(trimmed(payload, "seed_mode").to_lowercase().as_str(), "random" | "randomize") {
        env.random_seed()
    } else {
        seed
    }
}

fn set(prompt: &mut Obj, id: &str, name: &str, value: impl Into<Value>) -> Result<()> {
    set_input(prompt, id, name, value.into())
}

/// `_clean_lora_name`: a LoRA ComfyUI does not list becomes `[none]`.
fn clean_lora(env: &dyn GraphEnv, value: Option<&Value>) -> String {
    let name = match value {
        Some(v) if truthy(v) => text_or_empty(Some(v)).trim().to_string(),
        _ => NONE_LORA.to_string(),
    };
    if name == NONE_LORA || env.model_choices("loras").contains(&name) {
        name
    } else {
        NONE_LORA.to_string()
    }
}

fn load_template(name: &str) -> Result<(String, Obj)> {
    template(name)
}

/// `_require_model_choice`: the model must be one ComfyUI lists in one of the folders.
fn require_model(env: &dyn GraphEnv, categories: &[&str], value: &str, label: &str) -> Result<()> {
    let wanted = value.trim();
    if wanted.is_empty() {
        return fail_missing(categories, wanted, label);
    }
    let base = |s: &str| s.replace('\\', "/").rsplit('/').next().unwrap_or("").to_string();
    let wanted_base = base(wanted);
    for category in categories {
        for choice in env.model_choices(category) {
            let choice = choice.trim();
            if choice == wanted || (!choice.is_empty() && base(choice) == wanted_base) {
                return Ok(());
            }
        }
    }
    fail_missing(categories, wanted, label)
}

fn fail_missing<T>(categories: &[&str], value: &str, label: &str) -> Result<T> {
    fail(format!("{label} '{value}' was not found in ComfyUI/models/{}. Install the model there, refresh/restart ComfyUI, then try again.", categories[0]))
}

/// The picture the sampler's unused image input points at, or the one the user chose for image to image.
fn set_image_to_image(env: &dyn GraphEnv, prompt: &mut Obj, node: &str, payload: &Obj, enabled: bool, missing: &str) -> Result<()> {
    if enabled {
        let name = env
            .prepare_load_image(&text(payload, "image_to_image_path"), &text(payload, "image_to_image_data"), &match text(payload, "image_to_image_name") {
                n if n.is_empty() => "image.png".to_string(),
                n => n,
            })
            .map_err(GraphError::new)?;
        if name.is_empty() {
            return fail(missing);
        }
        set(prompt, node, "image", name)?;
    }
    Ok(())
}

fn set_basic_loras(env: &dyn GraphEnv, prompt: &mut Obj, node: &str, payload: &Obj, with_two_pass_flag: Option<bool>) -> Result<()> {
    set(prompt, node, "use_custom_loras", bool_payload(payload, "use_custom_loras", false))?;
    set(prompt, node, "lora_count", int_payload(payload, "lora_count", 0, 0, MAX_LORA_SLOTS))?;
    if let Some(flag) = with_two_pass_flag {
        set(prompt, node, "ltx_two_pass_mode", flag)?;
    }
    for slot in 1..=MAX_LORA_SLOTS {
        set(prompt, node, &format!("lora_{slot}"), clean_lora(env, payload.get(&format!("lora_{slot}"))))?;
        set(prompt, node, &format!("strength_{slot}"), float_payload(payload, &format!("strength_{slot}"), 1.0, -100.0, 100.0))?;
    }
    Ok(())
}

fn built(workflow: &str, prompt: Obj, seed: Option<u64>) -> Obj {
    let mut out = Obj::new();
    out.insert("workflow_path".into(), json!(workflow));
    out.insert("prompt".into(), Value::Object(prompt));
    if let Some(seed) = seed {
        out.insert("used_seed".into(), json!(seed));
    }
    out
}

fn prompt_text(payload: &Obj, empty_message: &str) -> Result<String> {
    let text = trimmed(payload, "prompt");
    if text.is_empty() {
        return fail(empty_message);
    }
    Ok(text)
}

// -- Z-Image ------------------------------------------------------------------------------------------------------------

pub fn build_zimage(env: &dyn GraphEnv, payload: &Obj) -> Result<Obj> {
    let (name, mut prompt) = load_template("text2image_zimage_API.json")?;
    let text_value = prompt_text(payload, "Prompt text is empty.")?;
    let (w1, h1) = (int_payload(payload, "first_pass_width", 1280, 64, 4096), int_payload(payload, "first_pass_height", 720, 64, 4096));
    let (w2, h2) = (int_payload(payload, "second_pass_width", 1920, 64, 4096), int_payload(payload, "second_pass_height", 1080, 64, 4096));
    let batch = int_payload(payload, "batch_size", 1, 1, 16);
    let seed = pick_seed(env, payload, 1);
    let use_i2i = bool_payload(payload, "use_image_to_image", false);
    let start_at = int_payload(payload, "image_to_image_start_at_step", 5, 1, 8);

    set(&mut prompt, "971", "text", text_value)?;
    set(&mut prompt, "960", "clip_name", text(payload, "clip_name"))?;
    set(&mut prompt, "961", "vae_name", text(payload, "vae_name"))?;
    set(&mut prompt, "972", "unet_name", text(payload, "unet_name"))?;
    set(&mut prompt, "965", "width", w1)?;
    set(&mut prompt, "965", "height", h1)?;
    set(&mut prompt, "965", "batch_size", batch)?;
    set(&mut prompt, "967", "width", w2)?;
    set(&mut prompt, "967", "height", h2)?;
    set(&mut prompt, "964", "noise_seed", seed)?;
    set(&mut prompt, "966", "noise_seed", seed)?;
    set(&mut prompt, "978", "switch", use_i2i)?;
    set(&mut prompt, "981", "switch", use_i2i)?;
    set(&mut prompt, "983", "value", start_at)?;
    set(&mut prompt, "979", "image", env.ensure_placeholder_image().map_err(GraphError::new)?)?;
    set_image_to_image(env, &mut prompt, "979", payload, use_i2i, "Image-to-image is enabled, but no source image was provided.")?;

    let use_loras = bool_payload(payload, "use_custom_loras", false);
    let lora_count = int_payload(payload, "lora_count", 0, 0, MAX_LORA_SLOTS);
    let lora_node = node_id_by_class(&prompt, "VRGDG_OptionalMultiLoraTwoPassStrengths", Some("974"))?;
    let two_pass_lora = crate::api::class_of(&prompt, &lora_node) == "VRGDG_OptionalMultiLoraTwoPassStrengths";
    set(&mut prompt, &lora_node, "use_custom_loras", use_loras)?;
    set(&mut prompt, &lora_node, "lora_count", lora_count)?;
    if two_pass_lora {
        for slot in 1..=MAX_LORA_SLOTS {
            let legacy = float_payload(payload, &format!("strength_{slot}"), 1.0, -100.0, 100.0);
            let first = float_payload(payload, &format!("first_pass_strength_{slot}"), legacy, -100.0, 100.0);
            let second = float_payload(payload, &format!("second_pass_strength_{slot}"), legacy, -100.0, 100.0);
            set(&mut prompt, &lora_node, &format!("lora_{slot}"), clean_lora(env, payload.get(&format!("lora_{slot}"))))?;
            set(&mut prompt, &lora_node, &format!("first_pass_strength_{slot}"), first)?;
            set(&mut prompt, &lora_node, &format!("second_pass_strength_{slot}"), second)?;
        }
    } else {
        set(&mut prompt, &lora_node, "ltx_two_pass_mode", bool_payload(payload, "ltx_two_pass_mode", false))?;
        for slot in 1..=MAX_LORA_SLOTS {
            set(&mut prompt, &lora_node, &format!("lora_{slot}"), clean_lora(env, payload.get(&format!("lora_{slot}"))))?;
            set(&mut prompt, &lora_node, &format!("strength_{slot}"), float_payload(payload, &format!("strength_{slot}"), 1.0, -100.0, 100.0))?;
        }
    }
    Ok(built(&name, prompt, Some(seed)))
}

// -- Krea 2 -------------------------------------------------------------------------------------------------------------

pub fn build_krea2(env: &dyn GraphEnv, payload: &Obj) -> Result<Obj> {
    let (name, mut prompt) = load_template("Krea2_TextToImage_API.json")?;
    let text_value = prompt_text(payload, "Prompt text is empty.")?;
    let (width, height) = (int_payload(payload, "width", 1920, 64, 4096), int_payload(payload, "height", 1080, 64, 4096));
    let (first_width, first_height) = (int_payload(payload, "first_pass_width", 1024, 64, 4096), int_payload(payload, "first_pass_height", 576, 64, 4096));
    let seed = pick_seed(env, payload, 1);
    let use_enhance = bool_payload(payload, "use_zimage_enhance", true);
    let strength = float_payload(payload, "zimage_enhance_strength", 0.5, -100.0, 100.0).clamp(0.1, 1.0);

    let krea_unet = first_text(payload, &["krea_unet_name", "unet_name"], "krea2_turbo_fp8_scaled.safetensors");
    let krea_clip = first_text(payload, &["krea_clip_name", "clip_name"], "qwen3vl_4b_fp8_scaled.safetensors");
    let krea_vae = first_text(payload, &["krea_vae_name", "vae_name"], "qwen_image_vae.safetensors");
    let z_unet = first_text(payload, &["z_unet_name", "enhance_unet_name"], "z_image_turbo_bf16.safetensors");
    let z_clip = first_text(payload, &["z_clip_name", "enhance_clip_name"], "qwen_3_4b.safetensors");
    let z_vae = first_text(payload, &["z_vae_name", "enhance_vae_name"], "ae.safetensors");

    require_model(env, &["diffusion_models", "unet"], &krea_unet, "Krea2 diffusion model")?;
    require_model(env, &["text_encoders", "clip"], &krea_clip, "Krea2 text encoder")?;
    require_model(env, &["vae"], &krea_vae, "Krea2 VAE")?;
    if use_enhance {
        require_model(env, &["unet", "diffusion_models"], &z_unet, "ZImage enhancer diffusion model")?;
        require_model(env, &["clip", "text_encoders"], &z_clip, "ZImage enhancer text encoder")?;
        require_model(env, &["vae"], &z_vae, "ZImage enhancer VAE")?;
    }

    set(&mut prompt, "200", "text", text_value)?;
    set(&mut prompt, "30:10", "unet_name", krea_unet)?;
    set(&mut prompt, "30:11", "clip_name", krea_clip)?;
    set(&mut prompt, "30:12", "vae_name", krea_vae)?;
    set(&mut prompt, "30:3", "seed", seed)?;
    set(&mut prompt, "30:5", "batch_size", int_payload(payload, "batch_size", 1, 1, 16))?;
    set(&mut prompt, "201", "width", first_width)?;
    set(&mut prompt, "201", "height", first_height)?;
    set(&mut prompt, "193:16", "unet_name", z_unet)?;
    set(&mut prompt, "193:18", "clip_name", z_clip)?;
    set(&mut prompt, "193:17", "vae_name", z_vae)?;
    set(&mut prompt, "193:86", "noise_seed", seed)?;
    set(&mut prompt, "193:98", "width", width)?;
    set(&mut prompt, "193:98", "height", height)?;

    // The ZImage branch uses a 10 step partial denoise. A larger strength starts earlier, so ZImage can change more.
    let steps = 10i64;
    let start = round_half_even(steps as f64 * (1.0 - strength)).clamp(0, steps - 1);
    set(&mut prompt, "193:82", "steps", steps)?;
    set(&mut prompt, "193:82", "start_at_step", start)?;
    set(&mut prompt, "193:82", "end_at_step", steps)?;
    if !use_enhance {
        // PreviewImage is the output. Pointing it at the Krea decode drops the unused ZImage branch from the run.
        set(&mut prompt, "199", "images", link("30:8", 0))?;
    }
    if let Some(aspect) = prompt.get_mut("49").and_then(Value::as_object_mut) {
        let inputs = aspect.entry("inputs").or_insert_with(|| json!({}));
        if let Some(inputs) = inputs.as_object_mut() {
            let ratio = f64::from(width as i32) / f64::from((height as i32).max(1));
            if (ratio - 16.0 / 9.0).abs() < 0.04 {
                inputs.insert("aspect_ratio".into(), json!("16:9 (Widescreen)"));
            } else if (ratio - 1.0).abs() < 0.04 {
                inputs.insert("aspect_ratio".into(), json!("1:1 (Square)"));
            } else if ratio < 1.0 {
                inputs.insert("aspect_ratio".into(), json!("9:16 (Portrait)"));
            }
            let megapixels = h3_core::pyval::round_to((first_width * first_height) as f64 / 1_000_000.0, 2).max(0.25);
            inputs.insert("megapixels".into(), json!(megapixels));
        }
    }
    Ok(built(&name, prompt, Some(seed)))
}

pub fn build_krea2_2pass(env: &dyn GraphEnv, payload: &Obj) -> Result<Obj> {
    let (name, mut prompt) = load_template("Krea2_API_2Pass.json")?;
    let text_value = prompt_text(payload, "Krea 2 prompt text is empty.")?;
    let aspect = first_text(payload, &["aspect_ratio"], "16:9 (Widescreen)");
    let batch = int_payload(payload, "batch_size", 1, 1, 16);
    let seed = pick_seed(env, payload, 1);
    let cfg = float_payload(payload, "cfg", 1.2, -100.0, 100.0).clamp(1.0, 1.2);
    let sampler = first_text(payload, &["sampler_name"], "euler_ancestral_cfg_pp");
    let use_i2i = bool_payload(payload, "use_image_to_image", false);
    let creativity = int_payload(payload, "image_to_image_creativity", 5, 0, 10);
    let unet = first_text(payload, &["unet_name"], "krea2_turbo_fp8_scaled.safetensors");
    let clip = first_text(payload, &["clip_name"], "qwen3vl_4b_fp8_scaled.safetensors");
    let vae = first_text(payload, &["vae_name"], "qwen_image_vae.safetensors");
    let use_loras = bool_payload(payload, "use_custom_loras", bool_payload(payload, "use_loras", false));
    let lora_count = if use_loras { int_payload(payload, "lora_count", 0, 0, 20) } else { 0 };

    require_model(env, &["diffusion_models", "unet"], &unet, "Krea 2 diffusion model")?;
    require_model(env, &["text_encoders", "clip"], &clip, "Krea 2 text encoder")?;
    require_model(env, &["vae"], &vae, "Krea 2 VAE")?;
    for slot in 1..=lora_count {
        let lora = clean_lora(env, payload.get(&format!("lora_{slot}")));
        if lora != NONE_LORA {
            require_model(env, &["loras"], &lora, &format!("Krea 2 LoRA {slot}"))?;
        }
    }

    set(&mut prompt, "228", "text", text_value)?;
    set(&mut prompt, "236", "unet_name", unet)?;
    set(&mut prompt, "233", "clip_name", clip)?;
    set(&mut prompt, "234", "vae_name", vae)?;
    set(&mut prompt, "248", "use_custom_loras", use_loras && lora_count > 0)?;
    set(&mut prompt, "248", "lora_count", if use_loras { lora_count } else { 0 })?;
    for slot in 1..=20i64 {
        let mut lora = clean_lora(env, payload.get(&format!("lora_{slot}")));
        let legacy = float_payload(payload, &format!("strength_{slot}"), 1.0, -100.0, 100.0);
        let first = float_payload(payload, &format!("first_pass_strength_{slot}"), legacy, -100.0, 100.0);
        let second = float_payload(payload, &format!("second_pass_strength_{slot}"), legacy, -100.0, 100.0);
        if !use_loras || slot > lora_count {
            lora = NONE_LORA.to_string();
        }
        set(&mut prompt, "248", &format!("lora_{slot}"), lora)?;
        set(&mut prompt, "248", &format!("first_pass_strength_{slot}"), first)?;
        set(&mut prompt, "248", &format!("second_pass_strength_{slot}"), second)?;
    }
    set(&mut prompt, "238", "aspect_ratio", aspect.clone())?;
    set(&mut prompt, "49", "aspect_ratio", aspect)?;
    set(&mut prompt, "240", "batch_size", batch)?;
    set(&mut prompt, "245", "value", creativity)?;
    set(&mut prompt, "242", "switch", use_i2i)?;
    set(&mut prompt, "243", "switch", use_i2i)?;
    set(&mut prompt, "235", "sampler_name", sampler)?;
    for node in ["230", "231"] {
        set(&mut prompt, node, "noise_seed", seed)?;
        set(&mut prompt, node, "cfg", cfg)?;
    }
    set_image_to_image(env, &mut prompt, "249", payload, use_i2i, "Krea 2 image-to-image is enabled, but no source image was provided.")?;
    Ok(built(&name, prompt, Some(seed)))
}

// -- MOSS-Image ---------------------------------------------------------------------------------------------------------

/// MOSS-Image text to image on ComfyUI's native support. A model only LoRA (the 8 step turbo LoRA) is optional. The model
/// already samples with shift 5 and a timestep multiplier of 1, so there is no ModelSamplingSD3 node: it forces a
/// multiplier of 1000 and the picture is noise.
pub fn build_moss_image(env: &dyn GraphEnv, payload: &Obj) -> Result<Obj> {
    let text_value = prompt_text(payload, "Prompt text is empty.")?;
    let unet = first_text(payload, &["unet_name"], "moss_image_sft_bf16.safetensors");
    let clip = first_text(payload, &["clip_name"], "qwen3.5_9b_int8_convrot.safetensors");
    let vae = first_text(payload, &["vae_name"], "flux2-vae.safetensors");
    let lora = first_text(payload, &["lora_name"], NONE_LORA);
    require_model(env, &["diffusion_models", "unet"], &unet, "MOSS-Image diffusion model")?;
    require_model(env, &["text_encoders", "clip"], &clip, "MOSS-Image text encoder")?;
    require_model(env, &["vae"], &vae, "MOSS-Image VAE")?;
    if lora != NONE_LORA {
        require_model(env, &["loras"], &lora, "MOSS-Image LoRA")?;
    }
    let seed = pick_seed(env, payload, 1);
    let cfg = float_payload(payload, "cfg", 1.0, 1.0, 20.0);

    let mut model = json!(["1", 0]);
    let mut prompt = Obj::new();
    prompt.insert("1".into(), json!({"class_type": "UNETLoader", "inputs": {"unet_name": unet, "weight_dtype": "default"}}));
    prompt.insert("2".into(), json!({"class_type": "CLIPLoader", "inputs": {"clip_name": clip, "type": "moss_image", "device": "default"}}));
    prompt.insert("3".into(), json!({"class_type": "VAELoader", "inputs": {"vae_name": vae}}));
    if lora != NONE_LORA {
        prompt.insert("4".into(), json!({"class_type": "LoraLoaderModelOnly", "inputs": {"model": model, "lora_name": lora, "strength_model": float_payload(payload, "lora_strength", 1.0, -10.0, 10.0)}}));
        model = json!(["4", 0]);
    }
    prompt.insert("7".into(), json!({"class_type": "CLIPTextEncode", "inputs": {"clip": ["2", 0], "text": text_value}}));
    prompt.insert("8".into(), json!({"class_type": "CLIPTextEncode", "inputs": {"clip": ["2", 0], "text": text(payload, "negative_prompt")}}));
    prompt.insert("9".into(), json!({"class_type": "EmptyFlux2LatentImage", "inputs": {
        "width": int_payload(payload, "width", 1344, 256, 4096),
        "height": int_payload(payload, "height", 768, 256, 4096),
        "batch_size": int_payload(payload, "batch_size", 1, 1, 16),
    }}));
    prompt.insert("10".into(), json!({"class_type": "KSampler", "inputs": {
        "model": model, "positive": ["7", 0], "negative": ["8", 0], "latent_image": ["9", 0],
        "seed": seed, "steps": int_payload(payload, "steps", 8, 1, 200), "cfg": cfg,
        "sampler_name": "euler", "scheduler": "sgm_uniform", "denoise": 1.0,
    }}));
    prompt.insert("11".into(), json!({"class_type": "VAEDecode", "inputs": {"samples": ["10", 0], "vae": ["3", 0]}}));
    prompt.insert("12".into(), json!({"class_type": "PreviewImage", "inputs": {"images": ["11", 0]}}));
    Ok(built("", prompt, Some(seed)))
}

// -- Ernie --------------------------------------------------------------------------------------------------------------

pub fn build_ernie(env: &dyn GraphEnv, payload: &Obj) -> Result<Obj> {
    let (name, mut prompt) = load_template("image_ernie_image_turbo_API.json")?;
    let text_value = prompt_text(payload, "Prompt text is empty.")?;
    let (width, height) = (int_payload(payload, "width", 1280, 64, 4096), int_payload(payload, "height", 720, 64, 4096));
    let batch = int_payload(payload, "batch_size", 1, 1, 16);
    let seed = pick_seed(env, payload, 1);
    let use_i2i = bool_payload(payload, "use_image_to_image", false);
    let start_at = int_payload(payload, "image_to_image_start_at_step", 5, 1, 8);

    set(&mut prompt, "111", "text", text_value)?;
    set(&mut prompt, "105", "unet_name", text(payload, "unet_name"))?;
    set(&mut prompt, "108", "clip_name", text(payload, "clip_name"))?;
    set(&mut prompt, "109", "vae_name", text(payload, "vae_name"))?;
    for node in ["104", "120"] {
        set(&mut prompt, node, "width", width)?;
        set(&mut prompt, node, "height", height)?;
        set(&mut prompt, node, "batch_size", batch)?;
    }
    set(&mut prompt, "121", "noise_seed", seed)?;
    set(&mut prompt, "114", "switch", use_i2i)?;
    set(&mut prompt, "117", "switch", use_i2i)?;
    set(&mut prompt, "115", "value", start_at)?;
    set(&mut prompt, "118", "image", env.ensure_placeholder_image().map_err(GraphError::new)?)?;
    set_image_to_image(env, &mut prompt, "118", payload, use_i2i, "Image-to-image is enabled, but no source image was provided.")?;
    set_basic_loras(env, &mut prompt, "113", payload, Some(false))?;
    Ok(built(&name, prompt, Some(seed)))
}

// -- Flux Klein and NanoBanana ------------------------------------------------------------------------------------------

/// The pictures a request lists (`image_ingredients` or `images`, as a list or as JSON or line separated text), as file
/// paths. Pictures sent as data are written into ComfyUI's input folder first.
fn ingredient_paths(env: &dyn GraphEnv, payload: &Obj, label: &str, kind: &str, found_label: &str) -> Result<Vec<String>> {
    let raw = match payload.get("image_ingredients").filter(|v| truthy(v)).or_else(|| payload.get("images").filter(|v| truthy(v))) {
        Some(v) => v.clone(),
        None => json!([]),
    };
    let items: Vec<Value> = match raw {
        Value::String(s) => match serde_json::from_str::<Value>(&s) {
            Ok(Value::Array(a)) => a,
            Ok(_) => return fail(format!("{kind} must be a list.")),
            Err(_) => s.lines().map(str::trim).filter(|l| !l.is_empty()).map(|l| json!({"path": l})).collect(),
        },
        Value::Array(a) => a,
        _ => return fail(format!("{kind} must be a list.")),
    };
    let mut paths = Vec::new();
    for (index, item) in items.into_iter().enumerate() {
        let item = match item {
            Value::String(s) => json!({"path": s}),
            other => other,
        };
        let Some(item) = item.as_object() else { continue };
        let raw_path = trimmed(item, "path");
        let raw_data = trimmed(item, "data");
        let fallback = format!("{}_{}.png", label.replace(' ', "_"), index + 1);
        let name = Some(trimmed(item, "name")).filter(|n| !n.is_empty()).unwrap_or(fallback);
        if !raw_data.is_empty() {
            let loaded = env.prepare_load_image("", &raw_data, &name).map_err(GraphError::new)?;
            paths.push(abs(&Path::new(&env.input_dir()).join(loaded)));
        } else if !raw_path.is_empty() {
            let found = existing_file(env, &raw_path, &format!("{found_label} {}", index + 1))?;
            paths.push(abs(Path::new(&found)));
        }
    }
    Ok(paths)
}

fn abs(path: &Path) -> String {
    let full: PathBuf = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    full.components().collect::<PathBuf>().display().to_string()
}

/// `_resolve_existing_file`: a path as it is, or looked for in ComfyUI's input, output and temp folders.
fn existing_file(env: &dyn GraphEnv, raw: &str, label: &str) -> Result<String> {
    let text = raw.trim().trim_matches(['"', '\'']).to_string();
    if text.is_empty() {
        return fail(format!("{label} path is empty."));
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if Path::new(&text).is_absolute() {
        candidates.push(PathBuf::from(&text));
    } else {
        candidates.push(PathBuf::from(&text));
        candidates.push(PathBuf::from(abs(Path::new(&text))));
        candidates.push(Path::new(&env.input_dir()).join(&text));
        candidates.push(Path::new(&env.output_dir()).join(&text));
    }
    for candidate in candidates {
        let normalized = abs(&candidate);
        if env.is_file(&normalized) {
            return Ok(normalized);
        }
    }
    fail(format!("{label} was not found: {text}"))
}

pub fn build_flux_klein(env: &dyn GraphEnv, payload: &Obj) -> Result<Obj> {
    let (name, mut prompt) = load_template("fluxKleinMultiImage_API.json")?;
    let text_value = prompt_text(payload, "Flux/Klein prompt text is empty.")?;
    let image_paths = ingredient_paths(env, payload, "ingredient", "Flux/Klein image ingredients", "Flux/Klein ingredient image")?;
    let (width, height) = (int_payload(payload, "width", 1024, 64, 4096), int_payload(payload, "height", 576, 64, 4096));
    let seed = seed_of(payload, 100);

    set(&mut prompt, "1067", "text", text_value)?;
    for node in ["1065", "1052"] {
        if prompt.contains_key(node) {
            set(&mut prompt, node, "width", width)?;
            set(&mut prompt, node, "height", height)?;
        }
    }
    if prompt.contains_key("1057") {
        set(&mut prompt, "1057", "width", width)?;
        set(&mut prompt, "1057", "height", height)?;
        set(&mut prompt, "1057", "batch_size", 1)?;
    }
    set(&mut prompt, "1056", "noise_seed", seed)?;
    set(&mut prompt, "1068", "unet_name", text(payload, "unet_name"))?;
    set(&mut prompt, "1066", "clip_name", text(payload, "clip_name"))?;
    set(&mut prompt, "1064", "vae_name", text(payload, "vae_name"))?;
    let lora_node = node_id_by_class(&prompt, "VRGDG_OptionalMultiLoraModelOnly", Some("1075"))?;
    set(&mut prompt, &lora_node, "use_custom_loras", bool_payload(payload, "use_custom_loras", false))?;
    set(&mut prompt, &lora_node, "lora_count", int_payload(payload, "lora_count", 0, 0, MAX_LORA_SLOTS))?;
    if crate::api::inputs(&prompt, &lora_node).is_some_and(|i| i.contains_key("ltx_two_pass_mode")) {
        set(&mut prompt, &lora_node, "ltx_two_pass_mode", false)?;
    }
    for slot in 1..=MAX_LORA_SLOTS {
        set(&mut prompt, &lora_node, &format!("lora_{slot}"), clean_lora(env, payload.get(&format!("lora_{slot}"))))?;
        set(&mut prompt, &lora_node, &format!("strength_{slot}"), float_payload(payload, &format!("strength_{slot}"), 1.0, -100.0, 100.0))?;
    }
    if image_paths.is_empty() {
        if prompt.contains_key("1053") {
            set(&mut prompt, "1053", "positive", link("1067", 0))?;
            set(&mut prompt, "1053", "negative", link("1058", 0))?;
        }
        prompt.shift_remove("1072");
        prompt.shift_remove("1059");
    } else {
        let items: Vec<Value> = image_paths.iter().map(|p| json!(p)).collect();
        set(&mut prompt, "1072", "image_paths", json_list_text(&items))?;
    }
    Ok(built(&name, prompt, None))
}

fn looks_like_prompt(value: &str) -> bool {
    let value = value.trim();
    value.chars().count() > 20 && value.chars().any(char::is_whitespace)
}

fn looks_like_api_key(value: &str) -> bool {
    let value = value.trim();
    value.chars().count() >= 20 && !value.chars().any(char::is_whitespace)
}

pub fn build_nb_image(env: &dyn GraphEnv, payload: &Obj) -> Result<Obj> {
    let (name, mut prompt) = load_template("NB_API.json")?;
    let mut text_value = trimmed(payload, "prompt");
    let mut api_key = trimmed(payload, "api_key");
    // The key and the prompt are sometimes pasted into each other's boxes.
    if looks_like_prompt(&api_key) && looks_like_api_key(&text_value) {
        std::mem::swap(&mut api_key, &mut text_value);
    }
    if text_value.is_empty() {
        return fail("NanoBanana prompt text is empty.");
    }
    if api_key.is_empty() {
        return fail("NanoBanana needs an API key.");
    }
    if api_key.chars().any(char::is_whitespace) {
        return fail("NanoBanana API key looks invalid. It appears to contain prompt text; paste the Google API key into the NanoBanana API key field.");
    }
    let image_paths = ingredient_paths(env, payload, "NanoBanana reference image", "Nanobanana Reference Images", "Nanobanana Reference Image")?;
    let nb_node = node_id_by_class(&prompt, "VRGDG_NanoBananaPro", Some("1"))?;
    let loader = node_id_by_class(&prompt, "VRGDG_ImageBatchMultiFromPaths", Some("3"))?;
    set(&mut prompt, &nb_node, "api_key", api_key)?;
    set(&mut prompt, &nb_node, "prompt", text_value)?;
    set(&mut prompt, &nb_node, "model", first_text(payload, &["model"], "gemini-3-pro-image-preview"))?;
    if image_paths.is_empty() {
        if let Some(inputs) = prompt.get_mut(&nb_node).and_then(|n| n.get_mut("inputs")).and_then(Value::as_object_mut) {
            inputs.shift_remove("image1");
        }
        prompt.shift_remove(&loader);
    } else {
        let items: Vec<Value> = image_paths.iter().map(|p| json!(p)).collect();
        set(&mut prompt, &loader, "image_paths", json_list_text(&items))?;
    }
    Ok(built(&name, prompt, None))
}

// -- Z upscale and enhance ----------------------------------------------------------------------------------------------

pub fn build_z_upscale_enhance(env: &dyn GraphEnv, payload: &Obj) -> Result<Obj> {
    let (name, mut prompt) = load_template("z_upscaleEnhance_API.json")?;
    let text_value = trimmed(payload, "prompt");
    let (width, height) = (int_payload(payload, "width", 1920, 64, 4096), int_payload(payload, "height", 1080, 64, 4096));
    let seed = pick_seed(env, payload, 1);
    let amount = int_payload(payload, "enhance_amount", 8, 1, 20);
    let image = env
        .prepare_load_image(&text(payload, "source_image_path"), &text(payload, "source_image_data"), &match text(payload, "source_image_name") {
            n if n.is_empty() => "source.png".to_string(),
            n => n,
        })
        .map_err(GraphError::new)?;
    if image.is_empty() {
        return fail("Upscale/enhance needs a source image.");
    }
    set(&mut prompt, "960", "clip_name", text(payload, "clip_name"))?;
    set(&mut prompt, "961", "vae_name", text(payload, "vae_name"))?;
    set(&mut prompt, "972", "unet_name", text(payload, "unet_name"))?;
    set(&mut prompt, "971", "text", text_value)?;
    set(&mut prompt, "967", "width", width)?;
    set(&mut prompt, "967", "height", height)?;
    set(&mut prompt, "979", "image", image)?;
    set(&mut prompt, "983", "value", amount)?;
    set(&mut prompt, "964", "noise_seed", seed)?;
    set_basic_loras(env, &mut prompt, "974", payload, Some(false))?;
    Ok(built(&name, prompt, Some(seed)))
}

/// Pick the builder for an image model key.
pub fn build_image(env: &dyn GraphEnv, model: &str, payload: &Obj) -> Result<Obj> {
    match model {
        "zimage" => build_zimage(env, payload),
        "krea2" => build_krea2(env, payload),
        "krea2_2pass" => build_krea2_2pass(env, payload),
        "moss_image" => build_moss_image(env, payload),
        "ernie_image" => build_ernie(env, payload),
        "flux_klein" => build_flux_klein(env, payload),
        "nb_image" => build_nb_image(env, payload),
        "z_upscale_enhance" => build_z_upscale_enhance(env, payload),
        other => fail(format!("Unknown image model: {other}")),
    }
}
