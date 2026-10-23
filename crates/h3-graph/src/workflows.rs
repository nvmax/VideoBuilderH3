//! The three graph builders: single pass, 2 Pass and 2 Pass Advanced. Port of `runner/minimax_workflows.py`. Each one
//! loads its template, writes the scene's prompt, audio, references, models and sampler settings into it, applies the
//! optional patches and returns the graph with a summary of what it did. The older fixed 3 pass builder is not ported:
//! the Video Builder never reaches it.

use h3_core::pyval::{
    bool_payload, first_value, float_payload, int_payload, text_or_empty, to_float, truthy, Obj,
};
use h3_core::resolution::frame_size;
use h3_core::tiles::{normalize_vram_preset, plan_spatial_tiles};
use h3_core::timing::{calculate_timing, TimingInput, TimingPlan};
use serde_json::{json, Value};

use crate::api::{
    as_link, compat_node_inputs, inputs, link, node_id_by_class, set_input,
};
use crate::env::{GraphEnv, GraphError, Result};
use crate::inputs::{
    abspath, audio_mode, clean_path_text, image_paths, json_list_text, mode_text, output_location, patch_i2v_keyframe_timing,
    patch_image_to_video_node, template, video_references, ASPECT_RATIOS,
};
use crate::patches::{
    clean_lora_name, cooldown_frames, effective_warmup_frames, is_latent_mode, latent_continuation_mode, model_choice_exists,
    patch_advanced_settings, patch_fast_decode, patch_latent_continuation, patch_loras, patch_memory_efficient_sage_attention,
    patch_optional_model_paths, patch_save_latent, patch_te_speed, patch_turbo, require_model_choice, text_or, NONE_LORA,
};

// -- shared pieces ------------------------------------------------------------------------------------------------------

fn obj(value: Value) -> Obj {
    value.as_object().cloned().unwrap_or_default()
}

fn text(value: Option<&Value>) -> String {
    text_or_empty(value).trim().to_string()
}

/// `str(payload.get(key) or default).strip()`.
fn named(payload: &Obj, key: &str, default: &str) -> String {
    match payload.get(key) {
        Some(v) if truthy(v) => text(Some(v)),
        _ => default.to_string(),
    }
}

/// A number from a loose value for the timing plan, with the plan's own wording for what is wrong.
fn timing_number(value: &Value, name: &str) -> Result<f64> {
    to_float(value).filter(|f| f.is_finite()).ok_or_else(|| GraphError::new(format!("{name} must be a finite number.")))
}

fn pyrepr_list(items: &[String]) -> String {
    format!("[{}]", items.iter().map(|i| format!("'{i}'")).collect::<Vec<_>>().join(", "))
}

/// The scene's audio file, project folder, scene number and timing plan.
struct SceneContext {
    audio_mode: &'static str,
    project: String,
    scene: u32,
    timing: TimingPlan,
    prepared_audio: Option<Value>,
}

/// Validate the scene inputs shared by every builder and plan the render and trim timing. `label` names the builder in
/// messages (`MiniMax H3` or `MiniMax H3 two-pass`).
fn scene_context(env: &dyn GraphEnv, payload: &Obj, label: &str, tolerate_bad_end: bool) -> Result<SceneContext> {
    let audio_mode = audio_mode(payload);
    let mut audio_path = String::new();
    if audio_mode == "input_audio" {
        let raw = clean_path_text(first_value(payload, &["audio_path", "source_audio_path"]).filter(|v| truthy(v)));
        if raw.is_empty() {
            return Err(GraphError::new(format!("{label} source audio path is empty.")));
        }
        audio_path = abspath(&raw);
        if !env.is_file(&audio_path) {
            return Err(GraphError::new(format!("{label} source audio was not found: {audio_path}")));
        }
    }
    let project_text = clean_path_text(payload.get("project_folder"));
    if project_text.is_empty() {
        return Err(GraphError::new("Project folder is empty."));
    }
    let project = abspath(&project_text);
    if !env.is_dir(&project) {
        return Err(GraphError::new(format!("Project folder was not found: {project}")));
    }
    let scene = int_payload(payload, "scene_number", 1, 1, 999_999) as u32;

    let start_value = first_value(payload, &["timeline_start_seconds", "scene_start_seconds", "start"]).cloned().unwrap_or(json!(0));
    let end_value = first_value(payload, &["timeline_end_seconds", "scene_end_seconds", "end"]).cloned();
    let end_value = match end_value {
        Some(v) => v,
        None => {
            let duration = first_value(payload, &["scene_duration_seconds", "scene_duration", "duration"]);
            let Some(duration) = duration else {
                return Err(GraphError::new(format!("{label} needs timeline_end_seconds or scene_duration_seconds.")));
            };
            let (start, length) = (to_float(&start_value), to_float(duration));
            match (start, length) {
                (Some(s), Some(d)) => json!(s + d),
                _ if tolerate_bad_end => return Err(GraphError::new(format!("{label} timeline timing must be numeric."))),
                _ => return Err(GraphError::new("could not convert string to float")),
            }
        }
    };
    let source_duration = match first_value(payload, &["source_duration_seconds", "audio_duration_seconds"]) {
        Some(v) => Some(timing_number(v, "source_duration_seconds")?),
        None if audio_mode == "input_audio" => Some(env.probe_duration(&audio_path).map_err(GraphError::new)?),
        None => None,
    };
    let source_start = match first_value(payload, &["source_start_seconds", "audio_start_seconds"]) {
        Some(v) => Some(timing_number(v, "source_start_seconds")?),
        None => None,
    };
    let warmup = effective_warmup_frames(env, payload);
    let pad_warmup = audio_mode == "built_in_audio" || (is_latent_mode(&latent_continuation_mode(payload)) && scene > 1);
    let timing = calculate_timing(&TimingInput {
        timeline_start_seconds: timing_number(&start_value, "timeline_start_seconds")?,
        timeline_end_seconds: timing_number(&end_value, "timeline_end_seconds")?,
        warmup_frames: warmup as f64,
        cooldown_frames: cooldown_frames(payload),
        source_start_seconds: source_start,
        source_duration_seconds: source_duration,
        max_frame_count: None,
        pad_warmup,
    })?;
    let prepared_audio = if audio_mode == "input_audio" {
        Some(env.trim_audio(&audio_path, &project, scene, &timing).map_err(GraphError::new)?)
    } else {
        None
    };
    Ok(SceneContext { audio_mode, project, scene, timing, prepared_audio })
}

/// A seed setting: negative means random, and the value is capped to 64 bits. Seeds can be larger than an `i64`.
fn seed_value(env: &dyn GraphEnv, payload: &Obj, key: &str, default: i64) -> u64 {
    let parsed: Option<i128> = match payload.get(key) {
        None => Some(i128::from(default)),
        Some(Value::Number(n)) => n.as_i64().map(i128::from).or_else(|| n.as_u64().map(i128::from)).or_else(|| n.as_f64().map(|f| f.trunc() as i128)),
        Some(Value::String(s)) => s.trim().parse::<i128>().ok(),
        Some(Value::Bool(b)) => Some(i128::from(*b)),
        Some(_) => None,
    };
    let value = parsed.unwrap_or(i128::from(default));
    if value < 0 {
        return env.random_seed();
    }
    u64::try_from(value).unwrap_or(u64::MAX)
}

fn video_prompt(payload: &Obj, keys: &[&str], empty_message: &str) -> Result<String> {
    let prompt = text(first_value(payload, keys).filter(|v| truthy(v)));
    if prompt.is_empty() { Err(GraphError::new(empty_message)) } else { Ok(prompt) }
}

fn check_aspect(payload: &Obj, message: &str) -> Result<String> {
    let aspect = named(payload, "aspect_ratio", "16:9 (Widescreen)");
    if !ASPECT_RATIOS.contains(&aspect.as_str()) {
        return Err(GraphError::new(format!("{message}{aspect}")));
    }
    Ok(aspect)
}

fn model_link(prompt: &Obj, node_id: &str) -> Option<Value> {
    inputs(prompt, node_id).and_then(|i| i.get("model")).filter(|m| as_link(m).is_some()).cloned()
}

fn set_models(prompt: &mut Obj, ids: &[&str], model: &Value) -> Result<()> {
    for id in ids {
        set_input(prompt, id, "model", model.clone())?;
    }
    Ok(())
}

// -- single pass ------------------------------------------------------------------------------------------------------------

/// Build the single pass graph.
pub fn build_single(env: &dyn GraphEnv, payload: &Obj) -> Result<Obj> {
    let mut payload = payload.clone();
    if bool_payload(&payload, "use_memory_efficient_sage_attention", false) {
        env.check_memory_efficient_sage_attention().map_err(GraphError::new)?;
    }
    let audio = audio_mode(&payload);
    let template_name =
        if audio == "built_in_audio" { "minimax_built_in_audio_builder_api.json" } else { "minimax_audio_driven_builder_api.json" };
    let (workflow_path, mut prompt) = template(template_name)?;

    let video_prompt = video_prompt(&payload, &["prompt", "video_prompt", "i2v_prompt", "t2v_prompt"], "MiniMax H3 video prompt is empty.")?;
    let ctx = scene_context(env, &payload, "MiniMax H3", true)?;
    let timing = ctx.timing.clone();

    let mut image_paths = image_paths(&payload)?;
    let mut video_refs = video_references(&payload)?;
    let video_mode = mode_text(&payload, &["video_mode", "mode"], "text_to_video");
    if video_mode == "image_to_video" || video_mode == "image_reference_to_video" {
        if image_paths.is_empty() {
            return Err(GraphError::new("MiniMax H3 image-to-video requires a scene image as the first frame."));
        }
        let mut last_frame = clean_path_text(payload.get("last_frame_path"));
        if !last_frame.is_empty() {
            last_frame = abspath(&last_frame);
            if !env.is_file(&last_frame) {
                return Err(GraphError::new(format!("MiniMax H3 last-frame image was not found: {last_frame}")));
            }
        }
        if video_mode == "image_reference_to_video" {
            let start = image_paths[0].clone();
            let rest: Vec<String> = image_paths[1..].to_vec();
            if !last_frame.is_empty() && abspath(&start) == last_frame {
                last_frame.clear();
            }
            let mut combined = vec![start];
            if !last_frame.is_empty() {
                combined.push(last_frame.clone());
            }
            combined.extend(rest);
            image_paths = combined;
            patch_image_to_video_node(&mut prompt, &image_paths, true, !last_frame.is_empty())?;
        } else {
            if !last_frame.is_empty() {
                image_paths = vec![image_paths[0].clone(), last_frame];
            }
            patch_image_to_video_node(&mut prompt, &image_paths, false, false)?;
        }
        video_refs.clear();
    }
    let aspect_ratio = check_aspect(&payload, "Unsupported MiniMax H3 aspect ratio: ")?;
    let megapixels = float_payload(&payload, "megapixels", 0.9, 0.1, 16.0);
    let diffusion = named(&payload, "diffusion_model_name", "minimax_h3_ref2va_pruned_int8_convrot.safetensors");
    let clip = named(&payload, "clip_name", "qwen3vl_32b_minimax_h3_nvfp4_awq.safetensors");
    let video_vae = named(&payload, "video_vae_name", "minimax_h3_video_vae_fp16.safetensors");
    let audio_vae = named(&payload, "audio_vae_name", "minimax_h3_audio_vae_fp32.safetensors");
    if diffusion.to_lowercase().ends_with(".gguf") {
        return Err(GraphError::new("MiniMax H3 GGUF loading is not enabled yet. Choose a non-GGUF diffusion model."));
    }
    require_model_choice(env, &["diffusion_models", "unet"], &diffusion, "MiniMax H3 diffusion model")?;
    require_model_choice(env, &["text_encoders", "clip"], &clip, "MiniMax H3 text encoder")?;
    require_model_choice(env, &["vae"], &video_vae, "MiniMax H3 video VAE")?;
    require_model_choice(env, &["vae"], &audio_vae, "MiniMax H3 audio VAE")?;

    let seed = seed_value(env, &payload, "seed", 69);
    let (output_folder, filename_prefix) = output_location(env, &ctx.project, ctx.scene, true);
    set_input(&mut prompt, "132", "value", json!(timing.workflow_duration_input_seconds))?;
    set_input(&mut prompt, "138", "value", json!(video_prompt))?;
    set_input(&mut prompt, "129", "noise_seed", json!(seed))?;
    set_input(&mut prompt, "115", "aspect_ratio", json!(aspect_ratio))?;
    set_input(&mut prompt, "115", "megapixels", json!(megapixels))?;
    set_input(&mut prompt, "115", "multiple", json!(32))?;
    set_input(&mut prompt, "141", "model_name", json!(diffusion))?;
    set_input(&mut prompt, "128", "clip_name", json!(clip))?;
    set_input(&mut prompt, "119", "vae_name", json!(video_vae))?;
    set_input(&mut prompt, "120", "vae_name", json!(audio_vae))?;
    if ctx.audio_mode == "input_audio" {
        let prepared = ctx.prepared_audio.as_ref().and_then(|p| p.get("audio_path")).cloned().unwrap_or(Value::Null);
        set_input(&mut prompt, "171", "audio_file", prepared)?;
        set_input(&mut prompt, "171", "seek_seconds", json!(0))?;
        set_input(&mut prompt, "171", "duration", json!(0))?;
    }
    let image_values: Vec<Value> = image_paths.iter().map(|p| json!(p)).collect();
    let video_values: Vec<Value> = video_refs.iter().map(|v| Value::Object(v.clone())).collect();
    set_input(&mut prompt, "180", "image_paths", json!(json_list_text(&image_values)))?;
    set_input(&mut prompt, "180", "video_references", json!(json_list_text(&video_values)))?;
    set_input(&mut prompt, "142", "frame_rate", json!(24))?;
    set_input(&mut prompt, "142", "filename_prefix", json!(filename_prefix))?;
    // Keep every aligned H3 frame. VHS trim_to_audio muxes with -shortest while stream-copying H.264, which can discard
    // final video packets before the exact scene trimmer receives them.
    set_input(&mut prompt, "142", "trim_to_audio", json!(false))?;
    if named(&payload, "video_mode", "reference_to_video") == "reference_to_video" {
        payload.insert("easy_cache_bypass".into(), json!(true));
        payload.insert("use_turbo_lora".into(), json!(false));
    }
    let mut advanced = patch_advanced_settings(&mut prompt, &payload)?;
    if bool_payload(&payload, "use_memory_efficient_sage_attention", false) {
        set_input(&mut prompt, "141", "sage_attention", json!("disabled"))?;
        advanced.insert("sage_attention".into(), json!("disabled"));
    }
    let lora_settings = patch_loras(env, &mut prompt, &payload)?;
    let turbo = patch_turbo(env, &mut prompt, &payload)?;
    let mem_eff = patch_memory_efficient_sage_attention(&mut prompt, &payload)?;
    let mut optional_patch_nodes: Vec<Value> = Vec::new();
    let use_ff = bool_payload(&payload, "use_feedforward", false);
    let use_bs = bool_payload(&payload, "use_block_sparse_attention", false);
    if use_ff || use_bs {
        let scheduler = node_id_by_class(&prompt, "BasicScheduler", Some("124"))?;
        let guider = node_id_by_class(&prompt, "BasicGuider", Some("126"))?;
        let Some(model) = model_link(&prompt, &scheduler) else {
            return Err(GraphError::new("MiniMax H3 optional model patches could not find the current model connection."));
        };
        let mut branches = vec![("single_pass".to_string(), model)];
        optional_patch_nodes = patch_optional_model_paths(env, &mut prompt, &mut branches, use_ff, use_bs)?;
        set_models(&mut prompt, &[&scheduler, &guider], &branches[0].1)?;
    }
    if bool_payload(&payload, "use_te_speed", false) {
        let scheduler = node_id_by_class(&prompt, "BasicScheduler", Some("124"))?;
        let guider = node_id_by_class(&prompt, "BasicGuider", Some("126"))?;
        let current = inputs(&prompt, &scheduler).and_then(|i| i.get("model")).cloned().unwrap_or(Value::Null);
        let patched = patch_te_speed(&mut prompt, &current, &payload, "9210");
        set_models(&mut prompt, &[&scheduler, &guider], &patched)?;
    }
    if bool_payload(&payload, "use_fast_vae_decode", false) {
        patch_fast_decode(env, &mut prompt)?;
    }
    if turbo.get("enabled").and_then(Value::as_bool) == Some(true) {
        advanced.insert("effective_sampler_name".into(), json!("MiniMaxH3TurboSampler"));
        advanced.insert("effective_scheduler".into(), json!("simple"));
        advanced.insert("effective_steps".into(), turbo.get("steps").cloned().unwrap_or(json!(0)));
    }

    let continuation = patch_latent_continuation(env, &mut prompt, &payload)?;
    let save_latent = patch_save_latent(env, &mut prompt, &payload, Some(&timing))?;
    let refmod = apply_refmod_if_requested(env, &mut prompt, &payload)?;
    if video_mode == "image_to_video" {
        patch_i2v_keyframe_timing(&mut prompt, &timing);
    }
    let video_audio = video_refs.iter().filter(|v| v.get("use_audio").and_then(Value::as_bool) == Some(true)).count();
    Ok(obj(json!({
        "workflow_path": workflow_path,
        "output_folder": output_folder,
        "prompt": prompt,
        "refmod": refmod,
        "latent_continuation_settings": continuation,
        "save_latent_settings": save_latent,
        "used_seed": seed,
        "audio_mode": ctx.audio_mode,
        "timing": timing,
        "prepared_audio": ctx.prepared_audio,
        "post_render_trim": {
            "start": timing.final_trim_start_seconds,
            "duration": timing.final_trim_duration_seconds,
            "frames": timing.final_frame_count,
        },
        "reference_inputs": {"image_count": image_paths.len(), "video_count": video_refs.len(), "video_audio_count": video_audio},
        "model_settings": {
            "diffusion_model_name": diffusion,
            "clip_name": clip,
            "video_vae_name": video_vae,
            "audio_vae_name": audio_vae,
        },
        "advanced_settings": advanced,
        "lora_settings": lora_settings,
        "turbo_settings": turbo,
        "memory_efficient_sage_attention": mem_eff,
        "optional_patch_nodes": optional_patch_nodes,
    })))
}

/// Rewire the built graph to the RefMod pipeline when the render asks for it. `null` otherwise.
fn apply_refmod_if_requested(env: &dyn GraphEnv, prompt: &mut Obj, payload: &Obj) -> Result<Value> {
    if crate::refmod::refmod_requested(payload) {
        return crate::refmod::apply_refmod_pipeline(prompt, payload, &|name| env.refmod_entry(name));
    }
    Ok(Value::Null)
}

// -- native audio -------------------------------------------------------------------------------------------------------------

/// Rewire the audio driven 2 pass graph to use H3's own joint video and audio latent (built-in audio). The template
/// reads a project audio file and locks the pass 1 latent to it through the audio drive node. For built-in audio those
/// inputs go away, everything that read the audio locked latent reads the H3 conditioning node's own latent, and the
/// pass 1 audio is decoded and becomes the saved video's audio. Nodes are found by class and link, not by id, so the
/// rewire survives a template renumbering.
fn use_native_audio(prompt: &mut Obj) -> Result<()> {
    let by_class = |prompt: &Obj, class: &str| -> Vec<String> {
        prompt.iter().filter(|(_, n)| n.get("class_type").and_then(Value::as_str) == Some(class)).map(|(k, _)| k.clone()).collect()
    };
    let drives = by_class(prompt, "VRGDG_MiniMaxH3AudioDrive");
    if drives.is_empty() {
        return Err(GraphError::new("The MiniMax H3 two-pass template changed: built-in audio needs an audio drive node."));
    }
    let mut h3_nodes: Vec<String> = Vec::new();
    for d in &drives {
        if let Some((id, 1)) = inputs(prompt, d).and_then(|i| i.get("av_latent")).and_then(as_link) {
            if !h3_nodes.contains(&id) {
                h3_nodes.push(id);
            }
        }
    }
    let audio_vaes: Vec<Value> =
        drives.iter().filter_map(|d| inputs(prompt, d).and_then(|i| i.get("audio_vae")).filter(|v| v.is_array()).cloned()).collect();
    if h3_nodes.len() != 1 || audio_vaes.is_empty() {
        return Err(GraphError::new(
            "The MiniMax H3 two-pass template changed: the audio drive must read one H3 node's latent and the audio VAE.",
        ));
    }
    let h3 = h3_nodes.remove(0);
    let samplers: Vec<String> = by_class(prompt, "SamplerCustomAdvanced")
        .into_iter()
        .filter(|key| {
            inputs(prompt, key).and_then(|i| i.get("latent_image")).and_then(as_link).is_some_and(|(id, _)| drives.contains(&id))
        })
        .collect();
    if samplers.len() != 1 {
        return Err(GraphError::new(format!(
            "The MiniMax H3 two-pass template changed: expected one pass-1 sampler reading the audio drive, found {}.",
            pyrepr_list(&samplers)
        )));
    }
    let mut loaders: Vec<String> = Vec::new();
    for d in &drives {
        if let Some((id, _)) = inputs(prompt, d).and_then(|i| i.get("source_audio")).and_then(as_link) {
            if !loaders.contains(&id) {
                loaders.push(id);
            }
        }
    }
    loaders.sort();
    if let Some(map) = prompt.get_mut(&h3).and_then(|n| n.get_mut("inputs")).and_then(Value::as_object_mut) {
        let names: Vec<String> = map.keys().filter(|k| k.starts_with("ref_audios.")).cloned().collect();
        for name in names {
            map.shift_remove(&name);
        }
    }
    for n in prompt.values_mut() {
        let Some(map) = n.get_mut("inputs").and_then(Value::as_object_mut) else { continue };
        for value in map.values_mut() {
            if as_link(value).is_some_and(|(id, index)| drives.contains(&id) && index == 0) {
                *value = link(&h3, 1);
            }
        }
    }
    for key in drives.iter().chain(loaders.iter()) {
        prompt.shift_remove(key);
    }
    let next = prompt.keys().filter_map(|k| k.parse::<i64>().ok()).max().unwrap_or(0) + 1;
    let decode = next.to_string();
    prompt.insert(
        decode.clone(),
        json!({
            "class_type": "LTXVAudioVAEDecode",
            "_meta": {"title": "Decode MiniMax H3 native audio (pass 1)"},
            "inputs": {"samples": [samplers[0], 0], "audio_vae": audio_vaes[0]},
        }),
    );
    for key in by_class(prompt, "VHS_VideoCombine") {
        set_input(prompt, &key, "audio", link(&decode, 0))?;
    }
    let mut dangling: Vec<String> = Vec::new();
    for (key, n) in prompt.iter() {
        let Some(map) = n.get("inputs").and_then(Value::as_object) else { continue };
        for (name, value) in map {
            if let Some(list) = value.as_array() {
                if list.len() == 2 {
                    if let Some(id) = list[0].as_str() {
                        if !prompt.contains_key(id) {
                            dangling.push(format!("('{key}', '{name}')"));
                        }
                    }
                }
            }
        }
    }
    if !dangling.is_empty() {
        return Err(GraphError::new(format!("The built-in audio rewire left dangling links: [{}]", dangling.join(", "))));
    }
    Ok(())
}

// -- 2 Pass ---------------------------------------------------------------------------------------------------------------------

/// Build the 2 Pass graph (reference to video, image to video and image plus reference).
pub fn build_two_pass(env: &dyn GraphEnv, payload: &Obj) -> Result<Obj> {
    let video_mode = mode_text(payload, &["video_mode", "mode"], "reference_to_video");
    let template_name = if video_mode == "image_to_video" {
        "minimax_i2v_audio_driven_builder_latent_upscale_2pass_api.json"
    } else {
        "minimax_audio_driven_builder_latent_upscale_2pass_api.json"
    };
    let (workflow_path, mut prompt) = template(template_name)?;
    let video_prompt = video_prompt(payload, &["prompt", "video_prompt"], "MiniMax H3 two-pass video prompt is empty.")?;
    if video_mode == "image_to_video" && audio_mode(payload) == "built_in_audio" {
        return Err(GraphError::new("MiniMax H3 Image to Video 2 Pass currently supports Input Audio only."));
    }
    let ctx = scene_context(env, payload, "MiniMax H3 two-pass", false)?;
    let timing = ctx.timing.clone();

    let mut image_paths = image_paths(payload)?;
    let mut video_refs = video_references(payload)?;
    if video_mode == "image_to_video" || video_mode == "image_reference_to_video" {
        if image_paths.is_empty() {
            return Err(GraphError::new("MiniMax H3 image-to-video requires a scene image as the first frame."));
        }
        let start_frame = image_paths[0].clone();
        let rest: Vec<String> = image_paths[1..].to_vec();
        let mut last_frame = clean_path_text(payload.get("last_frame_path"));
        if !last_frame.is_empty() {
            last_frame = abspath(&last_frame);
            if !env.is_file(&last_frame) {
                return Err(GraphError::new(format!("MiniMax H3 last-frame image was not found: {last_frame}")));
            }
            if video_mode != "image_to_video" && abspath(&start_frame) == last_frame {
                last_frame.clear();
            }
        }
        let mut combined = vec![start_frame];
        if !last_frame.is_empty() {
            combined.push(last_frame.clone());
        }
        if video_mode == "image_reference_to_video" {
            combined.extend(rest);
        }
        image_paths = combined;
        patch_image_to_video_node(&mut prompt, &image_paths, video_mode == "image_reference_to_video", !last_frame.is_empty())?;
        set_input(&mut prompt, "136", "width", link("186", 1))?;
        set_input(&mut prompt, "136", "height", link("187", 1))?;
        video_refs.clear();
    }
    let default_model = if video_mode == "image_to_video" {
        "minimax_h3_fl2va_pruned_int8_convrot.safetensors"
    } else {
        "minimax_h3_ref2va_pruned_int8_convrot.safetensors"
    };
    let diffusion = named(payload, "diffusion_model_name", default_model);
    let clip = named(payload, "clip_name", "qwen3vl_32b_minimax_h3_nvfp4_awq.safetensors");
    let video_vae = named(payload, "video_vae_name", "minimax_h3_video_vae_fp16.safetensors");
    let audio_vae = named(payload, "audio_vae_name", "minimax_h3_audio_vae_fp32.safetensors");
    let upscaler = named(payload, "latent_upscaler_name", "minimax_h3_latent_upscaler_3d_bf16.safetensors");
    require_model_choice(env, &["diffusion_models", "unet"], &diffusion, "MiniMax H3 two-pass diffusion model")?;
    require_model_choice(env, &["text_encoders", "clip"], &clip, "MiniMax H3 two-pass text encoder")?;
    require_model_choice(env, &["vae"], &video_vae, "MiniMax H3 two-pass video VAE")?;
    require_model_choice(env, &["vae"], &audio_vae, "MiniMax H3 two-pass audio VAE")?;
    require_model_choice(env, &["latent_upscale_models"], &upscaler, "MiniMax H3 learned latent upscaler")?;

    let seed = seed_value(env, payload, "seed", 69);
    let pass1_seed = seed_value(env, payload, "pass1_seed", -1);
    let pass2_seed = seed_value(env, payload, "pass2_seed", -1);
    let final_width = int_payload(payload, "final_width", 1920, 64, 16384);
    let final_height = int_payload(payload, "final_height", 1080, 64, 16384);
    let latent_scale = float_payload(payload, "latent_upscale_scale", 2.0, 1.0, 8.0);

    set_input(&mut prompt, "138", "value", json!(video_prompt))?;
    set_input(&mut prompt, "132", "value", json!(timing.workflow_duration_input_seconds))?;
    if let Some(prepared) = &ctx.prepared_audio {
        set_input(&mut prompt, "171", "audio_file", prepared.get("audio_path").cloned().unwrap_or(Value::Null))?;
        set_input(&mut prompt, "171", "seek_seconds", json!(0))?;
        set_input(&mut prompt, "171", "duration", json!(0))?;
    }
    let image_values: Vec<Value> = image_paths.iter().map(|p| json!(p)).collect();
    let video_values: Vec<Value> = video_refs.iter().map(|v| Value::Object(v.clone())).collect();
    set_input(&mut prompt, "180", "image_paths", json!(json_list_text(&image_values)))?;
    set_input(&mut prompt, "180", "video_references", json!(json_list_text(&video_values)))?;
    let ref_size = named(payload, "ref_image_size", "max").to_lowercase();
    if video_mode != "image_to_video" {
        set_input(&mut prompt, "136", "ref_image_size", json!(if ref_size == "match" || ref_size == "max" { ref_size } else { "max".to_string() }))?;
    }

    if video_mode == "image_to_video" || video_mode == "image_reference_to_video" {
        // A keyframe's latent must match the spatial grid used by its sampler. Use the actual upscaled dimensions,
        // including rounding, rather than the requested output size (the final decoded resize happens afterwards).
        for (node_id, source, label) in [("212", "186", "Width"), ("213", "187", "Height")] {
            prompt.insert(
                node_id.to_string(),
                json!({
                    "class_type": "ComfyMathExpression",
                    "inputs": {"expression": "round(a / 16 * b) * 16", "values.a": [source, 1], "values.b": ["185", 0]},
                    "_meta": {"title": format!("I2V Pass 2 - Actual Upscaled {label}")},
                }),
            );
        }
        let mut conditioning = prompt.get("136").cloned().unwrap_or(Value::Null);
        if let Some(map) = conditioning.get_mut("inputs").and_then(Value::as_object_mut) {
            map.insert("width".into(), link("212", 1));
            map.insert("height".into(), link("213", 1));
        }
        conditioning["_meta"] = json!({"title": "I2V Pass 2 - Upscaled Frame and Reference Conditioning"});
        prompt.insert("214".to_string(), conditioning);
        set_input(&mut prompt, "193", "conditioning", link("214", 0))?;
    }

    set_input(&mut prompt, "115", "value", json!(final_width))?;
    set_input(&mut prompt, "184", "value", json!(final_height))?;
    set_input(&mut prompt, "185", "value", json!(latent_scale))?;
    set_input(&mut prompt, "188", "model_name", json!(upscaler))?;
    set_input(&mut prompt, "188", "device", json!(named(payload, "latent_upscaler_device", "cuda")))?;
    set_input(&mut prompt, "188", "precision", json!(named(payload, "latent_upscaler_precision", "bf16")))?;

    set_input(&mut prompt, "129", "noise_seed", json!(pass1_seed))?;
    set_input(&mut prompt, "211", "noise_seed", json!(pass2_seed))?;
    set_input(&mut prompt, "123", "sampler_name", json!(text_or(payload, "pass1_sampler_name", "res_multistep")))?;
    set_input(&mut prompt, "210", "sampler_name", json!(text_or(payload, "pass2_sampler_name", "res_multistep")))?;
    set_input(&mut prompt, "124", "scheduler", json!(text_or(payload, "pass1_scheduler", "simple")))?;
    set_input(&mut prompt, "124", "steps", json!(int_payload(payload, "pass1_steps", 20, 1, 1000)))?;
    set_input(&mut prompt, "124", "denoise", json!(float_payload(payload, "pass1_denoise", 1.0, 0.0, 1.0)))?;
    set_input(&mut prompt, "192", "scheduler", json!(text_or(payload, "pass2_scheduler", "simple")))?;
    set_input(&mut prompt, "190", "value", json!(int_payload(payload, "pass2_steps", 2, 1, 1000)))?;
    set_input(&mut prompt, "191", "value", json!(float_payload(payload, "pass2_denoise", 0.2, 0.0, 1.0)))?;

    set_input(&mut prompt, "141", "model_name", json!(diffusion))?;
    set_input(&mut prompt, "141", "sage_attention", json!(named(payload, "sage_attention", "auto")))?;
    set_input(&mut prompt, "141", "enable_fp16_accumulation", json!(bool_payload(payload, "enable_fp16_accumulation", true)))?;
    set_input(&mut prompt, "128", "clip_name", json!(clip))?;
    set_input(&mut prompt, "119", "vae_name", json!(video_vae))?;
    set_input(&mut prompt, "120", "vae_name", json!(audio_vae))?;

    let default_lora = if video_mode == "image_to_video" {
        "minimax_h3_fl2v_turbo_4step_v1.1_768p_comfyui_bf16.safetensors"
    } else {
        "minimax_h3_ref2v_turbo_4step_v0.1_comfyui_bf16.safetensors"
    };
    let lora_value = payload.get("two_pass_lora_name").cloned().unwrap_or_else(|| json!(default_lora));
    let turbo_lora = clean_lora_name(env, Some(&lora_value));
    let base = turbo_lora.replace('\\', "/").rsplit('/').next().unwrap_or("").to_lowercase();
    if video_mode == "image_to_video" && base.contains("ref2v") {
        return Err(GraphError::new("Image to Video 2 Pass requires an FL2V/I2V Turbo LoRA, not a Reference-to-Video LoRA."));
    }
    if turbo_lora == NONE_LORA {
        return Err(GraphError::new("Select the MiniMax H3 two-pass Turbo LoRA; it is required by this fast workflow."));
    }
    require_model_choice(env, &["loras"], &turbo_lora, "MiniMax H3 two-pass Turbo LoRA")?;
    set_input(&mut prompt, "207", "lora_name", json!(turbo_lora))?;
    set_input(&mut prompt, "207", "strength_model", json!(float_payload(payload, "two_pass_lora_strength", 1.0, -10.0, 10.0)))?;

    let use_te_speed = bool_payload(payload, "two_pass_use_te_speed", false);
    set_input(&mut prompt, "207", "model", link("141", 0))?;
    prompt.shift_remove("208");
    let mut pass1_model = link("141", 0);
    let mut pass2_model = link("207", 0);

    // Extra LoRAs the user added, each on pass 1, pass 2 or both.
    struct Extra {
        name: String,
        strength: f64,
        apply_to: String,
    }
    let mut extra_loras: Vec<Extra> = Vec::new();
    if bool_payload(payload, "use_loras", false) || bool_payload(payload, "use_custom_loras", false) {
        let raw: Vec<Value> = payload.get("loras").and_then(Value::as_array).cloned().unwrap_or_default();
        let lora_count = int_payload(payload, "lora_count", raw.len() as i64, 0, 4) as usize;
        for item in raw.iter().take(lora_count) {
            let Some(map) = item.as_object() else { continue };
            let name = ["name", "lora_name", "loraName"].iter().filter_map(|k| map.get(*k)).find(|v| truthy(v));
            let name = clean_lora_name(env, name);
            if name.is_empty() || name == NONE_LORA {
                continue;
            }
            if !model_choice_exists(env, &["loras"], &name) {
                return Err(GraphError::new(format!(
                    "MiniMax extra LoRA '{name}' was not found in ComfyUI/models/loras. Download it, refresh/restart ComfyUI, and select it in MiniMax Video Settings."
                )));
            }
            // A LoRA with no target (or an unknown one) goes on the first pass.
            let pick = ["apply_to", "applyTo"].iter().filter_map(|k| map.get(*k)).find(|v| truthy(v));
            let mut apply_to = match pick {
                Some(v) => text(Some(v)).to_lowercase(),
                None => "pass1".to_string(),
            };
            if !matches!(apply_to.as_str(), "both" | "pass1" | "pass2") {
                apply_to = "pass1".to_string();
            }
            extra_loras.push(Extra { name, strength: float_payload(map, "strength", 1.0, -10.0, 10.0), apply_to });
        }
    }
    let mut next_lora_node_id: i64 = 9201;
    let mut applied_extra: Vec<Value> = Vec::new();
    let mut add_extra = |prompt: &mut Obj, model: &Value, item: &Extra, target: &str, index: usize| -> Value {
        while prompt.contains_key(&next_lora_node_id.to_string()) {
            next_lora_node_id += 1;
        }
        let id = next_lora_node_id.to_string();
        next_lora_node_id += 1;
        prompt.insert(
            id.clone(),
            json!({
                "class_type": "LoraLoaderModelOnly",
                "inputs": {"model": model, "lora_name": item.name, "strength_model": item.strength},
                "_meta": {"title": format!("Extra MiniMax LoRA {index} - {target}")},
            }),
        );
        applied_extra.push(json!({"name": item.name, "strength": item.strength, "apply_to": item.apply_to, "target": target, "node": id}));
        link(&id, 0)
    };
    for (i, item) in extra_loras.iter().enumerate() {
        let index = i + 1;
        if item.apply_to == "both" || item.apply_to == "pass1" {
            pass1_model = add_extra(&mut prompt, &pass1_model, item, "pass1", index);
        }
        if item.apply_to == "both" || item.apply_to == "pass2" {
            pass2_model = add_extra(&mut prompt, &pass2_model, item, "pass2", index);
        }
    }

    let use_ff = bool_payload(payload, "two_pass_use_feedforward", false);
    let use_bs = bool_payload(payload, "two_pass_use_block_sparse_attention", false);
    let mut model_refs: Vec<(String, Value)> = vec![("pass1".into(), pass1_model), ("pass2".into(), pass2_model)];
    let mut acceleration = Obj::new();
    for target in ["pass1", "pass2"] {
        acceleration.insert(
            target.to_string(),
            json!({
                "te_speed": bool_payload(payload, &format!("{target}_use_te_speed"), use_te_speed),
                "feedforward": bool_payload(payload, &format!("{target}_use_feedforward"), use_ff),
                "block_sparse_attention": bool_payload(payload, &format!("{target}_use_block_sparse_attention"), use_bs),
            }),
        );
    }
    let flag = |target: &str, key: &str| acceleration[target][key].as_bool().unwrap_or(false);
    let mut optional_patch_nodes: Vec<Value> = Vec::new();
    for (slot, target) in ["pass1", "pass2"].iter().enumerate() {
        if flag(target, "te_speed") {
            let node_id = if *target == "pass1" { "208" } else { "9211" };
            let patched = patch_te_speed(&mut prompt, &model_refs[slot].1.clone(), payload, node_id);
            model_refs[slot].1 = patched;
        }
        let mut branch = vec![(target.to_string(), model_refs[slot].1.clone())];
        optional_patch_nodes.extend(patch_optional_model_paths(env, &mut prompt, &mut branch, flag(target, "feedforward"), flag(target, "block_sparse_attention"))?);
        model_refs[slot].1 = branch.remove(0).1;
    }
    let (pass1_model, pass2_model) = (model_refs[0].1.clone(), model_refs[1].1.clone());
    set_models(&mut prompt, &["124", "126"], &pass1_model)?;
    set_models(&mut prompt, &["192", "193"], &pass2_model)?;

    let use_fast_decode = bool_payload(payload, "two_pass_use_fast_vae_decode", false);
    if use_fast_decode {
        patch_fast_decode(env, &mut prompt)?;
    }
    set_input(&mut prompt, "183", "upscale_method", json!(named(payload, "final_resize_method", "nvidia_rtx_vsr")))?;
    set_input(&mut prompt, "142", "crf", json!(int_payload(payload, "output_crf", 19, 0, 100)))?;
    let (output_folder, filename_prefix) = output_location(env, &ctx.project, ctx.scene, true);
    set_input(&mut prompt, "142", "filename_prefix", json!(format!("{filename_prefix}_stage2")))?;
    if ctx.audio_mode == "built_in_audio" {
        use_native_audio(&mut prompt)?;
    }
    let mut continuation = Value::Null;
    let mut save_latent = Value::Null;
    if !payload.get("_skip_latent_patches").is_some_and(truthy) {
        continuation = patch_latent_continuation(env, &mut prompt, payload)?;
        save_latent = patch_save_latent(env, &mut prompt, payload, Some(&timing))?;
    }
    if video_mode == "image_to_video" {
        patch_i2v_keyframe_timing(&mut prompt, &timing);
    }
    let refmod = apply_refmod_if_requested(env, &mut prompt, payload)?;
    let pass1_steps = inputs(&prompt, "124").and_then(|i| i.get("steps")).cloned().unwrap_or(Value::Null);
    let pass2_steps = inputs(&prompt, "190").and_then(|i| i.get("value")).cloned().unwrap_or(Value::Null);
    let any = |key: &str| ["pass1", "pass2"].iter().any(|t| flag(t, key));
    Ok(obj(json!({
        "workflow_path": workflow_path,
        "output_folder": output_folder,
        "prompt": prompt,
        "refmod": refmod,
        "latent_continuation_settings": continuation,
        "save_latent_settings": save_latent,
        "used_seed": seed,
        "audio_mode": ctx.audio_mode,
        "timing": timing,
        "prepared_audio": ctx.prepared_audio,
        "post_render_trim": {
            "start": timing.final_trim_start_seconds,
            "duration": timing.final_trim_duration_seconds,
            "frames": timing.final_frame_count,
        },
        "reference_inputs": {"image_count": image_paths.len(), "video_count": video_refs.len()},
        "two_pass": {
            "pass1_steps": pass1_steps,
            "pass2_steps": pass2_steps,
            "final_width": final_width,
            "final_height": final_height,
            "latent_upscale_scale": latent_scale,
            "te_speed_enabled": any("te_speed"),
            "pass_acceleration": acceleration,
            "extra_loras": applied_extra,
            "feedforward_enabled": any("feedforward"),
            "block_sparse_attention_enabled": any("block_sparse_attention"),
            "optional_patch_nodes": optional_patch_nodes,
            "fast_vae_decode_enabled": use_fast_decode,
            "fast_vae_decode_tile_batch_size": if use_fast_decode { json!(8) } else { Value::Null },
        },
    })))
}

// -- 2 Pass Advanced ---------------------------------------------------------------------------------------------------------------

/// Settings of the MMH3 nodes that are no longer user options. Values come from the tested audio workflow.
mod hidden {
    pub const TILE_SIZE_MODE: &str = "rows_cols";
    pub const ANCHOR_STRENGTH: f64 = 0.999;
    pub const OVERLAP_MODE: &str = "later";
    pub const OVERLAP_BLEND: &str = "linear";
    pub const UPSCALER_DEVICE: &str = "cuda";
    pub const UPSCALER_PRECISION: &str = "bf16";
}

/// Output order of the spatial tile plan node. The graph links by these names.
const PLAN_OUTPUT_NAMES: [&str; 12] = [
    "grid_rows", "grid_cols", "spatial_w_overlap", "spatial_h_overlap", "fade_width", "fade_height", "min_tile_size",
    "chunk_length", "temporal_overlap", "tile_width", "tile_height", "summary",
];

fn plan_ref(name: &str) -> Value {
    let index = PLAN_OUTPUT_NAMES.iter().position(|n| *n == name).unwrap_or(0);
    link("9309", index as i64)
}

/// Build the tiled and chunked 2 Pass Advanced graph. The 2 Pass builder supplies the model, reference, audio, timing,
/// LoRA and sampler branches. This builder replaces its whole frame upscale and refinement tail with MMH3 Ultimate
/// Upscale and gives each pass its own resolution selector.
pub fn build_advanced(env: &dyn GraphEnv, payload: &Obj) -> Result<Obj> {
    if mode_text(payload, &["video_mode", "mode"], "") == "image_to_video" {
        return Err(GraphError::new("Image to Video supports Single and 2 Pass only; 2 Pass Advanced is not supported."));
    }
    if text(payload.get("pipeline")).to_lowercase() == "refmod" {
        return Err(GraphError::new("The RefMod pipeline supports Single and 2 Pass only. Choose one of those passes."));
    }
    let required = ["MMH3UltimateUpscale", "VRGDG_MiniMaxH3UltimateUpscaleParams", "MMH3TemporalSplitParams", "MMH3SpatialSplitParams", "VRGDG_MiniMaxH3SpatialTilePlan"];
    let missing: Vec<&str> = required.iter().copied().filter(|c| env.node_info(c).is_none()).collect();
    if !missing.is_empty() {
        return Err(GraphError::new(format!(
            "MiniMax H3 2 Pass Advanced requires the latest Comfyui-MMH3-UltimateUpscale custom nodes. Missing: {}. Install or update the repository, then restart ComfyUI.",
            missing.join(", ")
        )));
    }
    let mut base = payload.clone();
    // The shared adapter validates and configures the same learned upscaler checkpoint, Turbo LoRA, references, source
    // audio and exact timing.
    for (key, value) in [
        ("final_width", json!(1920)),
        ("final_height", json!(1080)),
        ("latent_upscale_scale", json!(2.0)),
        ("pass2_steps", json!(1)),
        ("pass2_denoise", json!(0.2)),
        ("pass2_sampler_name", json!("sa_solver")),
        ("pass2_scheduler", json!("simple")),
    ] {
        base.entry(key.to_string()).or_insert(value);
    }
    base.insert("_skip_latent_patches".into(), json!(true));
    let mut result = build_two_pass(env, &base)?;
    let mut prompt = result.get("prompt").and_then(Value::as_object).cloned().unwrap_or_default();

    let aspect_ratio = check_aspect(payload, "Unsupported MiniMax H3 advanced two-pass aspect ratio: ")?;
    let pass1_mp = float_payload(payload, "advanced_pass1_megapixels", 0.4, 0.1, 16.0);
    let pass2_mp = float_payload(payload, "advanced_pass2_megapixels", 2.0, 0.1, 16.0);
    if pass2_mp < pass1_mp {
        return Err(GraphError::new("2 Pass Advanced Pass 2 resolution must be at least Pass 1 resolution."));
    }
    // Every tile, chunk and fade setting is derived from the Pass 2 size at run time by the tile plan node. The only
    // user choice is the VRAM preset. Retired presets (32gb, custom) map to 24gb.
    let preset_source = first_value(payload, &["advanced_vram_preset", "vram_preset"]).cloned().unwrap_or(json!(""));
    let vram_preset = normalize_vram_preset(Some(&text_or_empty(Some(&preset_source))))?;
    let (pass1_width, pass1_height) = frame_size(pass1_mp, &aspect_ratio);
    let (pass2_width, pass2_height) = frame_size(pass2_mp, &aspect_ratio);

    prompt.insert(
        "9300".into(),
        json!({
            "class_type": "ResolutionSelector",
            "inputs": {"aspect_ratio": aspect_ratio, "megapixels": pass1_mp, "multiple": 32},
            "_meta": {"title": "2 Pass Advanced - Pass 1 Resolution"},
        }),
    );
    prompt.insert(
        "9301".into(),
        json!({
            "class_type": "ResolutionSelector",
            "inputs": {"aspect_ratio": aspect_ratio, "megapixels": pass2_mp, "multiple": 32},
            "_meta": {"title": "2 Pass Advanced - Pass 2 Resolution"},
        }),
    );
    set_input(&mut prompt, "136", "width", link("9300", 0))?;
    set_input(&mut prompt, "136", "height", link("9300", 1))?;

    let mut pass2_conditioning = prompt.get("136").cloned().unwrap_or(Value::Null);
    if let Some(map) = pass2_conditioning.get_mut("inputs").and_then(Value::as_object_mut) {
        map.insert("width".into(), link("9301", 0));
        map.insert("height".into(), link("9301", 1));
        let pass2_prompt = first_value(payload, &["pass2_prompt", "minimax_h3_pass2_prompt"]).filter(|v| truthy(v));
        map.insert("prompt".into(), json!(text_or_empty(pass2_prompt)));
    }
    pass2_conditioning["_meta"] = json!({"title": "2 Pass Advanced - Final Resolution Conditioning"});
    prompt.insert("9302".into(), pass2_conditioning);

    prompt.insert(
        "9303".into(),
        json!({
            "class_type": "VRGDG_MiniMaxH3UltimateUpscaleParams",
            "inputs": {
                "model_name": named(payload, "latent_upscaler_name", "minimax_h3_latent_upscaler_3d_bf16.safetensors"),
                "width": link("9301", 0),
                "height": link("9301", 1),
                "device": hidden::UPSCALER_DEVICE,
                "precision": hidden::UPSCALER_PRECISION,
            },
            "_meta": {"title": "2 Pass Advanced - H3 Learned Latent Upscale"},
        }),
    );
    // Resolution driven tiling: the plan node reads the real Pass 2 size and outputs the grid, overlaps, fades, minimum
    // tile and temporal chunk settings for the VRAM preset.
    prompt.insert(
        "9309".into(),
        json!({
            "class_type": "VRGDG_MiniMaxH3SpatialTilePlan",
            "inputs": {"width": link("9301", 0), "height": link("9301", 1), "vram_preset": vram_preset},
            "_meta": {"title": "2 Pass Advanced - Spatial Tile Plan"},
        }),
    );
    prompt.insert(
        "9304".into(),
        json!({
            "class_type": "MMH3TemporalSplitParams",
            "inputs": {
                "chunk_length": plan_ref("chunk_length"),
                "temporal_overlap": plan_ref("temporal_overlap"),
                "anchor_strength": hidden::ANCHOR_STRENGTH,
            },
            "_meta": {"title": "2 Pass Advanced - Temporal Chunks"},
        }),
    );
    let spatial_required = obj(json!({
        "upscale_width": link("9301", 0),
        "upscale_height": link("9301", 1),
        "tile_size_mode": hidden::TILE_SIZE_MODE,
        // tile_width and tile_height are ignored in rows_cols mode. The plan node supplies the solved sizes.
        "tile_width": plan_ref("tile_width"),
        "tile_height": plan_ref("tile_height"),
        "grid_rows": plan_ref("grid_rows"),
        "grid_cols": plan_ref("grid_cols"),
        "spatial_w_overlap": plan_ref("spatial_w_overlap"),
        "spatial_h_overlap": plan_ref("spatial_h_overlap"),
        "fade_width": plan_ref("fade_width"),
        "fade_height": plan_ref("fade_height"),
        "min_tile_size": plan_ref("min_tile_size"),
        "overlap_mode": hidden::OVERLAP_MODE,
        "overlap_blend": hidden::OVERLAP_BLEND,
    }));
    let spatial_extras = obj(json!({
        "masked_area_noise": 0.0,
        "brightness_match": false,
        "dynamic_fade": "off",
        "dynamic_fade_min": 32,
    }));
    let spatial_inputs = compat_node_inputs(env, "MMH3SpatialSplitParams", spatial_required, &spatial_extras);
    prompt.insert(
        "9305".into(),
        json!({
            "class_type": "MMH3SpatialSplitParams",
            "inputs": spatial_inputs,
            "_meta": {"title": "2 Pass Advanced - Spatial Tiles"},
        }),
    );
    let pass2_model = inputs(&prompt, "192").and_then(|i| i.get("model")).cloned().unwrap_or(Value::Null);
    prompt.insert(
        "9306".into(),
        json!({
            "class_type": "MMH3UltimateUpscale",
            "inputs": {
                "model": pass2_model,
                "conditioning": ["9302", 0],
                "latent": ["125", 0],
                "noise": ["211", 0],
                "sampler": ["210", 0],
                "sigmas": ["192", 0],
                "cfg": 1.0,
                "latent_upscale_param": ["9303", 0],
                "temporal_split_param": ["9304", 0],
                "spatial_split_param": ["9305", 0],
            },
            "_meta": {"title": "2 Pass Advanced - MMH3 Ultimate Upscale"},
        }),
    );
    set_input(&mut prompt, "122", "samples", link("9306", 0))?;
    set_input(&mut prompt, "142", "images", link("122", 0))?;

    // Expose Pass 1 as a reviewable backup while Pass 2 remains the final clip.
    prompt.insert(
        "9307".into(),
        json!({
            "class_type": "VAEDecode",
            "inputs": {"samples": ["125", 0], "vae": ["119", 0]},
            "_meta": {"title": "2 Pass Advanced - Decode Pass 1 Preview"},
        }),
    );
    let mut pass1_output = prompt.get("142").cloned().unwrap_or(Value::Null);
    let prefix = inputs(&prompt, "142").and_then(|i| i.get("filename_prefix")).map(|v| text_or_empty(Some(v))).unwrap_or_default();
    if let Some(map) = pass1_output.get_mut("inputs").and_then(Value::as_object_mut) {
        map.insert("images".into(), link("9307", 0));
        map.insert("filename_prefix".into(), json!(prefix.replace("_stage2", "_stage1")));
    }
    pass1_output["_meta"] = json!({"title": "2 Pass Advanced - Pass 1 Backup"});
    prompt.insert("9308".into(), pass1_output);
    set_input(&mut prompt, "142", "filename_prefix", json!(prefix.replace("_stage2", "_advanced_stage2")))?;

    // Remove the original whole frame upscale and refinement tail now replaced by MMH3.
    for id in ["115", "181", "182", "183", "184", "185", "186", "187", "188", "189", "193", "194"] {
        prompt.shift_remove(id);
    }
    let continuation = patch_latent_continuation(env, &mut prompt, payload)?;
    let timing: Option<TimingPlan> = serde_json::from_value(result.get("timing").cloned().unwrap_or(Value::Null)).ok();
    let save_latent = patch_save_latent(env, &mut prompt, payload, timing.as_ref())?;

    result.insert("prompt".into(), Value::Object(prompt));
    result.insert("latent_continuation_settings".into(), continuation);
    result.insert("save_latent_settings".into(), save_latent);
    // The plan the plan node computes at run time, kept for progress text and the debug JSON.
    let plan = plan_spatial_tiles(pass2_width, pass2_height, &vram_preset)?;
    result.insert(
        "advanced_two_pass".into(),
        json!({
            "pass1_megapixels": pass1_mp,
            "pass2_megapixels": pass2_mp,
            "pass1_width": pass1_width,
            "pass1_height": pass1_height,
            "pass2_width": pass2_width,
            "pass2_height": pass2_height,
            "vram_preset": vram_preset,
            "tile_size_mode": hidden::TILE_SIZE_MODE,
            "tile_width": plan.tile_width,
            "tile_height": plan.tile_height,
            "grid_rows": plan.grid_rows,
            "grid_cols": plan.grid_cols,
            "chunk_length": plan.chunk_length,
            "temporal_overlap": plan.temporal_overlap,
        }),
    );
    result.shift_remove("two_pass");
    Ok(result)
}

/// Pick the builder for a workflow key (`minimax_h3`, `minimax_h3_2pass`, `minimax_h3_advanced_2pass`).
pub fn build(env: &dyn GraphEnv, workflow_key: &str, payload: &Obj) -> Result<Obj> {
    if workflow_key == "minimax_h3_advanced_2pass" && crate::refmod::refmod_requested(payload) {
        return Err(GraphError::new("The RefMod pipeline supports Single and 2 Pass only. Choose one of those passes."));
    }
    match workflow_key {
        "minimax_h3_advanced_2pass" => build_advanced(env, payload),
        "minimax_h3_2pass" => build_two_pass(env, payload),
        "minimax_h3" => build_single(env, payload),
        other => Err(GraphError::new(format!("Unknown MiniMax H3 workflow: {other}"))),
    }
}
