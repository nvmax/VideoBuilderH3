//! Scene images and reference card images, recorded the way the Video Builder records them. Port of
//! `_archive_scene_image` (`builder/media.py`) and the scene image lifecycle in
//! `agent_api/orchestrator/image_orchestrator.py` (section 24.1 of the pack's spec).
//!
//! Generated images are archived under `scene_image_previews/scene_NNNN/` and listed in the scene's `image_history`.
//! Approving one makes it `approved_image_path`, which is what the video render reads.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use base64::Engine;
use h3_core::pyval::{text_or_empty, to_float, Obj};
use serde_json::{json, Value};

use crate::layout::Stamp;

pub fn scene_preview_folder(project: &Path, scene_number: u32) -> PathBuf {
    project.join("scene_image_previews").join(format!("scene_{:04}", scene_number.max(1)))
}

/// `preview_<stamp>.<ext>` in the scene's preview folder, with `_02`, `_03`... when the name is taken.
pub fn unique_preview_path(project: &Path, scene_number: u32, extension: &str, now: &Stamp) -> io::Result<PathBuf> {
    let folder = scene_preview_folder(project, scene_number);
    fs::create_dir_all(&folder)?;
    let ext = extension.trim_start_matches('.').to_lowercase();
    let ext = if matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp") { ext } else { "png".to_string() };
    let base = folder.join(format!("preview_{}.{ext}", now.compact));
    if !base.exists() {
        return Ok(base);
    }
    let mut index = 2;
    loop {
        let candidate = folder.join(format!("preview_{}_{index:02}.{ext}", now.compact));
        if !candidate.exists() {
            return Ok(candidate);
        }
        index += 1;
    }
}

/// Copy an image into the scene's preview folder. Returns the new path.
pub fn archive_scene_image(project: &Path, scene_number: u32, source: &Path, now: &Stamp) -> io::Result<PathBuf> {
    if !source.is_file() {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("Image file not found: {}", source.display())));
    }
    let ext = source.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_else(|| "png".into());
    let target = unique_preview_path(project, scene_number, &ext, now)?;
    fs::copy(source, &target)?;
    Ok(target)
}

/// Write an image sent as a data URL (or plain base64) to a temporary file next to the previews and return its path and
/// extension. The caller archives it.
pub fn decode_image_data(data: &str) -> io::Result<(Vec<u8>, String)> {
    let text = data.trim();
    let (header, encoded) = match text.split_once(',') {
        Some((h, e)) if h.to_lowercase().starts_with("data:") => (h.to_lowercase(), e),
        _ => (String::new(), text),
    };
    let ext = if header.contains("jpeg") || header.contains("jpg") {
        "jpg"
    } else if header.contains("webp") {
        "webp"
    } else {
        "png"
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("The image data is not valid base64: {e}")))?;
    if bytes.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "The image data is empty."));
    }
    Ok((bytes, ext.to_string()))
}

/// Archive image bytes into the scene's preview folder. Returns the new path.
pub fn archive_scene_image_bytes(project: &Path, scene_number: u32, bytes: &[u8], ext: &str, now: &Stamp) -> io::Result<PathBuf> {
    let target = unique_preview_path(project, scene_number, ext, now)?;
    fs::write(&target, bytes)?;
    Ok(target)
}

fn segment_mut(session: &mut Obj, index: usize) -> io::Result<&mut Obj> {
    session
        .get_mut("segments")
        .and_then(Value::as_array_mut)
        .and_then(|a| a.get_mut(index))
        .and_then(Value::as_object_mut)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Scene not found."))
}

fn history(segment: &Obj) -> Vec<String> {
    segment.get("image_history").and_then(Value::as_array).map(|a| a.iter().map(|v| text_or_empty(Some(v))).collect()).unwrap_or_default()
}

/// A new image for the scene: it joins the history and becomes the shown take. The approved and custom images are
/// cleared, so the user chooses again (section 24.1).
pub fn record_scene_image(session: &mut Obj, index: usize, comfy_image: &Value, saved: &Path) -> io::Result<(usize, usize)> {
    let segment = segment_mut(session, index)?;
    let saved_text = saved.display().to_string();
    let mut list = history(segment);
    if !list.contains(&saved_text) {
        list.push(saved_text);
    }
    let shown = list.len() - 1;
    segment.insert("image".into(), comfy_image.clone());
    segment.insert("image_history".into(), json!(list));
    segment.insert("image_history_index".into(), json!(shown));
    for key in ["custom_image_path", "custom_image_data", "custom_image_name", "approved_image_path"] {
        segment.insert(key.into(), json!(""));
    }
    segment.insert("preview_mode".into(), json!("image"));
    Ok((shown, list.len()))
}

/// Make an image the scene's approved one: the path given, else the shown history take, else the last one, else the custom
/// image.
pub fn approve_scene_image(session: &mut Obj, index: usize, image_path: Option<&str>) -> io::Result<String> {
    let segment = segment_mut(session, index)?;
    let list = history(segment);
    let shown = segment.get("image_history_index").and_then(to_float).map(|f| f as i64).unwrap_or(-1);
    let target = match image_path.map(str::trim).filter(|p| !p.is_empty()) {
        Some(path) => path.to_string(),
        None if shown >= 0 && (shown as usize) < list.len() => list[shown as usize].clone(),
        None if !list.is_empty() => list.last().cloned().unwrap_or_default(),
        None => text_or_empty(segment.get("custom_image_path")),
    };
    if target.is_empty() || !Path::new(&target).is_file() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "No valid image path found to approve."));
    }
    let absolute = h3_core::abspath::absolute(&target).unwrap_or_else(|_| PathBuf::from(&target)).display().to_string();
    segment.insert("approved_image_path".into(), json!(absolute));
    segment.insert("preview_mode".into(), json!("image"));
    Ok(absolute)
}

/// Move the shown history take by `delta`, or to `index`. Returns the new index, the take and the take count.
pub fn revert_scene_image(session: &mut Obj, index: usize, delta: i64, to_index: Option<i64>) -> io::Result<(usize, String, usize)> {
    let segment = segment_mut(session, index)?;
    let list = history(segment);
    if list.is_empty() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "The scene has no image history to revert."));
    }
    let current = segment.get("image_history_index").and_then(to_float).map(|f| f as i64).unwrap_or(list.len() as i64 - 1);
    let last = list.len() as i64 - 1;
    let next = to_index.unwrap_or(current + delta).clamp(0, last) as usize;
    segment.insert("image_history_index".into(), json!(next));
    segment.insert("preview_mode".into(), json!("image"));
    Ok((next, list[next].clone(), list.len()))
}

/// Clear the scene's image. The files and the history stay.
pub fn clear_scene_image(session: &mut Obj, index: usize) -> io::Result<()> {
    let segment = segment_mut(session, index)?;
    segment.insert("approved_image_path".into(), json!(""));
    segment.insert("custom_image_path".into(), json!(""));
    segment.insert("image_assignment_cleared".into(), json!(true));
    Ok(())
}

// -- reference cards --------------------------------------------------------------------------------------------------------

/// The Builder's file name for a reference: letters, digits, `_`, `.`, `-` and spaces, the rest `_`.
fn safe_name(name: &str) -> String {
    let mut out = String::new();
    let mut last_bad = false;
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ' ' | '-') {
            out.push(c);
            last_bad = false;
        } else if !last_bad {
            out.push('_');
            last_bad = true;
        }
    }
    let out = out.trim_matches([' ', '.', '_']).to_string();
    if out.is_empty() { "reference".into() } else { out }
}

/// `data:image/jpeg;base64,...` of the image at most `max_height` high, the small preview a reference card shows.
pub fn preview_data_url(path: &Path, max_height: u32, quality: u8) -> io::Result<String> {
    let image = image::open(path).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?.to_rgb8();
    let image = if image.height() > max_height {
        let width = ((image.width() as f64) * (max_height as f64) / (image.height() as f64)).round().max(1.0) as u32;
        image::imageops::resize(&image, width, max_height, image::imageops::FilterType::Lanczos3)
    } else {
        image
    };
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, quality)
        .encode_image(&image)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    Ok(format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}

/// Save an image as a reference card's picture: a copy in `subject_location/subject` (or `location`) named after the card,
/// and the `{path, data, name}` object the Builder keeps on the card. `kind` is `subjects` or `locations`.
pub fn save_reference_image(project: &Path, kind: &str, card_name: &str, source: &Path) -> io::Result<Value> {
    let folder = match kind {
        "subjects" => "subject",
        "locations" => "location",
        _ => return Err(io::Error::new(io::ErrorKind::InvalidInput, "kind must be 'subjects' or 'locations'.")),
    };
    if !source.is_file() {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("Image file not found: {}", source.display())));
    }
    let dir = project.join("subject_location").join(folder);
    fs::create_dir_all(&dir)?;
    let ext = source.extension().map(|e| e.to_string_lossy().to_lowercase()).filter(|e| matches!(e.as_str(), "png" | "jpg" | "jpeg" | "webp" | "bmp")).unwrap_or_else(|| "png".into());
    let stem = safe_name(card_name);
    // A reference keeps its name. A new picture replaces the old file, which stays as a numbered copy.
    let target = dir.join(format!("{stem}.{ext}"));
    if target.exists() {
        let mut n = 2;
        loop {
            let backup = dir.join(format!("{stem}_previous_{n:02}.{ext}"));
            if !backup.exists() {
                fs::rename(&target, &backup)?;
                break;
            }
            n += 1;
        }
    }
    fs::copy(source, &target)?;
    let data = preview_data_url(&target, 220, 72).unwrap_or_default();
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(json!({"path": target.display().to_string(), "data": data, "name": name}))
}

/// Set the picture of the card with this id in the session's reference catalog. Returns the card's name.
pub fn set_reference_image(session: &mut Obj, kind: &str, card_id: &str, image: Value) -> io::Result<String> {
    let cards = session
        .get_mut("flux_reference_builder")
        .and_then(Value::as_object_mut)
        .and_then(|refs| refs.get_mut(kind))
        .and_then(Value::as_array_mut)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "The project has no reference builder."))?;
    let card = cards
        .iter_mut()
        .filter_map(Value::as_object_mut)
        .find(|c| c.get("id").and_then(Value::as_str) == Some(card_id))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("Reference '{card_id}' was not found.")))?;
    card.insert("image".into(), image);
    Ok(text_or_empty(card.get("name")))
}

/// The card with this id: its name and current picture path.
pub fn reference_card(session: &Obj, kind: &str, card_id: &str) -> Option<(String, String)> {
    session
        .get("flux_reference_builder")?
        .get(kind)?
        .as_array()?
        .iter()
        .filter_map(Value::as_object)
        .find(|c| c.get("id").and_then(Value::as_str) == Some(card_id))
        .map(|c| (text_or_empty(c.get("name")), text_or_empty(c.get("image").and_then(|i| i.get("path")))))
}
