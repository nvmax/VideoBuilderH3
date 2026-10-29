//! MiniMax H3 settings: normalize saved values, validate API patches, per-scene override and the render payload.
//! Port of `minimax/settings_payload.py`. Settings stay loose JSON (`Obj`), like the session file, so unknown keys a
//! newer pack version adds are never lost.

use std::sync::OnceLock;

use serde_json::{json, Value};

use crate::continuity::{canonical_continuity_mode, continuity_allowed_for_mode, MASKED};
use crate::error::CoreError;
use crate::pyval::{text_or_empty, to_float, truthy, Obj};
use crate::resolution::{migrate_resolution, output_frame_size, RESOLUTION_PRESETS};
use crate::tiles::normalize_vram_preset;

const DEFAULTS_JSON: &str = include_str!("../data/h3_settings_defaults.json");

const LORA_APPLY_TO: [&str; 3] = ["both", "pass1", "pass2"];
const REFERENCE_MODES: [&str; 2] = ["reference_to_video", "image_reference_to_video"];
const DECIMAL_KEY_TOKENS: [&str; 8] = ["megapixels", "denoise", "strength", "scale", "percent", "threshold", "depth", "noise"];
const SAGE_ATTENTION: [&str; 8] = [
    "disabled",
    "auto",
    "sageattn_qk_int8_pv_fp16_cuda",
    "sageattn_qk_int8_pv_fp16_triton",
    "sageattn_qk_int8_pv_fp8_cuda",
    "sageattn_qk_int8_pv_fp8_cuda++",
    "sageattn3",
    "sageattn3_per_block_mean",
];

pub const WORKFLOW_SINGLE: &str = "minimax_h3";
pub const WORKFLOW_TWO_PASS: &str = "minimax_h3_2pass";
pub const WORKFLOW_ADVANCED: &str = "minimax_h3_advanced_2pass";

/// Settings that no longer exist. Saved values are ignored and patches get a pointer to the new key.
fn retired_reason(key: &str) -> Option<&'static str> {
    match key {
        "two_pass_final_width" | "two_pass_final_height" => {
            Some("use resolution_preset / megapixels (one output resolution for every pass type)")
        }
        "advanced_two_pass_pass2_megapixels" => {
            Some("use resolution_preset / megapixels (the Pass 2 size is the output resolution)")
        }
        "advanced_two_pass_pass2_resolution_preset" => {
            Some("use resolution_preset (the Pass 2 size is the output resolution)")
        }
        _ => {
            let derived = [
                "tile_size_mode", "tile_width", "tile_height", "grid_rows", "grid_cols", "chunk_length",
                "temporal_overlap", "anchor_strength", "spatial_w_overlap", "spatial_h_overlap", "fade_width",
                "fade_height", "min_tile_size", "overlap_mode", "overlap_blend", "brightness_match", "dynamic_fade",
                "dynamic_fade_min", "masked_area_noise", "upscaler_device", "upscaler_precision",
            ];
            let name = key.strip_prefix("advanced_two_pass_")?;
            derived.contains(&name).then_some("derived from the output resolution and advanced_two_pass_vram_preset")
        }
    }
}

/// Saved by the UI but absent from the defaults: per-pass acceleration toggles and the per-pass profile cache.
fn optional_default(key: &str) -> Option<Value> {
    if key == "ref_pass_profiles" {
        return Some(json!({}));
    }
    let rest = key.strip_prefix("pass1_use_").or_else(|| key.strip_prefix("pass2_use_"))?;
    ["te_speed", "feedforward", "block_sparse_attention"].contains(&rest).then_some(Value::Bool(false))
}

const OPTIONAL_KEYS: [&str; 7] = [
    "pass1_use_te_speed",
    "pass1_use_feedforward",
    "pass1_use_block_sparse_attention",
    "pass2_use_te_speed",
    "pass2_use_feedforward",
    "pass2_use_block_sparse_attention",
    "ref_pass_profiles",
];

/// A fresh copy of the UI defaults, every setting the Video Builder panel saves.
pub fn defaults() -> Obj {
    static CACHE: OnceLock<Obj> = OnceLock::new();
    CACHE
        .get_or_init(|| match serde_json::from_str::<Value>(DEFAULTS_JSON) {
            Ok(Value::Object(map)) => map,
            _ => Obj::new(),
        })
        .clone()
}

fn enum_values(key: &str) -> Option<Vec<String>> {
    let list = |items: &[&str]| Some(items.iter().map(|s| s.to_string()).collect());
    match key {
        "pipeline" => list(&["standard", "refmod"]),
        "video_mode" => list(&[
            "text_to_video", "image_to_video", "image_reference_to_video", "reference_to_video", "video_to_video",
        ]),
        "render_pass" => list(&["single", "two_pass", "three_pass"]),
        "audio_mode" => list(&["input_audio", "built_in_audio"]),
        "i2v_transition_style" => list(&[
            "natural", "surreal_morph", "dreamlike_dissolve", "environment_transformation", "camera_reveal", "custom",
        ]),
        "ref_image_size" => list(&["max", "match"]),
        "resolution_preset" | "advanced_two_pass_pass1_resolution_preset" => list(&RESOLUTION_PRESETS),
        "advanced_two_pass_vram_preset" => list(&["8gb", "12gb", "16gb", "24gb"]),
        "continuity_mode" => list(&["off", MASKED]),
        "sage_attention" => list(&SAGE_ATTENTION),
        "location_transition_preset" => list(&[
            "normal", "surreal", "cinematic", "inner_world", "match", "motion", "creative_auto", "masked", "custom",
        ]),
        _ => None,
    }
}

/// Every setting with its type, default and limits, for the settings panel and `GET /settings/minimax-h3/schema`.
pub fn schema() -> Vec<Value> {
    let mut all = defaults();
    for key in OPTIONAL_KEYS {
        if let Some(default) = optional_default(key) {
            all.insert(key.to_string(), default);
        }
    }
    all.iter()
        .map(|(key, default)| {
            let kind = match default {
                Value::Bool(_) => "boolean",
                Value::Number(n) if n.is_f64() && !(n.as_f64().unwrap_or(0.0).fract() == 0.0 && !is_decimal_setting(key, default)) => "number",
                Value::Number(_) => "integer",
                Value::String(_) => "string",
                Value::Array(_) => "array",
                Value::Object(_) => "object",
                Value::Null => "null",
            };
            let mut entry = serde_json::json!({"key": key, "type": kind, "default": default});
            if let Some(choices) = enum_values(key) {
                entry["choices"] = serde_json::json!(choices);
            }
            if let Some((low, high)) = numeric_range(key) {
                entry["minimum"] = serde_json::json!(low);
                entry["maximum"] = serde_json::json!(high);
            }
            entry
        })
        .collect()
}

fn numeric_range(key: &str) -> Option<(f64, f64)> {
    if key == "megapixels" || key.ends_with("_megapixels") {
        return Some((0.1, 16.0));
    }
    if key == "steps" || key.ends_with("_steps") || key == "steps_before_turbo" {
        return Some((1.0, 1000.0));
    }
    if key == "denoise" || key.ends_with("_denoise") {
        return Some((0.0, 1.0));
    }
    None
}

fn is_number(value: &Value) -> bool {
    value.is_number()
}

fn is_decimal_setting(key: &str, default: &Value) -> bool {
    match default {
        Value::Bool(_) => false,
        Value::Number(n) => !n.is_i64() && !n.is_u64() || DECIMAL_KEY_TOKENS.iter().any(|t| key.contains(t)),
        _ => false,
    }
}

/// Text from older saves becomes the number or boolean it stands for (`"0.5"` becomes 0.5).
fn lenient(value: &Value, default: &Value) -> Value {
    let Value::String(text) = value else { return value.clone() };
    let text = text.trim();
    match default {
        Value::Bool(_) => match text.to_lowercase().as_str() {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            _ => value.clone(),
        },
        Value::Number(d) => match text.parse::<f64>() {
            Ok(number) if number.is_finite() => {
                if number.fract() == 0.0 && (d.is_i64() || d.is_u64()) {
                    Value::from(number as i64)
                } else {
                    Value::from(number)
                }
            }
            _ => value.clone(),
        },
        _ => value.clone(),
    }
}

fn err(message: &str) -> CoreError {
    CoreError::value(message)
}

/// Coerce `value` to the type of `default`, or fail with a short reason.
fn coerce(key: &str, value: &Value, default: &Value) -> Result<Value, CoreError> {
    if is_decimal_setting(key, default) {
        return match value {
            Value::Number(n) => Ok(Value::from(n.as_f64().unwrap_or(0.0))),
            _ => Err(err("must be a number")),
        };
    }
    match default {
        Value::Bool(_) => match value {
            Value::Bool(_) => Ok(value.clone()),
            Value::Number(n) if (n.is_i64() || n.is_u64()) && matches!(n.as_i64(), Some(0 | 1)) => {
                Ok(Value::Bool(n.as_i64() == Some(1)))
            }
            _ => Err(err("must be true or false")),
        },
        Value::Number(d) if d.is_i64() || d.is_u64() => match value {
            Value::Number(n) => {
                if n.is_f64() && n.as_f64().is_some_and(|f| f.fract() != 0.0) {
                    return Err(err("must be a whole number"));
                }
                Ok(n.as_i64().map(Value::from).unwrap_or_else(|| Value::from(n.as_f64().unwrap_or(0.0) as i64)))
            }
            _ => Err(err("must be a whole number")),
        },
        Value::Number(_) => match value {
            Value::Number(n) => Ok(Value::from(n.as_f64().unwrap_or(0.0))),
            _ => Err(err("must be a number")),
        },
        Value::String(_) => matches!(value, Value::String(_)).then(|| value.clone()).ok_or_else(|| err("must be text")),
        Value::Array(_) => matches!(value, Value::Array(_)).then(|| value.clone()).ok_or_else(|| err("must be a list")),
        Value::Object(_) => {
            matches!(value, Value::Object(_)).then(|| value.clone()).ok_or_else(|| err("must be an object"))
        }
        Value::Null => Ok(value.clone()),
    }
}

fn check_loras(loras: &[Value]) -> Result<(), CoreError> {
    if loras.len() > 4 {
        return Err(err("supports at most 4 LoRAs"));
    }
    for (index, item) in loras.iter().enumerate() {
        let n = index + 1;
        let name = item.as_object().map(|o| {
            let pick = o.get("name").filter(|v| truthy(v)).or_else(|| o.get("lora_name"));
            text_or_empty(pick).trim().to_string()
        });
        let Some(name) = name.filter(|n| !n.is_empty()) else {
            return Err(CoreError::value(format!("item {n} needs a 'name'")));
        };
        let _ = name;
        let apply_to = match item.get("apply_to") {
            Some(v) if truthy(v) => text_or_empty(Some(v)).trim().to_lowercase(),
            _ => "pass1".to_string(),
        };
        if !LORA_APPLY_TO.contains(&apply_to.as_str()) {
            return Err(CoreError::value(format!("item {n} apply_to must be one of: both, pass1, pass2")));
        }
    }
    Ok(())
}

/// Coerce and range/enum-check one setting. `lenient_text` accepts numbers and booleans saved as text, for reading saved
/// sessions. API patches stay strict so an agent gets a clear error instead of a silent conversion.
fn check_value(key: &str, value: &Value, default: &Value, lenient_text: bool) -> Result<Value, CoreError> {
    let source = if lenient_text { lenient(value, default) } else { value.clone() };
    let mut coerced = coerce(key, &source, default)?;
    if key == "i2v_transition_direction" {
        if let Value::String(s) = &coerced {
            coerced = Value::String(s.trim().to_string());
        }
    }
    if let Some(allowed) = enum_values(key) {
        let mut text = text_or_empty(Some(&coerced)).trim().to_lowercase();
        if key == "continuity_mode" {
            if let Some(canonical) = canonical_continuity_mode(Some(&Value::String(text.clone()))) {
                text = canonical.to_string();
            }
        }
        if !allowed.contains(&text) {
            return Err(CoreError::value(format!("must be one of: {}", allowed.join(", "))));
        }
        return Ok(Value::String(text));
    }
    if key == "latent_context_frames" {
        let choices = [16i64, 22, 39, 56, 90, 141, 192];
        let ok = coerced.as_i64().is_some_and(|v| choices.contains(&v));
        if !ok {
            let names: Vec<String> = choices.iter().map(i64::to_string).collect();
            return Err(CoreError::value(format!("must be one of: {}", names.join(", "))));
        }
    }
    if let Some((low, high)) = numeric_range(key) {
        if is_number(&coerced) {
            let f = coerced.as_f64().unwrap_or(0.0);
            if !(low <= f && f <= high) {
                return Err(CoreError::value(format!("must be between {low} and {high}")));
            }
        }
    }
    if key == "loras" {
        if let Value::Array(items) = &coerced {
            check_loras(items)?;
        }
    }
    Ok(coerced)
}

/// `{setting: problem}` for every unknown key or invalid value in `patch`. Empty means the patch is valid.
pub fn validate_patch(patch: &Obj) -> Obj {
    let mut known = defaults();
    for key in OPTIONAL_KEYS {
        if let Some(default) = optional_default(key) {
            known.insert(key.to_string(), default);
        }
    }
    let mut problems = Obj::new();
    for (key, value) in patch {
        if let Some(reason) = retired_reason(key) {
            problems.insert(key.clone(), Value::String(format!("retired MiniMax H3 setting: {reason}")));
            continue;
        }
        let Some(default) = known.get(key) else {
            problems.insert(key.clone(), Value::String("unknown MiniMax H3 setting".into()));
            continue;
        };
        if let Err(e) = check_value(key, value, default, false) {
            problems.insert(key.clone(), Value::String(e.message));
        }
    }
    problems
}

fn str_of<'a>(settings: &'a Obj, key: &str) -> &'a str {
    settings.get(key).and_then(Value::as_str).unwrap_or("")
}

/// Defaults overlaid with every valid saved value. Invalid or unknown saved keys fall back to defaults.
pub fn normalize_settings(raw: Option<&Value>) -> Obj {
    let mut settings = defaults();
    let Some(Value::Object(raw)) = raw else { return settings };
    let keys: Vec<String> = settings.keys().cloned().collect();
    for key in keys {
        let Some(saved) = raw.get(&key) else { continue };
        let default = settings[&key].clone();
        let mut value = saved.clone();
        if key == "advanced_two_pass_vram_preset" {
            // Retired 32gb / custom saves map to 24gb.
            match normalize_vram_preset(Some(&text_or_empty(Some(&value)))) {
                Ok(preset) => value = Value::String(preset),
                Err(_) => continue,
            }
        }
        if let Ok(checked) = check_value(&key, &value, &default, true) {
            settings.insert(key, checked);
        }
    }

    if str_of(&settings, "pipeline") == "refmod" {
        // The RefMod pipeline has one mode and no 2 Pass Advanced.
        settings.insert("video_mode".into(), json!("reference_to_video"));
        if str_of(&settings, "render_pass") == "three_pass" {
            settings.insert("render_pass".into(), json!("two_pass"));
        }
    } else if str_of(&settings, "video_mode") == "image_to_video" {
        // Old I2V saves carried render_pass but always rendered single pass.
        let version = raw.get("i2v_pass_settings_version").filter(|v| truthy(v)).and_then(to_float).map_or(0, |f| f as i64);
        if version < 1 {
            settings.insert("render_pass".into(), json!("single"));
        } else if str_of(&settings, "render_pass") == "three_pass" {
            settings.insert("render_pass".into(), json!("two_pass"));
        }
        settings.insert("i2v_pass_settings_version".into(), json!(1));
        let base = defaults();
        if settings.get("diffusion_model_name") == base.get("diffusion_model_name") {
            settings.insert("diffusion_model_name".into(), json!("minimax_h3_fl2va_pruned_int8_convrot.safetensors"));
        }
        if settings.get("two_pass_lora_name") == base.get("two_pass_lora_name") {
            settings.insert(
                "two_pass_lora_name".into(),
                json!("minimax_h3_fl2v_turbo_4step_v1.1_768p_comfyui_bf16.safetensors"),
            );
        }
    } else if raw.get("video_mode").and_then(Value::as_str) == Some("text_to_video") {
        // Older T2V saves ignored render_pass even when it held the global default. Only a save that names the mode counts: the default mode is T2V.
        let version = raw.get("t2v_pass_settings_version").filter(|v| truthy(v)).and_then(to_float).map_or(0, |f| f as i64);
        if version < 1 {
            settings.insert("render_pass".into(), json!("single"));
        } else if str_of(&settings, "render_pass") == "three_pass" {
            settings.insert("render_pass".into(), json!("two_pass"));
        }
        settings.insert("t2v_pass_settings_version".into(), json!(1));
    }

    // One output resolution for every pass type. Older saves took it from the pass type they rendered with.
    let (preset, megapixels) = migrate_resolution(raw, str_of(&settings, "render_pass"));
    settings.insert("resolution_preset".into(), json!(preset));
    settings.insert("megapixels".into(), json!(megapixels));

    for key in OPTIONAL_KEYS {
        if let (Some(saved), Some(default)) = (raw.get(key), optional_default(key)) {
            if let Ok(checked) = check_value(key, saved, &default, true) {
                settings.insert(key.to_string(), checked);
            }
        }
    }

    // Masked continuation plans its own head and tail.
    let continuity = str_of(&settings, "continuity_mode").to_string();
    if continuity == MASKED
        && continuity_allowed_for_mode(&continuity, str_of(&settings, "video_mode"), str_of(&settings, "render_pass"))
    {
        settings.insert("warmup_frames".into(), json!(0));
        settings.insert("cooldown_frames".into(), json!(0));
    }
    settings
}

/// Project settings, overlaid with the scene's own settings when the scene opts in (mirrors the UI).
pub fn settings_for_scene(session: &Value, segment: Option<&Value>) -> Obj {
    let project_raw = session.get("minimax_h3_settings").and_then(Value::as_object);
    let mut merged: Obj = project_raw.cloned().unwrap_or_default();
    if let Some(segment) = segment.and_then(Value::as_object) {
        if segment.get("use_scene_minimax_h3_settings").is_some_and(truthy) {
            let scene_raw = segment.get("minimax_h3_settings");
            if let Some(Value::Object(scene)) = scene_raw {
                for (k, v) in scene {
                    merged.insert(k.clone(), v.clone());
                }
            }
            let scene_has_mode = matches!(scene_raw, Some(Value::Object(o)) if o.contains_key("video_mode"));
            let scene_truthy = scene_raw.is_some_and(truthy);
            if !scene_truthy || !scene_has_mode {
                if let Some(mode) = segment.get("minimax_h3_mode").filter(|v| truthy(v)) {
                    merged.insert("video_mode".into(), mode.clone());
                }
            }
            // The pipeline belongs to the whole project, so a scene with its own settings follows it.
            let pipeline = project_raw.and_then(|p| p.get("pipeline")).cloned().unwrap_or_else(|| json!("standard"));
            merged.insert("pipeline".into(), pipeline);
            if str_of(&merged, "video_mode") == "text_to_video" {
                if let Some(Value::Object(scene)) = scene_raw {
                    merged.insert("t2v_pass_settings_version".into(), scene.get("t2v_pass_settings_version").cloned().unwrap_or(json!(0)));
                }
            }
        }
    }
    normalize_settings(Some(&Value::Object(merged)))
}

/// Merge an intentional API edit without treating pass selection as an old save. Unedited legacy saves still migrate to Single. An explicit
/// pass edit opts into the current pass semantics, like the Builder's pass buttons do. `inherited_mode` is the project's mode, for a scene.
pub fn apply_settings_patch(current: Option<&Obj>, patch: &Obj, inherited_mode: Option<&str>) -> Obj {
    let mut merged: Obj = current.cloned().unwrap_or_default();
    for (key, value) in patch {
        merged.insert(key.clone(), value.clone());
    }
    if patch.contains_key("render_pass") {
        let mode = merged.get("video_mode").filter(|v| truthy(v)).map(|v| text_or_empty(Some(v))).or_else(|| inherited_mode.filter(|m| !m.is_empty()).map(str::to_string)).unwrap_or_else(|| "text_to_video".into());
        if mode == "text_to_video" {
            merged.insert("t2v_pass_settings_version".into(), json!(1));
        } else if mode == "image_to_video" {
            merged.insert("i2v_pass_settings_version".into(), json!(1));
        }
    }
    merged
}

/// Supported pass: T2V and I2V have Single and 2 Pass, reference modes also have Advanced.
pub fn render_pass(settings: &Obj) -> &'static str {
    let pass = match settings.get("render_pass") {
        Some(v) if truthy(v) => text_or_empty(Some(v)),
        _ => "single".to_string(),
    };
    let mode = str_of(settings, "video_mode");
    if pass == "two_pass" && (mode == "image_to_video" || mode == "text_to_video") {
        return "two_pass";
    }
    if (pass == "two_pass" || pass == "three_pass") && REFERENCE_MODES.contains(&mode) {
        return if pass == "two_pass" { "two_pass" } else { "three_pass" };
    }
    "single"
}

/// Graph builder key for the settings' render pass.
pub fn workflow_key(settings: &Obj) -> &'static str {
    match render_pass(settings) {
        "two_pass" => WORKFLOW_TWO_PASS,
        "three_pass" => WORKFLOW_ADVANCED,
        _ => WORKFLOW_SINGLE,
    }
}

/// Map normalized settings to the payload keys the graph builders read. Scene specific keys (folder, scene number,
/// prompt, audio, references, timing) are the caller's job. `overrides` replaces any key afterwards.
pub fn build_render_payload(settings: &Obj, overrides: Option<&Obj>) -> Result<Obj, CoreError> {
    let s = settings;
    let g = |key: &str| s.get(key).cloned().unwrap_or(Value::Null);
    let pass = render_pass(s);
    let two_pass = pass == "two_pass";
    let advanced = pass == "three_pass";
    let multi = two_pass || advanced;
    let pick = |single: Value, two: Value, adv: Value| if two_pass { two } else if advanced { adv } else { single };

    let mut continuity = match s.get("continuity_mode") {
        Some(v) if truthy(v) => text_or_empty(Some(v)),
        _ => "off".to_string(),
    };
    let mode = str_of(s, "video_mode");
    if (mode == "image_to_video" || mode == "image_reference_to_video")
        && !continuity_allowed_for_mode(&continuity, mode, str_of(s, "render_pass"))
    {
        continuity = "off".to_string();
    }
    let masked = continuity == MASKED && !advanced;
    let frames = |key: &str| if masked { 0 } else { s.get(key).and_then(to_float).map_or(0, |f| (f as i64).max(0)) };

    let mut p = Obj::new();
    let mut put = |key: &str, value: Value| {
        p.insert(key.to_string(), value);
    };
    put("audio_mode", g("audio_mode"));
    put("pipeline", g("pipeline"));
    put("video_mode", g("video_mode"));
    put("continuity_mode", json!(continuity));
    put("latent_context_frames", g("latent_context_frames"));
    put("minimax_h3_latent_context_frames", g("latent_context_frames"));
    put("pre_frames", json!(frames("warmup_frames")));
    put("tail_loss_frames", json!(frames("cooldown_frames")));
    for key in [
        "seed", "aspect_ratio", "megapixels", "diffusion_model_name", "clip_name", "video_vae_name", "audio_vae_name",
        "sampler_name", "scheduler", "steps", "denoise", "ref_image_size", "two_pass_lora_name", "two_pass_lora_strength",
        "use_fast_vae_decode",
    ] {
        put(key, g(key));
    }
    put("te_speed_processing_control", g("two_pass_te_speed_processing_control"));
    put("te_speed_start_percent", g("two_pass_te_speed_start_percent"));
    put("te_speed_end_percent", g("two_pass_te_speed_end_percent"));
    put("te_speed_mcs", g("two_pass_te_speed_mcs"));
    put("te_speed_cache_depth", g("two_pass_te_speed_cache_depth"));
    put("te_speed_device", g("two_pass_te_speed_device"));
    put("three_pass_lightx_lora_name", g("three_pass_lightx_lora_name"));
    put("three_pass_lightx_lora_strength", g("three_pass_lightx_lora_strength"));
    put("pass1_steps", pick(g("steps"), g("two_pass_pass1_steps"), g("advanced_two_pass_pass1_steps")));
    put("pass1_denoise", pick(g("denoise"), g("two_pass_pass1_denoise"), g("advanced_two_pass_pass1_denoise")));
    put("pass1_sampler_name", pick(g("sampler_name"), g("two_pass_pass1_sampler"), g("advanced_two_pass_pass1_sampler")));
    put("pass1_scheduler", pick(g("scheduler"), g("two_pass_pass1_scheduler"), g("advanced_two_pass_pass1_scheduler")));
    put("pass1_seed", pick(g("seed"), g("two_pass_pass1_seed"), g("advanced_two_pass_pass1_seed")));
    put("pass2_steps", pick(json!(4), g("two_pass_pass2_steps"), g("advanced_two_pass_pass2_steps")));
    put("pass2_denoise", pick(json!(0.2), g("two_pass_pass2_denoise"), g("advanced_two_pass_pass2_denoise")));
    put("pass2_sampler_name", pick(g("sampler_name"), g("two_pass_pass2_sampler"), g("advanced_two_pass_pass2_sampler")));
    put("pass2_scheduler", pick(g("scheduler"), g("two_pass_pass2_scheduler"), g("advanced_two_pass_pass2_scheduler")));
    put("pass2_seed", pick(g("seed"), g("two_pass_pass2_seed"), g("advanced_two_pass_pass2_seed")));
    for key in [
        "sage_attention", "use_memory_efficient_sage_attention", "enable_fp16_accumulation", "use_loras", "lora_count",
        "loras", "use_turbo_lora", "turbo_lora_name", "turbo_lora_strength", "easy_cache_bypass",
        "easy_cache_reuse_threshold", "easy_cache_start_percent", "easy_cache_end_percent", "easy_cache_verbose",
    ] {
        put(key, g(key));
    }

    // Per-pass acceleration toggles: pass{n}_use_{te_speed,feedforward,block_sparse_attention}.
    for key in ["te_speed", "feedforward", "block_sparse_attention"] {
        for number in [1, 2] {
            let own = format!("pass{number}_use_{key}");
            let value = s.get(&own).cloned().unwrap_or_else(|| g(&format!("two_pass_use_{key}")));
            put(&own, value);
        }
    }

    if two_pass {
        let (w, h) = output_frame_size(s);
        put("final_width", json!(w));
        put("final_height", json!(h));
        put("latent_upscale_scale", g("two_pass_latent_upscale_scale"));
    }
    if multi {
        put("latent_upscaler_name", g("two_pass_latent_upscaler_name"));
        put("two_pass_use_feedforward", g("two_pass_use_feedforward"));
        put("two_pass_use_block_sparse_attention", g("two_pass_use_block_sparse_attention"));
        put("two_pass_use_fast_vae_decode", g("two_pass_use_fast_vae_decode"));
        put("final_resize_method", g("two_pass_final_resize_method"));
        put("output_crf", g("two_pass_output_crf"));
    } else {
        put("use_te_speed", g("use_te_speed"));
        put("use_feedforward", g("use_feedforward"));
        put("use_block_sparse_attention", g("use_block_sparse_attention"));
    }

    // Legacy 3-pass profile values the UI still forwards for every pass.
    for number in [1, 2, 3] {
        for field in ["megapixels", "steps", "denoise", "sampler", "scheduler", "seed", "te_speed"] {
            let key = format!("three_pass_pass{number}_{field}");
            put(&key, g(&key));
        }
    }

    // 2 Pass Advanced: the Pass 2 size is the shared output resolution. Tiles, chunks, fades and the upscaler device
    // are planned from it and the VRAM preset when the graph is built.
    put("advanced_pass1_megapixels", g("advanced_two_pass_pass1_megapixels"));
    put("advanced_pass2_megapixels", g("megapixels"));
    let vram = normalize_vram_preset(Some(&text_or_empty(s.get("advanced_two_pass_vram_preset"))))?;
    put("advanced_vram_preset", json!(vram));

    if let Some(extra) = overrides {
        for (k, v) in extra {
            p.insert(k.clone(), v.clone());
        }
    }
    Ok(p)
}
