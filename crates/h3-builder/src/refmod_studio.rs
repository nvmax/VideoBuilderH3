//! RefMods Studio: the checks and the pictures behind creating a RefMod, the saved RefMods on disk, and describing reference pictures. Port of
//! `minimax/refmod_studio.py`, `minimax/refmod_library.py` (delete and preview) and `minimax/refmod_picker.py`.
//!
//! The extraction itself is ComfyUI-MiniMaxH3Mod's *Create H3 RefMod* node. The routes queue it. This module fixes the settings the Studio does
//! not expose, and prepares the pictures: cropped to the boxes the user chose (or kept whole), scaled down onto one canvas that holds every
//! picture without cutting any, and padded with each picture's own border colour. The canvas rule is mirrored by `canvasFor` in
//! `web/music_video_builder/refmod_trim.mjs`. Keep the two in step.

use std::path::{Path, PathBuf};

use h3_core::pyval::{round_half_even, text_or_empty, to_float, to_int, truthy, Obj};
use h3_llm::clean::strip_thinking_text;
use h3_pipeline::refmod_library::{find_refmod, PREVIEW_SUFFIX};
use image::imageops::FilterType;
use image::{DynamicImage, ImageDecoder, ImageReader, Rgb, RgbImage};
use serde_json::{json, Value};

use crate::prompt_writers::{Llm, VisionCall};

/// Types the Studio can create. The folder under models/refmods is always the type.
pub const ACTIVE_TYPES: [&str; 9] = ["identity", "clothing_men", "clothing_women", "background", "generic", "object", "prop", "vehicle", "creature"];
pub const IMAGE_EXTENSIONS: [&str; 6] = [".png", ".jpg", ".jpeg", ".webp", ".bmp", ".gif"];
pub const MAX_TOKENS: i64 = 5120;
const MIN_CROP_SIZE: i64 = 32;
/// The video VAE needs at least 320 px on both sides. The canvas is padded to this size instead of scaling a small picture up.
const MIN_CANVAS_SIDE: i64 = 320;
const CANVAS_LIMIT: f64 = 1024.0;
const QUALITY_SCALES: [(&str, f64); 5] = [("maximum", 1.0), ("high", 0.8), ("balanced", 0.6), ("compact", 0.45), ("draft", 0.3)];
const PREVIEW_MAX_SIDE: u32 = 384;
pub const MAX_UPLOAD_BYTES: usize = 64 * 1024 * 1024;

/// The settings users cannot change from the Studio, as the node's inputs.
pub fn fixed_settings() -> Obj {
    json!({
        "ref_resolution": 1024, "pool_h": 32, "pool_w": 32, "latent_frames": 16, "multiplier": 1, "max_tokens": MAX_TOKENS, "merge": false, "motion_only": false,
        "budget_policy": "truncate", "extraction_preset": "manual", "background_retention": 0.0,
    })
    .as_object()
    .cloned()
    .unwrap_or_default()
}

/// A create request after its checks.
#[derive(Debug, Clone)]
pub struct Request {
    pub concept_type: String,
    pub name: String,
    pub paths: Vec<String>,
    pub crops: Vec<Option<[i64; 4]>>,
    pub quality: String,
    /// `encode` (Full Reference) or `training` (Compressed Reference).
    pub mode: &'static str,
    pub steps: i64,
    pub description: String,
    pub overwrite: bool,
}

/// `_snap32`: round half up to a multiple of 32, at least 32.
fn snap32(value: f64) -> i64 {
    ((value / 32.0 + 0.5).floor() as i64 * 32).max(32)
}

/// `canvas_for`: one canvas that holds every `(width, height)` without cutting any. The widest width by the tallest height, each scaled by
/// `scale` (never up, never past the limit on the longest side), snapped to multiples of 32 and padded up to the smallest canvas.
pub fn canvas_for(sizes: &[(i64, i64)], scale: f64) -> (i64, i64, f64) {
    let widest = sizes.iter().map(|s| s.0).max().unwrap_or(1);
    let tallest = sizes.iter().map(|s| s.1).max().unwrap_or(1);
    let fit = 1.0f64.min(scale).min(CANVAS_LIMIT / widest.max(tallest) as f64);
    (MIN_CANVAS_SIDE.max(snap32(widest as f64 * fit)), MIN_CANVAS_SIDE.max(snap32(tallest as f64 * fit)), fit)
}

pub fn tokens_for_canvas(frames: i64, width: i64, height: i64) -> i64 {
    frames * (width / 32) * (height / 32)
}

fn quality_scale(name: &str) -> Option<f64> {
    QUALITY_SCALES.iter().find(|(n, _)| *n == name).map(|(_, s)| *s)
}

fn base_name(path: &str) -> String {
    Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string())
}

fn parse_crop(value: &Value) -> Result<Option<[i64; 4]>, String> {
    match value {
        Value::Null => Ok(None),
        Value::Array(items) if items.is_empty() => Ok(None),
        Value::Array(items) if items.len() == 4 => {
            let mut out = [0i64; 4];
            for (slot, item) in out.iter_mut().zip(items) {
                *slot = to_float(item).filter(|f| f.is_finite()).map(round_half_even).ok_or_else(|| "must be four numbers".to_string())?;
            }
            Ok(Some(out))
        }
        _ => Err("must be four numbers".into()),
    }
}

/// `validate_request`: check and normalise a create request. The messages are shown on the page.
pub fn validate_request(payload: &Obj) -> Result<Request, String> {
    let concept_type = text_or_empty(payload.get("type"));
    if !ACTIVE_TYPES.contains(&concept_type.as_str()) {
        return Err(format!("'{concept_type}' cannot be created yet. Choose one of: {}.", ACTIVE_TYPES.join(", ")));
    }
    let mut name = text_or_empty(payload.get("name")).trim().to_string();
    for bad in ['\\', '/', ':', '*', '?', '"', '<', '>', '|'] {
        name = name.replace(bad, "_");
    }
    let name = name.trim_matches([' ', '.']).to_string();
    if name.is_empty() {
        return Err("Enter a name for the RefMod.".into());
    }
    let paths: Vec<String> = match payload.get("paths") {
        Some(Value::Array(items)) => items.iter().map(|p| text_or_empty(Some(p))).collect(),
        _ => Vec::new(),
    };
    if paths.is_empty() {
        return Err("Add at least one image.".into());
    }
    for path in &paths {
        let extension = Path::new(path).extension().map(|e| format!(".{}", e.to_string_lossy().to_lowercase())).unwrap_or_default();
        if !IMAGE_EXTENSIONS.contains(&extension.as_str()) || !Path::new(path).is_file() {
            return Err(format!("Image not found or not a supported type: {path}"));
        }
    }
    let raw_crops: Vec<Value> = match payload.get("crops") {
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    };
    if !raw_crops.is_empty() && raw_crops.len() != paths.len() {
        return Err("Send one crop (or null) for every image.".into());
    }
    let mut crops: Vec<Option<[i64; 4]>> = Vec::new();
    if raw_crops.is_empty() {
        crops = vec![None; paths.len()];
    } else {
        for (value, path) in raw_crops.iter().zip(&paths) {
            crops.push(parse_crop(value).map_err(|_| format!("Crop for {} must be four numbers: x0, y0, x1, y1.", base_name(path)))?);
        }
    }
    let quality = payload.get("quality").filter(|v| truthy(v)).map(|v| text_or_empty(Some(v))).unwrap_or_else(|| "balanced".into());
    if quality_scale(&quality).is_none() {
        return Err(format!("Quality must be one of: {}.", QUALITY_SCALES.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ")));
    }
    let mode_label = payload.get("mode").filter(|v| truthy(v)).map(|v| text_or_empty(Some(v))).unwrap_or_else(|| "Full Reference".into());
    let mode = match mode_label.as_str() {
        "Full Reference" => "encode",
        "Compressed Reference" => "training",
        _ => return Err("Mode must be Full Reference or Compressed Reference.".into()),
    };
    let steps = match payload.get("steps") {
        None => 500,
        Some(value) => to_int(value).ok_or_else(|| "Refinement steps must be a whole number.".to_string())?,
    };
    if !(0..=2000).contains(&steps) {
        return Err("Refinement steps must be between 0 and 2000.".into());
    }
    Ok(Request {
        concept_type,
        name,
        paths,
        crops,
        quality,
        mode,
        steps,
        description: text_or_empty(payload.get("description")).trim().to_string(),
        overwrite: payload.get("overwrite").is_some_and(truthy),
    })
}

/// A picture file as RGB with its orientation applied (`ImageOps.exif_transpose`).
fn open_oriented(path: &str) -> Result<RgbImage, String> {
    let reader = ImageReader::open(path).map_err(|e| format!("{path}: {e}"))?.with_guessed_format().map_err(|e| format!("{path}: {e}"))?;
    let mut decoder = reader.into_decoder().map_err(|e| format!("{path}: {e}"))?;
    let orientation = decoder.orientation().ok();
    let mut picture = DynamicImage::from_decoder(decoder).map_err(|e| format!("{path}: {e}"))?;
    if let Some(orientation) = orientation {
        picture.apply_orientation(orientation);
    }
    Ok(picture.to_rgb8())
}

/// `_border_colour`: the median colour of the outer ring of a picture, used to pad it without a visible seam.
fn border_colour(image: &RgbImage) -> Rgb<u8> {
    let (width, height) = image.dimensions();
    let mut ring: Vec<[u8; 3]> = Vec::new();
    for x in 0..width {
        ring.push(image.get_pixel(x, 0).0);
    }
    for x in 0..width {
        ring.push(image.get_pixel(x, height - 1).0);
    }
    for y in 0..height {
        ring.push(image.get_pixel(0, y).0);
    }
    for y in 0..height {
        ring.push(image.get_pixel(width - 1, y).0);
    }
    let mut out = [0u8; 3];
    for (channel, slot) in out.iter_mut().enumerate() {
        let mut values: Vec<u8> = ring.iter().map(|p| p[channel]).collect();
        values.sort_unstable();
        let middle = values.len() / 2;
        // numpy's median averages the two middle values of an even count, and the pack then truncates to an integer.
        *slot = if values.len() % 2 == 1 { values[middle] } else { ((u16::from(values[middle - 1]) + u16::from(values[middle])) / 2) as u8 };
    }
    Rgb(out)
}

fn check_crop(crop: Option<[i64; 4]>, width: u32, height: u32, path: &str) -> Result<Option<(u32, u32, u32, u32)>, String> {
    let Some([x0, y0, x1, y1]) = crop else { return Ok(None) };
    let (x0, y0, x1, y1) = (x0.max(0), y0.max(0), x1.min(i64::from(width)), y1.min(i64::from(height)));
    if x1 - x0 < MIN_CROP_SIZE || y1 - y0 < MIN_CROP_SIZE {
        return Err(format!("Crop for {} is smaller than {MIN_CROP_SIZE} px.", base_name(path)));
    }
    Ok(Some((x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32)))
}

/// `prepare_images`: crop, scale and pad the pictures onto one shared canvas. Returns the canvases and `(width, height)`.
pub fn prepare_images(paths: &[String], crops: &[Option<[i64; 4]>], quality: &str) -> Result<(Vec<RgbImage>, (u32, u32)), String> {
    let mut images: Vec<RgbImage> = Vec::new();
    for (path, crop) in paths.iter().zip(crops) {
        let picture = open_oriented(path)?;
        images.push(match check_crop(*crop, picture.width(), picture.height(), path)? {
            Some((x, y, w, h)) => image::imageops::crop_imm(&picture, x, y, w, h).to_image(),
            None => picture,
        });
    }
    let sizes: Vec<(i64, i64)> = images.iter().map(|i| (i64::from(i.width()), i64::from(i.height()))).collect();
    let (canvas_width, canvas_height, fit) = canvas_for(&sizes, quality_scale(quality).unwrap_or(0.6));
    let mut canvases = Vec::new();
    for image in images {
        let scale = fit.min(canvas_width as f64 / f64::from(image.width())).min(canvas_height as f64 / f64::from(image.height()));
        let image = if scale < 1.0 {
            let (w, h) = (round_half_even(f64::from(image.width()) * scale).max(1) as u32, round_half_even(f64::from(image.height()) * scale).max(1) as u32);
            image::imageops::resize(&image, w, h, FilterType::Lanczos3)
        } else {
            image
        };
        let mut canvas = RgbImage::from_pixel(canvas_width as u32, canvas_height as u32, border_colour(&image));
        image::imageops::replace(&mut canvas, &image, (i64::from(canvas_width as u32) - i64::from(image.width())) / 2, (i64::from(canvas_height as u32) - i64::from(image.height())) / 2);
        canvases.push(canvas);
    }
    Ok((canvases, (canvas_width as u32, canvas_height as u32)))
}

/// `save_preview`: a picture as a preview PNG no larger than 384 pixels on its longest side.
pub fn save_preview(image: &RgbImage, destination: &Path) -> Result<(), String> {
    let longest = image.width().max(image.height());
    let scaled;
    let preview: &RgbImage = if longest > PREVIEW_MAX_SIDE {
        let scale = f64::from(PREVIEW_MAX_SIDE) / f64::from(longest);
        scaled = image::imageops::resize(image, ((f64::from(image.width()) * scale).round() as u32).max(1), ((f64::from(image.height()) * scale).round() as u32).max(1), FilterType::Lanczos3);
        &scaled
    } else {
        image
    };
    preview.save_with_format(destination, image::ImageFormat::Png).map_err(|e| format!("{}: {e}", destination.display()))
}

/// The pictures at `paths` as RGB.
pub fn load_pictures(paths: &[String]) -> Result<Vec<DynamicImage>, String> {
    paths.iter().map(|p| image::open(p).map(|i| DynamicImage::ImageRgb8(i.to_rgb8())).map_err(|e| format!("{p}: {e}"))).collect()
}

/// A picture as PNG bytes.
pub fn png_bytes(image: &RgbImage) -> Result<Vec<u8>, String> {
    let mut png = std::io::Cursor::new(Vec::new());
    image.write_to(&mut png, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(png.into_inner())
}

/// The preview file of a RefMod, or nothing for an unknown name.
pub fn preview_path(dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    find_refmod(dirs, name).map(|entry| entry.path.with_extension("").with_extension(&PREVIEW_SUFFIX[1..]))
}

/// `delete_refmod`: delete one RefMod file and its preview. The pack's own `ComfyUI-MiniMaxH3Mod/mods` folder is not the user's library, so
/// RefMods there are never deleted.
pub fn delete_refmod(dirs: &[PathBuf], name: &str) -> Result<Value, String> {
    let entry = find_refmod(dirs, name).ok_or_else(|| format!("No RefMod named '{name}'."))?;
    if entry.directory.parent().and_then(|p| p.file_name()).is_some_and(|n| n == "ComfyUI-MiniMaxH3Mod") {
        return Err(format!("'{}' belongs to the MiniMaxH3Mod pack and cannot be deleted here.", entry.name));
    }
    let preview = preview_path(dirs, name);
    std::fs::remove_file(&entry.path).map_err(|e| format!("{}: {e}", entry.path.display()))?;
    if let Some(preview) = preview.filter(|p| p.is_file()) {
        let _ = std::fs::remove_file(preview);
    }
    eprintln!("[VRGDG-RS RefMod] Deleted {}", entry.name);
    Ok(json!({"name": entry.name}))
}

/// `_save_upload`: write an uploaded picture to the Studio's temp folder and return its path.
pub fn save_upload(folder: &Path, filename: &str, data: &[u8], unique: &str) -> Result<PathBuf, String> {
    let file_name = Path::new(filename).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "image".into());
    let (stem, extension) = match file_name.rsplit_once('.') {
        Some((stem, ext)) => (stem.to_string(), format!(".{}", ext.to_lowercase())),
        None => (file_name.clone(), String::new()),
    };
    if !IMAGE_EXTENSIONS.contains(&extension.as_str()) {
        return Err(format!("Unsupported image type '{}'.", if extension.is_empty() { filename } else { &extension }));
    }
    let safe: String = stem.chars().map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | ' ' | '.') { c } else { '_' }).collect();
    let safe = safe.trim().to_string();
    let safe = if safe.is_empty() { "image".to_string() } else { safe };
    std::fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    let path = folder.join(format!("{unique}_{safe}{extension}"));
    std::fs::write(&path, data).map_err(|e| format!("{}: {e}", path.display()))?;
    if image::open(&path).is_err() {
        let _ = std::fs::remove_file(&path);
        return Err(format!("'{filename}' is not a readable image."));
    }
    Ok(path)
}

/// `collect_image_paths`: the pictures of a Reference Builder save: the card's own picture first, then any `extra_images`. Each is
/// `{"path": ...}` or `{"data": <base64>, "name": ...}`. Pictures sent as data are written by `save`.
pub fn collect_image_paths(payload: &Obj, save: &dyn Fn(&str, &str) -> Result<String, String>) -> Result<Vec<String>, String> {
    let mut images: Vec<Obj> = vec![payload.get("image").and_then(Value::as_object).cloned().unwrap_or_default()];
    if let Some(Value::Array(extras)) = payload.get("extra_images") {
        images.extend(extras.iter().filter_map(Value::as_object).cloned());
    }
    let mut paths = Vec::new();
    for image in images {
        let mut path = text_or_empty(image.get("path")).trim().to_string();
        let data = text_or_empty(image.get("data")).trim().to_string();
        if path.is_empty() && !data.is_empty() {
            let name = image.get("name").filter(|v| truthy(v)).map(|v| text_or_empty(Some(v))).unwrap_or_else(|| "reference.png".into());
            path = save(&data, &name)?;
        }
        if !path.is_empty() {
            paths.push(path);
        }
    }
    Ok(paths)
}

/// `pick_image_files`: the native multi-select file dialog. The chosen picture paths, none when the dialog is cancelled.
pub fn pick_image_files() -> Result<Vec<String>, String> {
    if !cfg!(windows) {
        return Err("Native file dialog is not available on this system. Type the path instead.".into());
    }
    let script = "Add-Type -AssemblyName System.Windows.Forms\n$dialog = New-Object System.Windows.Forms.OpenFileDialog\n$dialog.Title = 'Choose reference images'\n$dialog.Multiselect = $true\n$dialog.Filter = 'Image files (*.png;*.jpg;*.jpeg;*.webp;*.bmp;*.gif)|*.png;*.jpg;*.jpeg;*.webp;*.bmp;*.gif|All files (*.*)|*.*'\nif ($dialog.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) { [Console]::Write(($dialog.FileNames -join \"`n\")) }";
    let mut command = std::process::Command::new("powershell");
    command.args(["-NoProfile", "-STA", "-Command", script]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let output = command.output().map_err(|e| format!("Native file dialog is not available: {e}"))?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(if output.stderr.is_empty() { &output.stdout } else { &output.stderr }).trim().to_string();
        return Err(if message.is_empty() { "PowerShell picker failed.".into() } else { message });
    }
    Ok(String::from_utf8_lossy(&output.stdout).lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect())
}

// -- describing pictures -----------------------------------------------------------------------------------------------------

const ENDING: &str = " Output only the description as plain text, one paragraph, no preamble.";

/// `DESCRIBE_INSTRUCTIONS`: what the vision model is asked for each kind of RefMod.
pub fn describe_instructions() -> Vec<(&'static str, String)> {
    let clothing = format!("These images all show the same outfit or garment. Write one concise description of the clothing only: garment types, cut and fit, colours, patterns, fabrics and materials, trims, logos, footwear, jewelry and other accessories. If a person is wearing it, do not describe the person: no face, hair, skin, body, age or pose. If an image shows only the clothing, describe it directly. Ignore backgrounds, lighting and camera angle.{ENDING}");
    vec![
        ("identity", format!("These images all show the same character or subject. Write one concise description of the subject that stays true across every image: age and build, face and hair, skin or fur colour, clothing and accessories, and any distinctive marks. Ignore backgrounds, poses, lighting and camera angle.{ENDING}")),
        ("clothing", clothing.clone()),
        ("background", format!("These images all show the same place or environment. Write one concise description of the setting: the type of location, architecture or landscape features, layout, key objects and props, materials and textures, colours, time of day, weather and atmosphere. Do not describe people or characters. Ignore camera angle.{ENDING}")),
        ("style", format!("These images all share one visual style. Write one concise description of the style itself, not of what the images show: the medium and technique (for example cartoon, 3D render, watercolor, pencil sketch, photograph, pixel art, black and white), colour palette, line quality, shading and texture, lighting mood, level of detail, and the era or genre it belongs to. Word it so it could be applied to any subject, and do not name the specific characters, objects or places in the images.{ENDING}")),
        ("pose_motion", format!("These images all show the same pose, movement or camera move. Write one concise description of the pose or motion only: body position, limb placement, direction and flow of movement, gesture, and any camera movement or framing. Do not describe who the person is, their clothing, or the background.{ENDING}")),
        ("object", format!("These images all show the same object. Write one concise description of the object only: what it is, its shape and proportions, materials, colours, surface finish, markings, logos, wear and any distinctive details. Do not describe who is holding it, the background or the lighting.{ENDING}")),
        ("prop", format!("These images all show the same prop or set-dressing item. Write one concise description of the prop only: what it is, its shape, size, materials, colours, condition and distinctive details. Do not describe people, the background or the lighting.{ENDING}")),
        ("vehicle", format!("These images all show the same vehicle. Write one concise description of the vehicle only: make or type, body shape, paint colours and finish, wheels, lights, trim, decals, modifications, damage and wear. Do not describe the driver, the road or the background.{ENDING}")),
        ("creature", format!("These images all show the same creature or animal. Write one concise description of the creature only: species or type, size and build, body shape, skin, fur or scales, colours and patterns, eyes, limbs, distinctive features and any gear it wears. Ignore the background, pose and lighting.{ENDING}")),
        ("generic", format!("These images all show the same subject or idea. Write one concise description of what they have in common: the main subject, its key visual features, colours and materials, and the overall look.{ENDING}")),
        ("clothing_men", clothing.clone()),
        ("clothing_women", clothing),
    ]
}

/// `_pick_images_to_describe`: up to `max_images` pictures, spread across the list. Identity always keeps its first three close-ups.
fn pick_images(images: &[DynamicImage], concept_type: &str, max_images: usize) -> Vec<DynamicImage> {
    let count = max_images.min(images.len());
    let lead: Vec<DynamicImage> = if concept_type == "identity" && count >= 3 { images[..3].to_vec() } else { Vec::new() };
    let rest = &images[lead.len()..];
    let room = count - lead.len();
    if rest.is_empty() {
        return lead;
    }
    let step = if room > 0 { (rest.len() as f64 / room as f64).max(1.0) } else { 1.0 };
    let mut chosen = lead;
    for i in 0..room {
        chosen.push(rest[((i as f64 * step) as usize).min(rest.len() - 1)].clone());
    }
    chosen
}

fn downscale(picture: &DynamicImage, edge: u32) -> DynamicImage {
    let longest = picture.width().max(picture.height());
    if longest <= edge {
        return picture.clone();
    }
    let scale = f64::from(edge) / f64::from(longest);
    picture.resize_exact(((f64::from(picture.width()) * scale).round() as u32).max(1), ((f64::from(picture.height()) * scale).round() as u32).max(1), FilterType::Lanczos3)
}

/// `describe_images`: describe pictures with the vision model of the LLM runner the Video Builder has selected. The question depends on
/// `concept_type`. This never loads or switches LM Studio models.
pub async fn describe_images(llm: &dyn Llm, images: &[DynamicImage], concept_type: &str, instruction: &str, name_hint: &str, max_images: usize, image_edge: u32) -> Result<String, String> {
    if images.is_empty() {
        return Err("[VRGDG RefMod] No reference images to describe.".into());
    }
    let table = describe_instructions();
    let Some((_, base)) = table.iter().find(|(key, _)| *key == concept_type) else {
        return Err(format!("[VRGDG RefMod] Describing '{concept_type}' is not available."));
    };
    let chosen = pick_images(images, concept_type, max_images);
    let mut text = if instruction.trim().is_empty() { base.clone() } else { instruction.trim().to_string() };
    if !name_hint.trim().is_empty() {
        text.push_str(&format!("\nThe subject is: {}.", name_hint.trim()));
    }
    let pictures: Vec<DynamicImage> = chosen.iter().map(|p| downscale(p, image_edge)).collect();
    let (reply, _) = llm.vision(VisionCall { instruction: text, images: pictures, temperature: 0.25, top_p: 0.95, max_new_tokens: 400 }).await?;
    let description = strip_thinking_text(&reply);
    if description.is_empty() {
        return Err("[VRGDG RefMod] The vision model returned an empty description.".into());
    }
    Ok(description)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(value: Value) -> Obj {
        value.as_object().cloned().unwrap()
    }

    #[test]
    fn the_canvas_holds_every_picture() {
        assert_eq!(canvas_for(&[(2000, 1000), (800, 1600)], 1.0), (1024, 832, 0.512));
        assert_eq!(canvas_for(&[(100, 100)], 0.6), (320, 320, 0.6));
        assert_eq!(canvas_for(&[(1000, 500)], 0.6).0, 608);
        assert_eq!(tokens_for_canvas(3, 640, 480), 3 * 20 * 15);
    }

    #[test]
    fn a_request_is_checked_like_the_packs() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.png");
        RgbImage::new(64, 64).save(&file).unwrap();
        let path = file.display().to_string();
        let ok = validate_request(&payload(json!({"type": "identity", "name": " a/b:c. ", "paths": [path], "mode": "Compressed Reference", "steps": "250"}))).unwrap();
        assert_eq!((ok.name.as_str(), ok.mode, ok.steps, ok.quality.as_str()), ("a_b_c", "training", 250, "balanced"));
        let message = |v: Value| validate_request(&payload(v)).unwrap_err();
        assert_eq!(message(json!({"type": "style"})), "'style' cannot be created yet. Choose one of: identity, clothing_men, clothing_women, background, generic, object, prop, vehicle, creature.");
        assert_eq!(message(json!({"type": "prop", "name": "  "})), "Enter a name for the RefMod.");
        assert_eq!(message(json!({"type": "prop", "name": "x"})), "Add at least one image.");
        assert!(message(json!({"type": "prop", "name": "x", "paths": ["Z:/nope.png"]})).starts_with("Image not found or not a supported type"));
        assert_eq!(message(json!({"type": "prop", "name": "x", "paths": [path], "crops": [null, null]})), "Send one crop (or null) for every image.");
        assert_eq!(message(json!({"type": "prop", "name": "x", "paths": [path], "quality": "ultra"})), "Quality must be one of: maximum, high, balanced, compact, draft.");
        assert_eq!(message(json!({"type": "prop", "name": "x", "paths": [path], "mode": "x"})), "Mode must be Full Reference or Compressed Reference.");
        assert_eq!(message(json!({"type": "prop", "name": "x", "paths": [path], "steps": 5000})), "Refinement steps must be between 0 and 2000.");
        assert_eq!(message(json!({"type": "prop", "name": "x", "paths": [path], "steps": "x"})), "Refinement steps must be a whole number.");
    }

    #[test]
    fn pictures_are_cropped_scaled_and_padded() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.png"), dir.path().join("b.png"));
        RgbImage::from_pixel(800, 400, Rgb([200, 30, 30])).save(&a).unwrap();
        RgbImage::from_pixel(300, 600, Rgb([30, 30, 200])).save(&b).unwrap();
        let paths = vec![a.display().to_string(), b.display().to_string()];
        let (canvases, size) = prepare_images(&paths, &[None, Some([0, 0, 300, 300])], "maximum").unwrap();
        assert_eq!(size, (800, 416));
        assert!(canvases.iter().all(|c| c.dimensions() == (800, 416)));
        assert_eq!(canvases[0].get_pixel(400, 208).0, [200, 30, 30]);
        assert_eq!(canvases[1].get_pixel(0, 0).0, [30, 30, 200]);
        let err = prepare_images(&paths, &[Some([0, 0, 10, 10]), None], "maximum").unwrap_err();
        assert_eq!(err, "Crop for a.png is smaller than 32 px.");
    }

    #[tokio::test]
    async fn pictures_are_described_with_the_question_of_their_type() {
        use crate::prompt_writers::testing::{Asked, Canned};
        let pictures: Vec<DynamicImage> = (0..4).map(|_| DynamicImage::ImageRgb8(RgbImage::from_pixel(1600, 900, Rgb([9, 9, 9])))).collect();
        let llm = Canned::new(&["<think>hm</think>A tall woman with red hair."]);
        let text = describe_images(&llm, &pictures, "clothing_women", "", " Ann ", 2, 768).await.unwrap();
        assert_eq!(text, "A tall woman with red hair.");
        match &llm.asked.lock().unwrap()[0] {
            Asked::Vision { instruction, sizes, temperature, top_p, max_new_tokens } => {
                assert!(instruction.starts_with("These images all show the same outfit or garment."));
                assert!(instruction.ends_with("
The subject is: Ann."));
                assert_eq!((sizes.clone(), *temperature, *top_p, *max_new_tokens), (vec![(768, 432), (768, 432)], 0.25, 0.95, 400));
            }
            other => panic!("{other:?}"),
        }
        let err = describe_images(&Canned::new(&[]), &pictures, "style", "", "", 6, 768).await.unwrap_err();
        assert_eq!(err, "The recorder ran out of canned replies.");
        assert_eq!(describe_images(&Canned::new(&[]), &pictures, "pose", "", "", 6, 768).await.unwrap_err(), "[VRGDG RefMod] Describing 'pose' is not available.");
        assert_eq!(describe_images(&Canned::new(&[]), &[], "style", "", "", 6, 768).await.unwrap_err(), "[VRGDG RefMod] No reference images to describe.");
    }

    #[test]
    fn identity_keeps_its_first_close_ups_when_describing() {
        let pictures: Vec<DynamicImage> = (0..10).map(|i| DynamicImage::ImageRgb8(RgbImage::from_pixel(4, 4, Rgb([i * 20, 0, 0])))).collect();
        let chosen = pick_images(&pictures, "identity", 6);
        let reds: Vec<u8> = chosen.iter().map(|p| p.to_rgb8().get_pixel(0, 0).0[0]).collect();
        assert_eq!(reds, vec![0, 20, 40, 60, 100, 140]);
        assert_eq!(pick_images(&pictures[..2], "identity", 6).len(), 2);
    }
}
