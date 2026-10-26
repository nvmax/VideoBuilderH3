//! Templates, reference media, scene audio and output folders. Port of `runner/minimax_inputs.py` and
//! `runner/minimax_keyframes.py`.

use std::path::{Path, PathBuf};

use h3_core::pyjson;
use h3_core::pyval::{first_value, float_or_zero, text_or_empty, to_float, truthy, Obj};
use serde_json::{json, Value};
use sha1::{Digest, Sha1};

use crate::api::{as_link, link};
use crate::env::{GraphEnv, GraphError, Result};

pub const ASPECT_RATIOS: [&str; 8] = [
    "1:1 (Square)",
    "2:3 (Portrait Photo)",
    "3:2 (Photo)",
    "3:4 (Portrait Standard)",
    "4:3 (Standard)",
    "9:16 (Portrait Widescreen)",
    "16:9 (Widescreen)",
    "21:9 (Ultrawide)",
];

const MAX_REFERENCE_IMAGES: usize = 9;
const MAX_REFERENCE_VIDEOS: usize = 3;

/// The embedded copy of a pack workflow template. These are the API graphs the Video Builder patches.
pub fn template(name: &str) -> Result<(String, Obj)> {
    let text = match name {
        "minimax_audio_driven_builder_api.json" => include_str!("../templates/minimax_audio_driven_builder_api.json"),
        "minimax_audio_driven_builder_latent_upscale_2pass_api.json" => {
            include_str!("../templates/minimax_audio_driven_builder_latent_upscale_2pass_api.json")
        }
        "minimax_i2v_audio_driven_builder_latent_upscale_2pass_api.json" => {
            include_str!("../templates/minimax_i2v_audio_driven_builder_latent_upscale_2pass_api.json")
        }
        "minimax_built_in_audio_builder_api.json" => include_str!("../templates/minimax_built_in_audio_builder_api.json"),
        "lyrics_timestamp_api.json" => include_str!("../templates/lyrics_timestamp_api.json"),
        "clear_memory_api.json" => include_str!("../templates/clear_memory_api.json"),
        "beat_srt_api.json" => include_str!("../templates/beat_srt_api.json"),
        "text2image_zimage_API.json" => include_str!("../templates/text2image_zimage_API.json"),
        "Krea2_TextToImage_API.json" => include_str!("../templates/Krea2_TextToImage_API.json"),
        "Krea2_API_2Pass.json" => include_str!("../templates/Krea2_API_2Pass.json"),
        "fluxKleinMultiImage_API.json" => include_str!("../templates/fluxKleinMultiImage_API.json"),
        "image_ernie_image_turbo_API.json" => include_str!("../templates/image_ernie_image_turbo_API.json"),
        "NB_API.json" => include_str!("../templates/NB_API.json"),
        "z_upscaleEnhance_API.json" => include_str!("../templates/z_upscaleEnhance_API.json"),
        other => return Err(GraphError::new(format!("Workflow API template was not found: {other}"))),
    };
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(map)) if !map.is_empty() => Ok((name.to_string(), map)),
        _ => Err(GraphError::new("Workflow API template is not a valid ComfyUI API prompt JSON.")),
    }
}

/// `os.path.abspath`.
pub fn abspath(path: &str) -> String {
    h3_core::abspath::absolute(Path::new(path)).map(|p| p.display().to_string()).unwrap_or_else(|_| path.to_string())
}

/// `str(value or "").strip().strip('"').strip("'")` for a path typed by a person.
pub fn clean_path_text(value: Option<&Value>) -> String {
    text_or_empty(value).trim().trim_matches('"').to_string()
}

fn normalize_name(value: &str) -> String {
    value.trim().to_lowercase().replace(['-', ' '], "_")
}

/// `str(payload.get(key) or default).strip().lower()` with dashes and spaces turned into underscores.
pub fn mode_text(payload: &Obj, keys: &[&str], default: &str) -> String {
    let pick = keys.iter().filter_map(|k| payload.get(*k)).find(|v| truthy(v));
    match pick {
        Some(v) => normalize_name(&text_or_empty(Some(v))),
        None => default.to_string(),
    }
}

/// `input_audio` or `built_in_audio` from the many spellings the UI and API use.
pub fn audio_mode(payload: &Obj) -> &'static str {
    let raw = mode_text(payload, &["audio_mode", "audioMode"], "input_audio");
    if matches!(raw.as_str(), "built_in_audio" | "native_audio" | "generated_audio") { "built_in_audio" } else { "input_audio" }
}

/// A list from a list, an object, JSON text or lines of text.
fn collection(value: Option<&Value>, keys: &[&str]) -> Vec<Value> {
    match value {
        None | Some(Value::Null) => vec![],
        Some(Value::Array(items)) => items.clone(),
        Some(Value::Object(map)) => {
            for key in keys {
                if let Some(Value::Array(items)) = map.get(*key) {
                    return items.clone();
                }
            }
            map.values().cloned().collect()
        }
        Some(other) => {
            let text = text_or_empty(Some(other)).trim().to_string();
            if text.is_empty() {
                return vec![];
            }
            if let Ok(parsed @ (Value::Array(_) | Value::Object(_))) = serde_json::from_str::<Value>(&text) {
                return collection(Some(&parsed), keys);
            }
            text.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).map(Value::String).collect()
        }
    }
}

fn media_path(value: &Value) -> String {
    let source = match value {
        Value::Object(map) => ["path", "file", "image", "video"].iter().filter_map(|k| map.get(*k)).find(|v| truthy(v)).cloned(),
        other => Some(other.clone()),
    };
    text_or_empty(source.as_ref()).trim().trim_matches(['"', '\'']).to_string()
}

/// The scene's reference image paths, at most nine.
pub fn image_paths(payload: &Obj) -> Result<Vec<String>> {
    let raw = first_value(payload, &["image_paths", "reference_images", "images"]);
    let paths: Vec<String> = collection(raw, &["image_paths", "images"]).iter().map(media_path).filter(|p| !p.is_empty()).collect();
    if paths.len() > MAX_REFERENCE_IMAGES {
        return Err(GraphError::new(format!(
            "MiniMax H3 supports at most {MAX_REFERENCE_IMAGES} reference images; received {}.",
            paths.len()
        )));
    }
    Ok(paths)
}

/// The scene's reference videos as `{path, start_seconds, duration, use_audio}`, at most three.
pub fn video_references(payload: &Obj) -> Result<Vec<Obj>> {
    let raw = first_value(payload, &["video_references", "reference_videos", "videos"]);
    let mut references = Vec::new();
    for item in collection(raw, &["video_references", "videos"]) {
        let (path, start, duration, use_audio) = match &item {
            Value::Object(map) => {
                let seconds = |keys: &[&str]| -> Result<f64> {
                    let pick = first_value(map, keys).filter(|v| truthy(v));
                    let value = match pick {
                        None => 0.0,
                        Some(v) => to_float(v).ok_or_else(|| GraphError::new("MiniMax H3 video reference timing must be numeric."))?,
                    };
                    Ok(value.max(0.0))
                };
                let use_audio = match first_value(map, &["use_audio", "include_audio", "reference_audio"]) {
                    Some(Value::String(s)) => matches!(s.trim().to_lowercase().as_str(), "1" | "true" | "yes" | "on"),
                    Some(v) => truthy(v),
                    None => false,
                };
                (media_path(&item), seconds(&["start_seconds", "start", "seek_seconds"])?, seconds(&["duration", "duration_seconds"])?, use_audio)
            }
            other => (media_path(other), 0.0, 0.0, false),
        };
        if !path.is_empty() {
            let mut entry = Obj::new();
            entry.insert("path".into(), json!(path));
            entry.insert("start_seconds".into(), json!(start));
            entry.insert("duration".into(), json!(duration));
            entry.insert("use_audio".into(), json!(use_audio));
            references.push(entry);
        }
    }
    if references.len() > MAX_REFERENCE_VIDEOS {
        return Err(GraphError::new(format!(
            "MiniMax H3 supports at most {MAX_REFERENCE_VIDEOS} reference videos; received {}.",
            references.len()
        )));
    }
    Ok(references)
}

/// Replace the H3 reference node with the native first and last frame node (or the image plus reference node). Both
/// emit positive conditioning and the joint video and audio latent, so the sampler, audio and output chain stay the
/// same. The media loader's first image outputs map straight to the node's frame inputs.
pub fn patch_image_to_video_node(prompt: &mut Obj, image_paths: &[String], include_references: bool, has_last_frame: bool) -> Result<()> {
    let Some(node) = prompt.get_mut("136").and_then(Value::as_object_mut) else {
        return Err(GraphError::new("'MiniMax H3 image-to-video node 136 was not found.'"));
    };
    if image_paths.is_empty() {
        return Err(GraphError::new("MiniMax H3 image-to-video requires a first-frame image."));
    }
    if !include_references && image_paths.len() > 2 {
        return Err(GraphError::new("MiniMax H3 image-to-video accepts at most a first and last frame."));
    }
    let mut inputs = Obj::new();
    inputs.insert("prompt".into(), link("138", 0));
    inputs.insert("width".into(), link("115", 0));
    inputs.insert("height".into(), link("115", 1));
    inputs.insert("length".into(), link("131", 1));
    inputs.insert("clip".into(), link("128", 0));
    inputs.insert("vae".into(), link("119", 0));
    inputs.insert("first_frame".into(), link("180", 0));
    if (include_references && has_last_frame) || (!include_references && image_paths.len() > 1) {
        inputs.insert("last_frame".into(), link("180", 1));
    }
    if include_references {
        let ref_start = if has_last_frame { 2 } else { 1 };
        for (ref_index, slot) in (ref_start..image_paths.len()).enumerate() {
            inputs.insert(format!("ref_images.ref_image_{ref_index}"), link("180", slot as i64));
        }
        inputs.insert("ref_image_size".into(), json!("max"));
        node.insert("class_type".into(), json!("VRGDG_MiniMaxH3ImageReferenceToVideo"));
        node.insert("_meta".into(), json!({"title": "MiniMax H3 Image + Reference to Video"}));
    } else {
        node.insert("class_type".into(), json!("MiniMaxH3ImageToVideo"));
        node.insert("_meta".into(), json!({"title": "MiniMax H3 Image to Video"}));
    }
    node.insert("inputs".into(), Value::Object(inputs));
    Ok(())
}

/// Windows lower cases a path and uses backslashes when comparing. Other systems compare it as written.
fn normcase(path: &str) -> String {
    if cfg!(windows) { path.replace('/', "\\").to_lowercase() } else { path.to_string() }
}

/// The scene's scratch folder under ComfyUI's output folder and the filename prefix of its renders. The folder name holds
/// a hash of the project path, so two projects with the same name never share a folder.
pub fn output_location(env: &dyn GraphEnv, project_folder: &str, scene: u32, create: bool) -> (String, String) {
    let (folder, prefix) = output_location_in(&env.output_dir(), project_folder, scene);
    if create {
        env.ensure_dir(&folder);
    }
    (folder, prefix)
}

/// [`output_location`] from ComfyUI's output folder alone, without creating anything.
pub fn output_location_in(output_dir: &str, project_folder: &str, scene: u32) -> (String, String) {
    let base = Path::new(project_folder).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut name = String::new();
    let mut in_run = false;
    for c in base.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            name.push(c);
            in_run = false;
        } else if !in_run {
            name.push('_');
            in_run = true;
        }
    }
    let name = name.trim_matches('_');
    let name = if name.is_empty() { "project" } else { name };
    let digest = Sha1::digest(normcase(project_folder).as_bytes());
    let key: String = digest.iter().take(4).map(|b| format!("{b:02x}")).collect();
    let relative: PathBuf = ["VRGDG_MiniMaxH3".to_string(), format!("{name}_{key}"), format!("scene_{scene:04}")].iter().collect();
    let folder = PathBuf::from(output_dir).join(&relative);
    let prefix = relative.join(format!("MiniMaxH3_scene_{scene:04}")).display().to_string().replace('\\', "/");
    (folder.display().to_string(), prefix)
}

/// Keep the first and last frame anchors of an I2V render on the range the exact scene trim keeps.
pub fn patch_i2v_keyframe_timing(prompt: &mut Obj, timing: &h3_core::timing::TimingPlan) {
    let first = (timing.final_trim_start_seconds * 24.0).round_ties_even() as i64;
    let last = first + timing.final_frame_count - 1;
    for source in ["136", "214"] {
        let qualifies = prompt.get(source).and_then(Value::as_object).is_some_and(|n| {
            n.get("class_type").and_then(Value::as_str) == Some("MiniMaxH3ImageToVideo")
                && n.get("inputs").and_then(|i| i.get("last_frame")).is_some()
        });
        if !qualifies {
            continue;
        }
        let target = format!("i2v_keyframe_timing_{source}");
        let wanted = link(source, 0);
        for consumer in prompt.values_mut() {
            let Some(map) = consumer.get_mut("inputs").and_then(Value::as_object_mut) else { continue };
            for value in map.values_mut() {
                if *value == wanted {
                    *value = link(&target, 0);
                }
            }
        }
        prompt.insert(
            target,
            json!({
                "class_type": "VRGDG_MiniMaxH3KeyframeTiming",
                "inputs": {"conditioning": [source, 0], "first_frame_index": first, "last_frame_index": last},
                "_meta": {"title": "I2V First / Last Frame \u{2014} Exact Scene Range"},
            }),
        );
    }
}

/// `json.dumps(list, ensure_ascii=False)` as the media loader node expects it.
pub fn json_list_text(items: &[Value]) -> String {
    pyjson::dumps_inline(&Value::Array(items.to_vec()))
}

/// Is `value` a two item link? Re-exported for the patch code.
pub fn is_link(value: &Value) -> bool {
    as_link(value).is_some()
}

/// `float(x or 0)` helper for payload numbers that may be text.
pub fn number_or_zero(value: Option<&Value>) -> f64 {
    float_or_zero(value).unwrap_or(0.0)
}
