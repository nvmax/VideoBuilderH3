//! Pictures a graph loads: copying one into ComfyUI's input folder and the blank placeholder. Port of
//! `_prepare_load_image_name` and `_ensure_placeholder_load_image` in `runner/api_graph.py`.

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;

use crate::env::GraphEnv;

const PLACEHOLDER_NAME: &str = "vrgdg_placeholder_i2i.png";
/// A one pixel PNG.
const PLACEHOLDER_BASE64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+ip1sAAAAASUVORK5CYII=";

fn millis() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis())
}

fn extension_of(name: &str) -> String {
    Path::new(name).extension().map(|e| format!(".{}", e.to_string_lossy().to_lowercase())).unwrap_or_default()
}

/// A copy of a picture in ComfyUI's input folder, from a file or from base64 text. Returns its name, or "" when neither was
/// given.
pub fn prepare_load_image<E: GraphEnv + ?Sized>(env: &E, path: &str, data: &str, name: &str) -> Result<String, String> {
    let input = env.input_dir();
    let raw_path = path.trim().trim_matches('"');
    if !raw_path.is_empty() {
        let source = std::path::absolute(Path::new(raw_path)).unwrap_or_else(|_| raw_path.into());
        if !source.is_file() {
            return Err(format!("Image-to-image source was not found: {}", source.display()));
        }
        let ext = Some(extension_of(raw_path)).filter(|e| !e.is_empty()).unwrap_or_else(|| ".png".into());
        let target = format!("vrgdg_i2i_{}{ext}", millis());
        fs::create_dir_all(&input).map_err(|e| format!("{input}: {e}"))?;
        fs::copy(&source, Path::new(&input).join(&target)).map_err(|e| e.to_string())?;
        return Ok(target);
    }
    let raw_data = data.trim();
    if raw_data.is_empty() {
        return Ok(String::new());
    }
    let (encoded, ext) = match raw_data.split_once(',') {
        Some((header, encoded)) if header.to_lowercase().starts_with("data:") => {
            let header = header.to_lowercase();
            let ext = if header.contains("jpeg") || header.contains("jpg") {
                ".jpg"
            } else if header.contains("webp") {
                ".webp"
            } else {
                ".png"
            };
            (encoded, ext.to_string())
        }
        _ => (raw_data, Some(extension_of(name)).filter(|e| !e.is_empty()).unwrap_or_else(|| ".png".into())),
    };
    let bytes = base64::engine::general_purpose::STANDARD.decode(encoded.trim()).map_err(|e| format!("Incorrect padding or bad image data: {e}"))?;
    let target = format!("vrgdg_i2i_{}{ext}", millis());
    fs::create_dir_all(&input).map_err(|e| format!("{input}: {e}"))?;
    fs::write(Path::new(&input).join(&target), bytes).map_err(|e| e.to_string())?;
    Ok(target)
}

/// The blank picture an unused `LoadImage` node points at. Written once into ComfyUI's input folder.
pub fn ensure_placeholder_image<E: GraphEnv + ?Sized>(env: &E) -> Result<String, String> {
    let input = env.input_dir();
    if input.is_empty() {
        return Err("ComfyUI's input folder is not known.".into());
    }
    fs::create_dir_all(&input).map_err(|e| format!("{input}: {e}"))?;
    let target = Path::new(&input).join(PLACEHOLDER_NAME);
    let valid = fs::read(&target).map(|b| b.starts_with(&[0x89, b'P', b'N', b'G'])).unwrap_or(false);
    if !valid {
        let bytes = base64::engine::general_purpose::STANDARD.decode(PLACEHOLDER_BASE64).map_err(|e| e.to_string())?;
        fs::write(&target, bytes).map_err(|e| e.to_string())?;
    }
    Ok(PLACEHOLDER_NAME.to_string())
}
