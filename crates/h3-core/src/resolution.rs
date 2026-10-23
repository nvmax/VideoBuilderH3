//! Output-resolution math shared by every render pass type. Port of `minimax/resolution.py`.

use serde_json::Value;

use crate::pyval::{round_to, text_or_empty, to_float, truthy, Obj};

pub const RESOLUTION_PRESETS: [&str; 5] = ["custom", "1k", "2k", "1440p", "4k"];
pub const FRAME_MULTIPLE_PX: i64 = 32;
pub const DEFAULT_ASPECT_RATIO: &str = "16:9 (Widescreen)";
pub const DEFAULT_RESOLUTION_PRESET: &str = "1k";

fn long_edge_px(preset: &str) -> Option<i64> {
    match preset {
        "1k" => Some(1024),
        "2k" => Some(1920),
        "1440p" => Some(2560),
        "4k" => Some(3840),
        _ => None,
    }
}

/// JavaScript `Math.round`: half up.
fn js_round(value: f64) -> i64 {
    (value + 0.5).floor() as i64
}

/// `(width, height)` ratio from a label such as `16:9 (Widescreen)`. Falls back to 16:9, and a zero side to 16 or 9.
pub fn aspect_ratio_parts(aspect_ratio: &str) -> (i64, i64) {
    let bytes = aspect_ratio.as_bytes();
    // First `digits ws* : ws* digits` run, like `re.search(r"(\d+)\s*:\s*(\d+)", ...)`.
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            let first = &aspect_ratio[start..i];
            let mut j = i;
            while j < bytes.len() && (bytes[j] as char).is_whitespace() {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b':' {
                j += 1;
                while j < bytes.len() && (bytes[j] as char).is_whitespace() {
                    j += 1;
                }
                let second_start = j;
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if j > second_start {
                    let w: i64 = first.parse().unwrap_or(0);
                    let h: i64 = aspect_ratio[second_start..j].parse().unwrap_or(0);
                    return (if w == 0 { 16 } else { w }, if h == 0 { 9 } else { h });
                }
            }
            continue;
        }
        i += 1;
    }
    (16, 9)
}

/// Frame size in pixels (multiples of 32) for a megapixel target and aspect ratio.
pub fn frame_size(megapixels: f64, aspect_ratio: &str) -> (i64, i64) {
    let (rw, rh) = aspect_ratio_parts(aspect_ratio);
    let target = 0.1f64.max(if megapixels != 0.0 { megapixels } else { 2.0 });
    let scale = (target * 1048576.0 / (rw * rh) as f64).sqrt();
    let m = FRAME_MULTIPLE_PX as f64;
    (js_round(rw as f64 * scale / m) * FRAME_MULTIPLE_PX, js_round(rh as f64 * scale / m) * FRAME_MULTIPLE_PX)
}

/// Megapixels of a preset at an aspect ratio, or `None` for `custom` and unknown presets.
pub fn preset_megapixels(preset: &str, aspect_ratio: &str) -> Option<f64> {
    let long_edge = long_edge_px(&preset.trim().to_lowercase())?;
    let (rw, rh) = aspect_ratio_parts(aspect_ratio);
    let scale = long_edge as f64 / rw.max(rh) as f64;
    let m = FRAME_MULTIPLE_PX as f64;
    let width = js_round(rw as f64 * scale / m) * FRAME_MULTIPLE_PX;
    let height = js_round(rh as f64 * scale / m) * FRAME_MULTIPLE_PX;
    Some(round_to((width * height) as f64 / 1048576.0, 4))
}

fn clamp_megapixels(value: Option<&Value>, fallback: f64) -> f64 {
    let number = value.and_then(to_float).unwrap_or(0.0);
    0.1f64.max(16.0f64.min(if number != 0.0 { number } else { fallback }))
}

fn is_positive(value: Option<&Value>) -> bool {
    value.and_then(to_float).is_some_and(|f| f > 0.0)
}

fn aspect_of(raw: &Obj) -> String {
    match raw.get("aspect_ratio") {
        Some(v) if truthy(v) => text_or_empty(Some(v)),
        _ => DEFAULT_ASPECT_RATIO.to_string(),
    }
}

/// Output megapixels of a settings dict: the preset's size, else the custom value (0.9 default).
pub fn resolved_megapixels(settings: &Obj) -> f64 {
    let preset = text_or_empty(settings.get("resolution_preset"));
    if let Some(mp) = preset_megapixels(&preset, &text_or_empty(settings.get("aspect_ratio"))) {
        return mp;
    }
    match settings.get("megapixels").and_then(to_float) {
        Some(f) if f != 0.0 => f,
        _ => 0.9,
    }
}

/// Output width and height in pixels of a settings dict.
pub fn output_frame_size(settings: &Obj) -> (i64, i64) {
    frame_size(resolved_megapixels(settings), &text_or_empty(settings.get("aspect_ratio")))
}

/// `(resolution_preset, megapixels)` for a saved settings dict, including older saves that took their resolution from
/// the pass type they rendered with. Twin of `resolveMiniMaxH3Resolution` in `minimax_h3.mjs`.
pub fn migrate_resolution(raw: &Obj, render_pass: &str) -> (String, f64) {
    let aspect = aspect_of(raw);
    let saved = text_or_empty(raw.get("resolution_preset")).trim().to_lowercase();
    if RESOLUTION_PRESETS.contains(&saved.as_str()) {
        let mp = preset_megapixels(&saved, &aspect).unwrap_or_else(|| clamp_megapixels(raw.get("megapixels"), 0.9));
        return (saved, mp);
    }
    let final_w = raw.get("two_pass_final_width");
    let final_h = raw.get("two_pass_final_height");
    let has_final_size = is_positive(final_w) && is_positive(final_h);
    let present = |k: &str| raw.get(k).is_some_and(|v| !v.is_null());
    let has_legacy = present("megapixels")
        || has_final_size
        || present("advanced_two_pass_pass2_megapixels")
        || present("advanced_two_pass_pass2_resolution_preset");
    if !has_legacy {
        let mp = preset_megapixels(DEFAULT_RESOLUTION_PRESET, &aspect).unwrap_or(0.5625);
        return (DEFAULT_RESOLUTION_PRESET.to_string(), mp);
    }
    let mut preset = "custom".to_string();
    let megapixels;
    if render_pass == "three_pass" {
        let legacy = text_or_empty(raw.get("advanced_two_pass_pass2_resolution_preset")).trim().to_lowercase();
        preset = if RESOLUTION_PRESETS.contains(&legacy.as_str()) { legacy } else { "custom".into() };
        megapixels = preset_megapixels(&preset, &aspect)
            .unwrap_or_else(|| clamp_megapixels(raw.get("advanced_two_pass_pass2_megapixels"), 2.0));
    } else if render_pass == "two_pass" && has_final_size {
        let w = final_w.and_then(to_float).unwrap_or(0.0);
        let h = final_h.and_then(to_float).unwrap_or(0.0);
        megapixels = clamp_megapixels(Some(&Value::from(w * h / 1048576.0)), 0.9);
    } else {
        megapixels = clamp_megapixels(raw.get("megapixels"), 0.9);
    }
    if preset == "custom" {
        let size = frame_size(megapixels, &aspect);
        for key in RESOLUTION_PRESETS {
            if let Some(candidate) = preset_megapixels(key, &aspect) {
                if frame_size(candidate, &aspect) == size {
                    return (key.to_string(), candidate);
                }
            }
        }
    }
    (preset, round_to(megapixels, 4))
}
