//! Graph patches: sampler and Sage settings, LoRAs, Turbo, optional model patches, latent save and masked continuation.
//! Port of `runner/minimax_patches.py` and the model helpers of `runner/models.py`.

use h3_core::pyval::{bool_payload, first_value, float_payload, int_payload, text_or_empty, to_float, to_int, truthy, Obj};
use h3_core::timing::TimingPlan;
use h3_core::tokens::{normalize_masked_context_frames, plan_masked_context};
use serde_json::{json, Value};

use crate::api::{
    as_link, class_of, find_final_vae_decode_id, inputs, link, node_id_by_class, optional_node_id_by_class, replace_input_refs,
    set_input,
};
use crate::env::{GraphEnv, GraphError, Result};
use crate::inputs::{abspath, clean_path_text};

pub const NONE_LORA: &str = "[none]";
const MASKED: &str = "latent_continuation_masked";

const SAGE_MODES: [&str; 8] = [
    "disabled",
    "auto",
    "sageattn_qk_int8_pv_fp16_cuda",
    "sageattn_qk_int8_pv_fp16_triton",
    "sageattn_qk_int8_pv_fp8_cuda",
    "sageattn_qk_int8_pv_fp8_cuda++",
    "sageattn3",
    "sageattn3_per_block_mean",
];

// -- models -----------------------------------------------------------------------------------------------------------

fn folder_choices(env: &dyn GraphEnv, categories: &[&str]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for category in categories {
        for choice in env.model_choices(category) {
            if !seen.contains(&choice) {
                seen.push(choice);
            }
        }
    }
    seen
}

fn basename(path: &str) -> String {
    path.replace('\\', "/").rsplit('/').next().unwrap_or("").to_string()
}

/// A model file name ComfyUI lists, matched by full name or by file name alone.
pub fn model_choice_exists(env: &dyn GraphEnv, categories: &[&str], value: &str) -> bool {
    let requested = value.trim();
    if requested.is_empty() {
        return false;
    }
    let requested_base = basename(requested);
    folder_choices(env, categories).iter().any(|choice| {
        let text = choice.trim();
        !text.is_empty() && (text == requested || basename(text) == requested_base)
    })
}

pub fn require_model_choice(env: &dyn GraphEnv, categories: &[&str], value: &str, label: &str) -> Result<()> {
    if model_choice_exists(env, categories, value) {
        return Ok(());
    }
    Err(GraphError::new(format!(
        "{label} '{value}' was not found in ComfyUI/models/{}. Install the model there, refresh/restart ComfyUI, then try again.",
        categories.first().copied().unwrap_or("")
    )))
}

/// A LoRA name ComfyUI lists, else `[none]`.
pub fn clean_lora_name(env: &dyn GraphEnv, value: Option<&Value>) -> String {
    let text = match value {
        Some(v) if truthy(v) => text_or_empty(Some(v)).trim().to_string(),
        _ => NONE_LORA.to_string(),
    };
    if text == NONE_LORA || env.model_choices("loras").contains(&text) { text } else { NONE_LORA.to_string() }
}

/// `str(payload.get(key) or default).strip()`, or the default when that is empty.
pub fn text_or(payload: &Obj, key: &str, default: &str) -> String {
    let text = match payload.get(key) {
        Some(v) if truthy(v) => text_or_empty(Some(v)).trim().to_string(),
        _ => default.to_string(),
    };
    if text.is_empty() { default.to_string() } else { text }
}

fn model_ref(prompt: &Obj, node_id: &str) -> Option<Value> {
    inputs(prompt, node_id).and_then(|i| i.get("model")).filter(|m| as_link(m).is_some()).cloned()
}

fn free_id(prompt: &Obj, start: i64) -> String {
    let mut id = start;
    while prompt.contains_key(&id.to_string()) {
        id += 1;
    }
    id.to_string()
}

// -- continuation mode and warm-up ------------------------------------------------------------------------------------

/// The continuity mode of a payload. Retired latent modes continue masked.
pub fn latent_continuation_mode(payload: &Obj) -> String {
    let pick = ["minimax_h3_continuity_mode", "continuity_mode", "continuityMode"].iter().filter_map(|k| payload.get(*k)).find(|v| truthy(v));
    let mode = text_or_empty(pick).trim().to_lowercase().replace(['-', ' '], "_");
    const RETIRED: [&str; 10] = [
        "latent", "latent_continuation", "continuation", "latent_exact", "latent_exact_frame", "latent_continuation_exact",
        "latent_continuation_exact_frame", "latent_masked", "latent_masked_av", "latent_continuation_masked",
    ];
    if RETIRED.contains(&mode.as_str()) { MASKED.to_string() } else { mode }
}

pub fn is_latent_mode(mode: &str) -> bool {
    mode == MASKED
}

fn scene_number(payload: &Obj) -> u32 {
    int_payload(payload, "scene_number", 1, 1, 999_999) as u32
}

/// Context size setting, snapped to a valid masked context size.
pub fn latent_context_frames_setting(payload: &Obj) -> i64 {
    let pick = ["minimax_h3_latent_context_frames", "latent_context_frames", "context_frames"]
        .iter()
        .filter_map(|k| payload.get(*k))
        .find(|v| truthy(v));
    let frames = pick.and_then(to_int).unwrap_or(22);
    normalize_masked_context_frames(frames as f64)
}

/// Frames at the end of this render that lie after the scene's visible last frame.
pub fn tail_padding_frames(timing: &TimingPlan) -> i64 {
    let visible = h3_core::pyval::round_half_even(timing.actual_warmup_seconds * 24.0) + timing.final_frame_count;
    (timing.h3_frame_count - visible).max(0)
}

/// Cool-down frames for the timing plan. Masked continuation plans its own tail, so the setting does not apply.
pub fn cooldown_frames(payload: &Obj) -> f64 {
    if is_latent_mode(&latent_continuation_mode(payload)) {
        return 0.0;
    }
    first_value(payload, &["cooldown_frames", "tail_loss_frames"]).and_then(to_float).unwrap_or(0.0)
}

/// The masked continuation window of the predecessor's saved latent, or `None` when it cannot be planned.
fn masked_latent_plan(env: &dyn GraphEnv, payload: &Obj) -> Option<h3_core::tokens::MaskedContext> {
    let project = clean_path_text(payload.get("project_folder"));
    let scene = scene_number(payload);
    if scene <= 1 || project.is_empty() || !env.is_dir(&project) {
        return None;
    }
    let info = env.latent_facts(&abspath(&project), scene - 1);
    if !info.exists || info.token_count <= 0 {
        return None;
    }
    plan_masked_context(info.token_count, info.tail_padding_frames, latent_context_frames_setting(payload) as f64).ok()
}

/// Warm-up frames for the timing plan. Masked continuation anchors the predecessor's trailing frames at the start of the
/// render, so those frames, and the audio that played over them, are a warm-up in front of the scene. The plan then
/// starts the audio that much earlier and trims the same span off the finished video and audio together.
pub fn effective_warmup_frames(env: &dyn GraphEnv, payload: &Obj) -> i64 {
    let requested = first_value(payload, &["warmup_frames", "pre_frames"]).filter(|v| truthy(v)).and_then(to_float).map_or(0, |f| (f as i64).max(0));
    if !is_latent_mode(&latent_continuation_mode(payload)) {
        return requested;
    }
    // Masked continuation sets its own warm-up, so the Render settings frames do not apply.
    if scene_number(payload) <= 1 {
        return 0;
    }
    match masked_latent_plan(env, payload) {
        Some(plan) => plan.warmup_frames.max(0),
        None => latent_context_frames_setting(payload),
    }
}

// -- latent save and continuation ---------------------------------------------------------------------------------------

/// Save every render's final latent (and a 2 Pass render's first pass latent) through the save node.
pub fn patch_save_latent(env: &dyn GraphEnv, prompt: &mut Obj, payload: &Obj, timing: Option<&TimingPlan>) -> Result<Value> {
    let project_text = clean_path_text(payload.get("project_folder"));
    if project_text.is_empty() || !env.is_dir(&project_text) {
        return Ok(json!({"enabled": false, "reason": "No valid project folder"}));
    }
    let project = abspath(&project_text);
    let scene = scene_number(payload);
    let decode_id = match find_final_vae_decode_id(prompt) {
        Ok(id) if prompt.contains_key(&id) => id,
        _ => return Ok(json!({"enabled": false, "reason": "VAEDecode node not found in prompt"})),
    };
    let samples = inputs(prompt, &decode_id).and_then(|i| i.get("samples")).cloned();
    let samples = match samples {
        Some(Value::Array(a)) if a.len() >= 2 => a,
        _ => return Ok(json!({"enabled": false, "reason": "VAEDecode samples input not found or invalid"})),
    };
    let upstream = match &samples[0] {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    if !prompt.contains_key(&upstream) {
        return Ok(json!({"enabled": false, "reason": format!("Upstream latent source node '{upstream}' not found in prompt")}));
    }
    let tail = timing.map(tail_padding_frames);
    let save_id = free_id(prompt, 9850);
    prompt.insert(
        save_id.clone(),
        json!({
            "class_type": "VRGDG_MiniMaxH3SaveLatent",
            "inputs": {
                "latent": samples,
                "project_folder": project,
                "scene_number": scene,
                "frame_count": 0,
                "fps": 24.0,
                "tail_padding_frames": tail.unwrap_or(-1),
            },
            "_meta": {"title": format!("Universal Latent Auto-Save (Scene {scene:03})")},
        }),
    );
    if let Some(decode_inputs) = prompt.get_mut(&decode_id).and_then(|n| n.get_mut("inputs")).and_then(Value::as_object_mut) {
        decode_inputs.insert("samples".into(), link(&save_id, 0));
    }

    // 2 Pass: also keep the first pass's latent, so the next scene's first pass continues it directly instead of a
    // shrunk copy of this scene's finished second pass (which would put the second pass's contrast in twice).
    let first_pass_source = prompt
        .values()
        .filter(|n| n.get("class_type").and_then(Value::as_str) == Some("VRGDG_MiniMaxH3ReplaceUpscaledVideoLatent"))
        .find_map(|n| n.get("inputs").and_then(|i| i.get("original_av_latent")).filter(|v| v.is_array()).cloned());
    let mut pass1_save_id: Option<String> = None;
    if let Some(source) = first_pass_source {
        let id = free_id(prompt, save_id.parse::<i64>().unwrap_or(9850) + 1);
        prompt.insert(
            id.clone(),
            json!({
                "class_type": "VRGDG_MiniMaxH3SaveLatent",
                "inputs": {
                    "latent": source.clone(),
                    "project_folder": project,
                    "scene_number": scene,
                    "frame_count": 0,
                    "fps": 24.0,
                    "tail_padding_frames": tail.unwrap_or(-1),
                    "pass1": true,
                },
                "_meta": {"title": format!("First-Pass Latent Save (Scene {scene:03})")},
            }),
        );
        // The second pass reads the first pass through the save node, so the first pass file is always written before
        // the final latent. The next scene only trusts a first pass file that is not newer than the final latent.
        let ids: Vec<String> = prompt.keys().cloned().collect();
        for node_id in ids {
            if node_id == id || class_of(prompt, &node_id) == "VRGDG_MiniMaxH3SaveLatent" {
                continue;
            }
            if let Some(map) = prompt.get_mut(&node_id).and_then(|n| n.get_mut("inputs")).and_then(Value::as_object_mut) {
                for value in map.values_mut() {
                    if *value == source {
                        *value = link(&id, 0);
                    }
                }
            }
        }
        pass1_save_id = Some(id);
    }
    Ok(json!({
        "enabled": true,
        "save_node_id": save_id,
        "pass1_save_node_id": pass1_save_id,
        "project_folder": project,
        "scene_number": scene,
    }))
}

/// Latent Continuation Masked: the predecessor's latent becomes the protected head of the sampled latent. A load node
/// slices a phase aligned window of the predecessor and an apply node copies it into the first tokens of the sampler's
/// latent with a zero denoise mask. 2 Pass applies the same window again to the second sampler. 2 Pass Advanced is not
/// supported, because its second pass runs inside the MMH3 Ultimate Upscale node, which builds its own masks.
fn patch_latent_continuation_masked(env: &dyn GraphEnv, prompt: &mut Obj, payload: &Obj) -> Result<Value> {
    if prompt.values().any(|n| n.get("class_type").and_then(Value::as_str) == Some("MMH3UltimateUpscale")) {
        return Err(GraphError::new(
            "Latent Continuation Masked is not available in 2 Pass Advanced. Use Single pass or 2 Pass, or set continuity to off.",
        ));
    }
    let project_text = clean_path_text(payload.get("project_folder"));
    if project_text.is_empty() || !env.is_dir(&project_text) {
        return Ok(json!({"enabled": false, "reason": format!("Project folder not found: {project_text}")}));
    }
    let project = abspath(&project_text);
    let scene = scene_number(payload);
    if scene <= 1 {
        return Ok(json!({
            "enabled": false,
            "scene_number": scene,
            "reason": "Scene 1 is the opening scene; no predecessor latent needed",
        }));
    }
    let pred = scene - 1;
    if !env.latent_exists(&project, pred) {
        return Err(GraphError::new(format!(
            "Latent Continuation Masked for Scene {scene:03} requires Scene {pred:03} latent, but '{}' was not found. Render Scene {pred:03} first.",
            env.latent_path(&project, pred)
        )));
    }

    let sampler_ids: Vec<String> =
        prompt.iter().filter(|(_, n)| n.get("class_type").and_then(Value::as_str) == Some("SamplerCustomAdvanced")).map(|(k, _)| k.clone()).collect();
    let replace_ids: Vec<String> = prompt
        .iter()
        .filter(|(_, n)| n.get("class_type").and_then(Value::as_str) == Some("VRGDG_MiniMaxH3ReplaceUpscaledVideoLatent"))
        .map(|(k, _)| k.clone())
        .collect();
    let reads_upscaled = |prompt: &Obj, id: &str| -> bool {
        inputs(prompt, id)
            .and_then(|i| i.get("latent_image"))
            .and_then(as_link)
            .is_some_and(|(source, _)| replace_ids.contains(&source))
    };
    let second: Vec<String> = sampler_ids.iter().filter(|id| reads_upscaled(prompt, id)).cloned().collect();
    let first: Vec<String> = sampler_ids.iter().filter(|id| !second.contains(id)).cloned().collect();
    if first.len() != 1 || second.len() > 1 {
        return Err(GraphError::new(format!(
            "Latent Continuation Masked supports Single pass and 2 Pass renders (found {} samplers it cannot map to those). Use Latent Continuation for this render.",
            sampler_ids.len()
        )));
    }
    let sampler_id = first[0].clone();
    let latent_source = match inputs(prompt, &sampler_id).and_then(|i| i.get("latent_image")) {
        Some(v) if as_link(v).is_some() => v.clone(),
        _ => return Ok(json!({"enabled": false, "reason": "Sampler latent input not found"})),
    };

    let context_frames = latent_context_frames_setting(payload);
    let info = env.latent_facts(&project, pred);
    let plan = plan_masked_context(info.token_count, info.tail_padding_frames, context_frames as f64)?;

    // With Audio Drive the song audio is already locked into the latent. Built-in audio keeps the predecessor's.
    let source_id = as_link(&latent_source).map(|(id, _)| id).unwrap_or_default();
    let include_audio = class_of(prompt, &source_id) != "VRGDG_MiniMaxH3AudioDrive";

    // The head is resized through pictures when the predecessor was saved at another size, which needs the video VAE.
    let conditioning: Vec<String> = prompt
        .iter()
        .filter(|(_, n)| matches!(n.get("class_type").and_then(Value::as_str), Some("MiniMaxH3ReferenceToVideo" | "MiniMaxH3ImageToVideo")))
        .map(|(k, _)| k.clone())
        .collect();
    let video_vae = conditioning
        .iter()
        .find_map(|id| inputs(prompt, id).and_then(|i| i.get("vae")).filter(|v| v.is_array()).cloned());
    // Image to Video pins the scene image as a keyframe at frame 0. The protected head already holds the opening
    // frames, so that keyframe would fight it. Later scenes open on the predecessor instead.
    let mut dropped_first_frame = false;
    for id in &conditioning {
        if class_of(prompt, id) != "MiniMaxH3ReferenceToVideo" {
            if let Some(map) = prompt.get_mut(id).and_then(|n| n.get_mut("inputs")).and_then(Value::as_object_mut) {
                if map.shift_remove("first_frame").is_some() {
                    dropped_first_frame = true;
                }
            }
        }
    }

    let load_id = free_id(prompt, 9210);
    let apply_id = free_id(prompt, load_id.parse::<i64>().unwrap_or(9210) + 1);
    let load_inputs = json!({
        "project_folder": project,
        "scene_number": pred,
        "context_frames": context_frames,
        "exact_frame_mode": false,
        "masked_av": true,
    });
    prompt.insert(
        load_id.clone(),
        json!({
            "class_type": "VRGDG_MiniMaxH3LoadLatent",
            "inputs": load_inputs.clone(),
            "_meta": {"title": format!("Predecessor Latent Context (Scene {pred:03} \u{b7} {context_frames} frames \u{b7} masked)")},
        }),
    );
    let apply_inputs = |latent: Value, context: Value, include_audio: bool| -> Value {
        let mut map = json!({"latent": latent, "context_latent": context, "include_audio": include_audio});
        if let Some(vae) = &video_vae {
            map["vae"] = vae.clone();
        }
        map
    };
    prompt.insert(
        apply_id.clone(),
        json!({
            "class_type": "VRGDG_MiniMaxH3ApplyMaskedContinuation",
            "inputs": apply_inputs(latent_source, link(&load_id, 0), include_audio),
            "_meta": {"title": format!("MiniMax H3 Latent Continuation Masked ({} frames)", plan.context_frames)},
        }),
    );
    set_input(prompt, &sampler_id, "latent_image", link(&apply_id, 0))?;

    // 2 Pass: the first pass continues the predecessor's own first pass latent when that render saved one. Its head then
    // needs no resize through pictures, and only the second pass adds its contrast, once.
    let mut pass1_load_id: Option<String> = None;
    if !second.is_empty() && env.pass1_latent_matches(&project, pred) {
        let id = free_id(prompt, load_id.parse::<i64>().unwrap_or(9210) + 2);
        let mut inputs_with_pass1 = load_inputs.clone();
        inputs_with_pass1["pass1"] = json!(true);
        prompt.insert(
            id.clone(),
            json!({
                "class_type": "VRGDG_MiniMaxH3LoadLatent",
                "inputs": inputs_with_pass1,
                "_meta": {"title": format!("Predecessor First-Pass Latent (Scene {pred:03} \u{b7} {context_frames} frames \u{b7} masked)")},
            }),
        );
        set_input(prompt, &apply_id, "context_latent", link(&id, 0))?;
        pass1_load_id = Some(id);
    }

    let mut second_apply_id: Option<String> = None;
    if let Some(second_sampler_id) = second.first() {
        let id = free_id(prompt, apply_id.parse::<i64>().unwrap_or(9211) + 1);
        let latent = inputs(prompt, second_sampler_id).and_then(|i| i.get("latent_image")).cloned().unwrap_or(Value::Null);
        // The audio is the locked source track in every 2 pass graph, its mask comes through the upscale.
        prompt.insert(
            id.clone(),
            json!({
                "class_type": "VRGDG_MiniMaxH3ApplyMaskedContinuation",
                "inputs": apply_inputs(latent, link(&load_id, 0), false),
                "_meta": {"title": format!("MiniMax H3 Latent Continuation Masked \u{b7} Pass 2 ({} frames)", plan.context_frames)},
            }),
        );
        set_input(prompt, second_sampler_id, "latent_image", link(&id, 0))?;
        second_apply_id = Some(id);
    }

    Ok(json!({
        "enabled": true,
        "mode": MASKED,
        "predecessor_scene": pred,
        "context_frames": plan.context_frames,
        "warmup_frames": effective_warmup_frames(env, payload),
        "load_node_id": load_id,
        "apply_node_id": apply_id,
        "second_pass_apply_node_id": second_apply_id,
        "sampler_node_id": sampler_id,
        "include_audio": include_audio,
        "lost_tail_frames": plan.lost_tail_frames,
        "pass1_load_node_id": pass1_load_id,
        "tail_padding_known": info.tail_padding_frames.is_some(),
        "dropped_first_frame": dropped_first_frame,
        "trim_node_id": Value::Null,
    }))
}

pub fn patch_latent_continuation(env: &dyn GraphEnv, prompt: &mut Obj, payload: &Obj) -> Result<Value> {
    if latent_continuation_mode(payload) != MASKED {
        return Ok(json!({"enabled": false, "reason": "Continuity mode is not latent_continuation_masked"}));
    }
    patch_latent_continuation_masked(env, prompt, payload)
}

// -- sampler, Sage, Turbo, LoRAs, optional model patches -----------------------------------------------------------------

/// Sampler, scheduler, steps, denoise, Sage attention and Easy Cache settings of the single pass graph.
pub fn patch_advanced_settings(prompt: &mut Obj, payload: &Obj) -> Result<Obj> {
    let sampler_id = node_id_by_class(prompt, "KSamplerSelect", Some("123"))?;
    let scheduler_id = node_id_by_class(prompt, "BasicScheduler", Some("124"))?;
    let loader_id = node_id_by_class(prompt, "DiffusionModelLoaderKJ", Some("141"))?;
    let easy_cache_id = optional_node_id_by_class(prompt, "EasyCache", "", &["174"]);

    let sampler_name = text_or(payload, "sampler_name", "res_multistep");
    let scheduler = text_or(payload, "scheduler", "simple");
    let steps = int_payload(payload, "steps", 20, 1, 1000);
    let denoise = float_payload(payload, "denoise", 1.0, 0.0, 1.0);
    let bypass = bool_payload(payload, "easy_cache_bypass", false);
    let reuse = float_payload(payload, "easy_cache_reuse_threshold", 0.3, 0.0, 1.0);
    let start = float_payload(payload, "easy_cache_start_percent", 0.2, 0.0, 1.0);
    let end = float_payload(payload, "easy_cache_end_percent", 0.9, 0.0, 1.0);
    let verbose = bool_payload(payload, "easy_cache_verbose", false);
    let mut sage = match payload.get("sage_attention") {
        Some(v) if truthy(v) => text_or_empty(Some(v)).trim().to_string(),
        _ => "auto".to_string(),
    };
    if !SAGE_MODES.contains(&sage.as_str()) {
        sage = "auto".to_string();
    }
    let fp16 = bool_payload(payload, "enable_fp16_accumulation", true);

    set_input(prompt, &sampler_id, "sampler_name", json!(sampler_name))?;
    set_input(prompt, &scheduler_id, "scheduler", json!(scheduler))?;
    set_input(prompt, &scheduler_id, "steps", json!(steps))?;
    set_input(prompt, &scheduler_id, "denoise", json!(denoise))?;
    set_input(prompt, &loader_id, "sage_attention", json!(sage))?;
    set_input(prompt, &loader_id, "enable_fp16_accumulation", json!(fp16))?;
    if let Some(id) = easy_cache_id {
        set_input(prompt, &id, "reuse_threshold", json!(reuse))?;
        set_input(prompt, &id, "start_percent", json!(start))?;
        set_input(prompt, &id, "end_percent", json!(end))?;
        set_input(prompt, &id, "verbose", json!(verbose))?;
        if bypass {
            replace_input_refs(prompt, (&id, 0), (&loader_id, 0));
            prompt.shift_remove(&id);
        }
    }
    let summary = json!({
        "sampler_name": sampler_name,
        "scheduler": scheduler,
        "steps": steps,
        "denoise": denoise,
        "easy_cache_bypass": bypass,
        "easy_cache_reuse_threshold": reuse,
        "easy_cache_start_percent": start,
        "easy_cache_end_percent": end,
        "easy_cache_verbose": verbose,
        "sage_attention": sage,
        "enable_fp16_accumulation": fp16,
    });
    Ok(summary.as_object().cloned().unwrap_or_default())
}

/// The legacy Turbo LoRA through ComfyUI's native loader. It is only a LoRA toggle and needs no extra sampler.
pub fn patch_turbo(env: &dyn GraphEnv, prompt: &mut Obj, payload: &Obj) -> Result<Value> {
    if !bool_payload(payload, "use_turbo_lora", false) {
        return Ok(json!({"enabled": false, "lora_name": "", "strength": 0.0, "scheduler": "", "steps": 0}));
    }
    let lora_name = text_or(payload, "turbo_lora_name", "minimax_h3_turbo_4step_ema_ckpt850.safetensors");
    if lora_name.is_empty() {
        return Err(GraphError::new("MiniMax-H3 Turbo is enabled, but no Turbo LoRA file is selected."));
    }
    if !model_choice_exists(env, &["loras"], &lora_name) {
        return Err(GraphError::new(format!(
            "MiniMax-H3 Turbo LoRA '{lora_name}' was not found in ComfyUI/models/loras. Download the LoRA, refresh/restart ComfyUI, and select it in MiniMax Video Settings."
        )));
    }
    let strength = float_payload(payload, "turbo_lora_strength", 1.0, -10.0, 10.0);
    let scheduler_id = node_id_by_class(prompt, "BasicScheduler", Some("124"))?;
    let guider_id = node_id_by_class(prompt, "BasicGuider", Some("126"))?;
    let Some(model) = model_ref(prompt, &scheduler_id) else {
        return Err(GraphError::new("MiniMax-H3 Turbo could not find the current model connection feeding BasicScheduler."));
    };
    let id = free_id(prompt, 9001);
    prompt.insert(
        id.clone(),
        json!({
            "class_type": "LoraLoaderModelOnly",
            "inputs": {"model": model, "lora_name": lora_name, "strength_model": strength},
        }),
    );
    set_input(prompt, &scheduler_id, "model", link(&id, 0))?;
    set_input(prompt, &guider_id, "model", link(&id, 0))?;
    Ok(json!({
        "enabled": true,
        "lora_name": lora_name,
        "strength": strength,
        "scheduler": "",
        "steps": 0,
        "lora_node": "LoraLoaderModelOnly",
        "sampler_node": "",
    }))
}

/// Route the model through the memory efficient Sage Attention patch node.
pub fn patch_memory_efficient_sage_attention(prompt: &mut Obj, payload: &Obj) -> Result<Value> {
    if !bool_payload(payload, "use_memory_efficient_sage_attention", false) {
        return Ok(json!({"enabled": false, "node": ""}));
    }
    let scheduler_id = node_id_by_class(prompt, "BasicScheduler", Some("124"))?;
    let guider_id = node_id_by_class(prompt, "BasicGuider", Some("126"))?;
    let Some(model) = model_ref(prompt, &scheduler_id) else {
        return Err(GraphError::new("MiniMax H3 memory-efficient Sage Attention patch could not find the current model connection."));
    };
    let id = free_id(prompt, 9201);
    prompt.insert(
        id.clone(),
        json!({
            "class_type": "MiniMaxH3MemoryEfficientSageAttentionPatch",
            "inputs": {"model": model},
            "_meta": {"title": "MiniMax H3 Mem Eff Sage Attention Patch"},
        }),
    );
    set_input(prompt, &scheduler_id, "model", link(&id, 0))?;
    set_input(prompt, &guider_id, "model", link(&id, 0))?;
    Ok(json!({"enabled": true, "node": id}))
}

/// Chain the configured LoRAs onto the model the scheduler and guider read.
pub fn patch_loras(env: &dyn GraphEnv, prompt: &mut Obj, payload: &Obj) -> Result<Value> {
    let disabled = json!({"enabled": false, "count": 0, "loras": []});
    if !(bool_payload(payload, "use_loras", false) || bool_payload(payload, "use_custom_loras", false)) {
        return Ok(disabled);
    }
    if bool_payload(payload, "use_turbo_lora", false) {
        return Err(GraphError::new("MiniMax normal LoRAs and MiniMax-H3 Turbo LoRA cannot be enabled at the same time."));
    }
    let mut configured: Vec<(String, f64)> = Vec::new();
    if let Some(Value::Array(items)) = payload.get("loras") {
        for item in items {
            let Some(map) = item.as_object() else { continue };
            let name = ["name", "lora_name", "loraName"].iter().filter_map(|k| map.get(*k)).find(|v| truthy(v));
            configured.push((clean_lora_name(env, name), float_payload(map, "strength", 1.0, -10.0, 10.0)));
        }
    }
    let lora_count = int_payload(payload, "lora_count", configured.len() as i64, 0, 4) as usize;
    if configured.is_empty() {
        for slot in 1..=lora_count {
            let name = payload.get(&format!("lora_{slot}"));
            configured.push((
                clean_lora_name(env, name.or(Some(&json!(NONE_LORA)))),
                float_payload(payload, &format!("lora_{slot}_strength"), 1.0, -10.0, 10.0),
            ));
        }
    }
    let configured: Vec<(String, f64)> =
        configured.into_iter().take(lora_count).filter(|(name, _)| !name.is_empty() && name != NONE_LORA).collect();
    if configured.is_empty() {
        return Ok(disabled);
    }
    for (name, _) in &configured {
        if !model_choice_exists(env, &["loras"], name) {
            return Err(GraphError::new(format!(
                "MiniMax LoRA '{name}' was not found in ComfyUI/models/loras. Download the LoRA, refresh/restart ComfyUI, and select it in MiniMax Video Settings."
            )));
        }
    }
    let scheduler_id = node_id_by_class(prompt, "BasicScheduler", Some("124"))?;
    let guider_id = node_id_by_class(prompt, "BasicGuider", Some("126"))?;
    let Some(start) = model_ref(prompt, &scheduler_id) else {
        return Err(GraphError::new("MiniMax LoRA patch could not find the current model connection feeding BasicScheduler."));
    };
    let mut next_id = 9101;
    let mut current = start;
    let mut applied = Vec::new();
    for (index, (name, strength)) in configured.iter().enumerate() {
        let id = free_id(prompt, next_id);
        next_id = id.parse::<i64>().unwrap_or(next_id) + 1;
        prompt.insert(
            id.clone(),
            json!({
                "class_type": "LoraLoaderModelOnly",
                "inputs": {"model": current.clone(), "lora_name": name, "strength_model": strength},
                "_meta": {"title": format!("MiniMax LoRA {}", index + 1)},
            }),
        );
        current = link(&id, 0);
        applied.push(json!({"name": name, "strength": strength, "node": id}));
    }
    set_input(prompt, &scheduler_id, "model", current.clone())?;
    set_input(prompt, &guider_id, "model", current)?;
    Ok(json!({"enabled": true, "count": applied.len(), "loras": applied}))
}

/// Chunked feed forward and block sparse attention patches, applied per model branch. `branches` maps a target name to
/// the model link it starts from and receives the patched link.
pub fn patch_optional_model_paths(
    env: &dyn GraphEnv,
    prompt: &mut Obj,
    branches: &mut [(String, Value)],
    use_feedforward: bool,
    use_block_sparse_attention: bool,
) -> Result<Vec<Value>> {
    let mut sparse_selection = String::new();
    if use_feedforward || use_block_sparse_attention {
        let mut required: Vec<&str> = Vec::new();
        if use_feedforward {
            required.push("MiniMaxChunkFeedForward");
        }
        if use_block_sparse_attention {
            required.push("BlockSparseAttention");
        }
        let missing: Vec<&str> = required.iter().copied().filter(|c| env.node_info(c).is_none()).collect();
        if !missing.is_empty() {
            return Err(GraphError::new(format!(
                "MiniMax H3 optional model patches are enabled, but these nodes are missing: {}. Install/update ComfyUI-KJNodes and ComfyUI as needed, then restart ComfyUI.",
                missing.join(", ")
            )));
        }
    }
    if use_block_sparse_attention {
        let info = env.node_info("BlockSparseAttention").unwrap_or(Value::Null);
        let spec = ["required", "optional"]
            .iter()
            .filter_map(|s| info.get("input").and_then(|i| i.get(*s)).and_then(|m| m.get("selection")))
            .next();
        let keys: Vec<String> = spec
            .and_then(|s| s.get(1))
            .and_then(|o| o.get("options"))
            .and_then(Value::as_array)
            .map(|opts| opts.iter().filter_map(|o| o.get("key").and_then(Value::as_str).map(str::to_string)).collect())
            .unwrap_or_default();
        // DynamicCombo keys changed when Block Sparse Attention became Model Sparse Attention.
        match ["sol-attn", "Sol-Attn (adaptive tau)"].iter().find(|k| keys.iter().any(|x| x == *k)) {
            Some(key) => sparse_selection = key.to_string(),
            None => {
                return Err(GraphError::new(
                    "The installed BlockSparseAttention node does not advertise a supported Sol-Attn method. Disable Use Block Sparse Attention or update the VRGDG nodes for this ComfyUI version.",
                ))
            }
        }
    }
    let mut patch_nodes = Vec::new();
    let mut next_id = 9202;
    for (target, model) in branches.iter_mut() {
        let mut add = |prompt: &mut Obj, from: &Value, class: &str, extra: Value, title: &str| -> Value {
            let id = free_id(prompt, next_id);
            next_id = id.parse::<i64>().unwrap_or(next_id) + 1;
            let mut node_inputs = json!({"model": from});
            if let (Some(map), Some(more)) = (node_inputs.as_object_mut(), extra.as_object()) {
                for (k, v) in more {
                    map.insert(k.clone(), v.clone());
                }
            }
            prompt.insert(id.clone(), json!({"class_type": class, "inputs": node_inputs, "_meta": {"title": format!("{title} - {target}")}}));
            patch_nodes.push(json!({"node": id, "class_type": class, "target": target}));
            link(&id, 0)
        };
        if use_feedforward {
            *model = add(prompt, model, "MiniMaxChunkFeedForward", json!({"chunks": 8, "seq_threshold": 4096}), "MiniMax H3 Chunk FeedForward");
        }
        if use_block_sparse_attention {
            *model = add(
                prompt,
                model,
                "BlockSparseAttention",
                json!({
                    "selection": sparse_selection,
                    "selection.tau": 1.3,
                    "start_percent": 0.2,
                    "end_percent": 1.0,
                    "dense_blocks": "",
                    "min_tokens": 12288,
                    "extra_tokens": 256,
                    "sink_conditioning": "exact_kv_and_rows",
                    "verbose": false,
                }),
                "Block Sparse Attention",
            );
        }
    }
    Ok(patch_nodes)
}

/// TE-Speed caching on a model branch. Returns the link to the patched model.
pub fn patch_te_speed(prompt: &mut Obj, model: &Value, payload: &Obj, node_id: &str) -> Value {
    prompt.insert(
        node_id.to_string(),
        json!({
            "class_type": "TESpeedMiniMaxH3",
            "inputs": {
                "model": model,
                "processing_control_value": float_payload(payload, "te_speed_processing_control", 0.07, 0.0, 1.0),
                "processing_percent_1": float_payload(payload, "te_speed_start_percent", 0.1, 0.0, 1.0),
                "processing_percent_2": float_payload(payload, "te_speed_end_percent", 0.9, 0.0, 1.0),
                "mcs": int_payload(payload, "te_speed_mcs", 2, 1, 64),
                "cache_depth": float_payload(payload, "te_speed_cache_depth", 0.75, 0.0, 1.0),
                "device": match payload.get("te_speed_device") { Some(v) if truthy(v) => text_or_empty(Some(v)), _ => "auto".to_string() },
            },
            "_meta": {"title": "TE-Speed MiniMax H3"},
        }),
    );
    link(node_id, 0)
}

/// Swap the final decode for the tiled fast decoder.
pub fn patch_fast_decode(env: &dyn GraphEnv, prompt: &mut Obj) -> Result<()> {
    if env.node_info("H3FastVAEDecode").is_none() {
        return Err(GraphError::new("Fast VAE decode requires H3FastVAEDecode. Update VRGameDevGirl nodes and restart ComfyUI."));
    }
    let decoder = inputs(prompt, "122").cloned().unwrap_or_default();
    prompt.insert(
        "122".to_string(),
        json!({
            "class_type": "H3FastVAEDecode",
            "inputs": {
                "samples": decoder.get("samples").cloned().unwrap_or(Value::Null),
                "vae": decoder.get("vae").cloned().unwrap_or(Value::Null),
                "tile_batch_size": 8,
            },
            "_meta": {"title": "H3 VAE Decode Fast - Final Output"},
        }),
    );
    Ok(())
}
