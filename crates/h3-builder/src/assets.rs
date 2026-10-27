//! Scene pictures, reference pictures and audio files the Builder saves into a project. Ports of `_save_scene_image`,
//! `_archive_scene_image`, `_extract_video_final_frame_as_scene_image`, `_save_flux_reference_image`, the reference
//! imports (`builder/media.py`) and the audio and SRT savers (`builder/audio.py`, `builder/audio_clips.py`).

use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine;
use h3_core::pyval::{text_or_empty, to_float, to_int, truthy, Obj};
use h3_media::beats::estimate_beats;
use h3_media::peaks::read_audio_peaks;
use h3_project::atomic::atomic_write_text;
use h3_project::derived::{safe_project_name, segments_to_srt, format_srt_time};
use h3_project::images::{preview_data_url, scene_preview_folder, unique_preview_path};
use h3_project::layout::{images_folder, srt_path, Stamp};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::files::{parse_srt_segments, resolve_existing_file};
use crate::payload::path_text;
use crate::runner::{resolve_comfy_image, ComfyFolders};
use crate::Env;

type Reply = Result<Value, String>;

const IMAGE_EXTENSIONS: [&str; 4] = [".png", ".jpg", ".jpeg", ".webp"];

fn text(path: &Path) -> String {
    path.display().to_string()
}

fn absolute(path: &Path) -> PathBuf {
    h3_core::abspath::absolute(path).unwrap_or_else(|_| path.to_path_buf()).components().collect()
}

fn project_of(payload: &Obj) -> Result<PathBuf, String> {
    let raw = path_text(payload, "project_folder");
    if raw.is_empty() {
        return Err("Project folder is empty.".into());
    }
    Ok(absolute(Path::new(&raw)))
}

fn io(path: &Path, error: impl std::fmt::Display) -> String {
    format!("{}: {error}", path.display())
}

/// `int(payload.get(key) or default)`.
fn int_or(payload: &Obj, key: &str, default: i64) -> Result<i64, String> {
    match payload.get(key).filter(|v| truthy(v)) {
        None => Ok(default),
        Some(v) => to_int(v).ok_or_else(|| format!("invalid literal for int() with base 10: {v}")),
    }
}

fn float_or(payload: &Obj, key: &str, default: f64) -> f64 {
    payload.get(key).filter(|v| truthy(v)).and_then(to_float).unwrap_or(default)
}

fn extension(path: &Path) -> String {
    path.extension().map(|e| format!(".{}", e.to_string_lossy().to_lowercase())).unwrap_or_default()
}

/// A random 32 digit hex name part, like `uuid.uuid4().hex`.
fn random_hex() -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let seed = format!("{nanos}:{}:{}", std::process::id(), COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    let digest = Sha256::digest(seed.as_bytes());
    digest.iter().take(16).map(|b| format!("{b:02x}")).collect()
}

/// `_image_from_data_url` then PNG: a picture sent as a `data:image/...;base64,` URL, decoded and saved as an RGB PNG.
fn png_from_data_url(data: &str) -> Result<Vec<u8>, String> {
    let trimmed = data.trim();
    let lower = trimmed.to_lowercase();
    let encoded = ["data:image/png;base64,", "data:image/jpeg;base64,", "data:image/jpg;base64,", "data:image/webp;base64,"]
        .iter()
        .find(|prefix| lower.starts_with(*prefix))
        .map(|prefix| &trimmed[prefix.len()..])
        .ok_or("Expected a base64 image data URL.")?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(encoded.trim()).map_err(|e| format!("Incorrect padding or bad image data: {e}"))?;
    let picture = image::load_from_memory(&bytes).map_err(|e| format!("cannot identify image file: {e}"))?.to_rgb8();
    let mut out = std::io::Cursor::new(Vec::new());
    picture.write_to(&mut out, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

/// The picture a request names: ComfyUI's history entry, or a file.
fn source_image(payload: &Obj, comfy: Option<&ComfyFolders>, label: &str) -> Result<PathBuf, String> {
    match payload.get("image").and_then(Value::as_object) {
        Some(info) => resolve_comfy_image(comfy.ok_or("ComfyUI's folders are not known. Is ComfyUI running?")?, info),
        None => resolve_existing_file(&text_or_empty(payload.get("source_path")), label),
    }
}

fn image_data(payload: &Obj) -> String {
    text_or_empty(payload.get("image_data")).trim().to_string()
}

/// `POST .../save_scene_image`: the scene's approved picture, `zimage_approved/image_NNNN.<ext>`.
pub fn save_scene_image(payload: &Obj, comfy: Option<&ComfyFolders>) -> Reply {
    let project = project_of(payload)?;
    let folder = images_folder(&project);
    fs::create_dir_all(&folder).map_err(|e| io(&folder, e))?;
    let scene = int_or(payload, "scene_number", 1)?.max(1);
    let target = if !image_data(payload).is_empty() {
        let target = folder.join(format!("image_{scene:04}.png"));
        fs::write(&target, png_from_data_url(&image_data(payload))?).map_err(|e| io(&target, e))?;
        target
    } else {
        let source = source_image(payload, comfy, "Image file")?;
        let ext = Some(extension(&source)).filter(|e| IMAGE_EXTENSIONS.contains(&e.as_str())).unwrap_or_else(|| ".png".into());
        let target = folder.join(format!("image_{scene:04}{ext}"));
        fs::copy(&source, &target).map_err(|e| io(&target, e))?;
        target
    };
    Ok(json!({"saved_path": text(&target), "images_folder": text(&folder), "scene_number": scene}))
}

/// `POST .../archive_scene_image`: keep a picture in the scene's preview history.
pub fn archive_scene_image(payload: &Obj, comfy: Option<&ComfyFolders>, now: &Stamp) -> Reply {
    let project = project_of(payload)?;
    let scene = int_or(payload, "scene_number", 1)?.max(1) as u32;
    let target = if !image_data(payload).is_empty() {
        let target = unique_preview_path(&project, scene, ".png", now).map_err(|e| e.to_string())?;
        fs::write(&target, png_from_data_url(&image_data(payload))?).map_err(|e| io(&target, e))?;
        target
    } else {
        let source = source_image(payload, comfy, "Image file")?;
        let ext = Some(extension(&source)).filter(|e| !e.is_empty()).unwrap_or_else(|| ".png".into());
        let target = unique_preview_path(&project, scene, &ext, now).map_err(|e| e.to_string())?;
        fs::copy(&source, &target).map_err(|e| io(&target, e))?;
        target
    };
    Ok(json!({"saved_path": text(&target), "preview_folder": text(&scene_preview_folder(&project, scene)), "scene_number": scene}))
}

fn inside(parent: &Path, child: &Path) -> bool {
    let norm = |p: &Path| {
        let t = absolute(p).display().to_string();
        if cfg!(windows) { t.replace('/', "\\").to_lowercase() } else { t }
    };
    PathBuf::from(norm(child)).starts_with(norm(parent))
}

/// `POST .../delete_project_media`: delete one file of the project.
pub fn delete_project_media(payload: &Obj) -> Reply {
    let project = project_of(payload)?;
    let raw = path_text(payload, "path");
    if raw.is_empty() {
        return Err("Media path is empty.".into());
    }
    let media = absolute(Path::new(&raw));
    if !media.is_file() {
        return Ok(json!({"deleted": false, "path": text(&media), "reason": "File was already missing."}));
    }
    if !inside(&project, &media) {
        return Err("This file is outside the current project folder, so it was not deleted.".into());
    }
    fs::remove_file(&media).map_err(|e| io(&media, e))?;
    Ok(json!({"deleted": true, "path": text(&media)}))
}

/// `POST .../extract_video_final_frame`: the last picture of a project video, as a new take for a scene.
pub fn extract_video_final_frame(env: &Env, payload: &Obj, now: &Stamp) -> Reply {
    let project = project_of(payload)?;
    let source = resolve_existing_file(&text_or_empty(payload.get("source_path")), "Source video")?;
    if !inside(&project, &source) {
        return Err("Source video must be inside the current project folder.".into());
    }
    let wanted = payload.get("scene_number").filter(|v| truthy(v)).or_else(|| payload.get("target_scene_number").filter(|v| truthy(v)));
    let scene = wanted.and_then(to_int).unwrap_or(1).max(1) as u32;
    let tools = env.tools()?;
    let target = unique_preview_path(&project, scene, ".png", now).map_err(|e| e.to_string())?;
    let mut last_error = String::new();
    for seek in ["-0.04", "-0.12", "-0.5"] {
        let args: Vec<String> = ["-y", "-sseof", seek, "-i", &text(&source), "-frames:v", "1", "-update", "1", &text(&target)].iter().map(|s| s.to_string()).collect();
        match tools.ffmpeg_checked(&args, "ffmpeg could not extract a final frame.") {
            Ok(_) if fs::metadata(&target).is_ok_and(|m| m.len() > 0) => {
                return Ok(json!({"saved_path": text(&target), "preview_folder": text(&scene_preview_folder(&project, scene)), "scene_number": scene, "source_path": text(&source)}));
            }
            Ok(_) => last_error = "ffmpeg could not extract a final frame.".into(),
            Err(e) => last_error = e.to_string(),
        }
        let _ = fs::remove_file(&target);
    }
    Err(if last_error.is_empty() { "ffmpeg could not extract a final frame.".into() } else { last_error })
}

/// `_unique_file_path`: the path, or `<name>_02.<ext>` and up when it is taken.
fn unique_file_path(path: &Path) -> Result<PathBuf, String> {
    let folder = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(folder).map_err(|e| io(folder, e))?;
    if !path.exists() {
        return Ok(path.to_path_buf());
    }
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (2..).map(|i| folder.join(format!("{stem}_{i:02}{ext}"))).find(|p| !p.exists()).ok_or_else(|| "No free file name.".into())
}

/// `POST .../save_flux_reference_image`: a reference picture in `project_context/flux_references/<kind>s`.
pub fn save_flux_reference_image(payload: &Obj, comfy: Option<&ComfyFolders>) -> Reply {
    let project = project_of(payload)?;
    let kind = match text_or_empty(payload.get("reference_type")).trim().to_lowercase().as_str() {
        k @ ("subject" | "location" | "ingredients_sheet") => k.to_string(),
        _ => "location".to_string(),
    };
    let raw_name = text_or_empty(payload.get("name")).trim().to_string();
    let name = safe_project_name(&if raw_name.is_empty() { kind.clone() } else { raw_name });
    let folder_name = if kind == "ingredients_sheet" { "ingredients_sheets".to_string() } else { format!("{kind}s") };
    let target_dir = h3_project::layout::context_folder(&project).join("flux_references").join(folder_name);
    fs::create_dir_all(&target_dir).map_err(|e| io(&target_dir, e))?;
    let target = if !image_data(payload).is_empty() {
        let target = unique_file_path(&target_dir.join(format!("{name}.png")))?;
        fs::write(&target, png_from_data_url(&image_data(payload))?).map_err(|e| io(&target, e))?;
        target
    } else {
        let source = source_image(payload, comfy, "Reference image")?;
        let ext = Some(extension(&source)).filter(|e| !e.is_empty()).unwrap_or_else(|| ".png".into());
        let target = unique_file_path(&target_dir.join(format!("{name}{ext}")))?;
        fs::copy(&source, &target).map_err(|e| io(&target, e))?;
        target
    };
    Ok(json!({"saved_path": text(&target), "reference_type": kind, "folder": text(&target_dir)}))
}

/// `POST .../import_reference_subjects` and `.../import_reference_locations`: the pictures and descriptions in the
/// project's `subject_location` folders as Reference Builder cards.
pub fn import_references(payload: &Obj, kind: &str) -> Reply {
    let raw = path_text(payload, "project_folder");
    let project = absolute(Path::new(&raw));
    let (singular, plural, prefix) = if kind == "subject" { ("subject", "subjects", "subj_import") } else { ("location", "locations", "loc_import") };
    if raw.is_empty() || !project.is_dir() {
        return Err(format!("Create or load a project first so the {singular} folder can be found."));
    }
    let base = project.join("subject_location");
    let mut folder = base.join(singular);
    if kind != "subject" && !folder.is_dir() && base.join("locaton").is_dir() {
        folder = base.join("locaton");
    }
    if !folder.is_dir() {
        return Err(if kind == "subject" {
            format!("Subject folder does not exist:\n{}", folder.display())
        } else {
            format!("Location folder does not exist:\n{}\n\nExpected folder layout:\nsubject_location/location", base.join("location").display())
        });
    }
    let mut files: Vec<_> = fs::read_dir(&folder).map_err(|e| io(&folder, e))?.flatten().collect();
    files.sort_by_key(|e| e.file_name().to_string_lossy().to_lowercase());
    let mut cards: Vec<Value> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    for entry in files {
        let path = entry.path();
        let filename = entry.file_name().to_string_lossy().to_string();
        let ext = extension(&path);
        if !path.is_file() || ![".png", ".jpg", ".jpeg", ".webp", ".bmp"].contains(&ext.as_str()) {
            continue;
        }
        let stem = Path::new(&filename).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let text_path = folder.join(format!("{stem}.txt"));
        let description = if text_path.is_file() {
            String::from_utf8_lossy(&fs::read(&text_path).unwrap_or_default()).trim().to_string()
        } else {
            missing.push(format!("{stem}.txt"));
            String::new()
        };
        let preview = preview_data_url(&path, 220, 72).unwrap_or_default();
        let safe_id: String = {
            let mut out = String::new();
            let mut in_run = false;
            for c in stem.chars() {
                if c.is_ascii_alphanumeric() || c == '_' {
                    out.push(c);
                    in_run = false;
                } else if !in_run {
                    out.push('_');
                    in_run = true;
                }
            }
            out.trim_matches('_').to_string()
        };
        let safe_id = if safe_id.is_empty() { format!("{singular}_{}", cards.len() + 1) } else { safe_id };
        cards.push(json!({
            "id": format!("{prefix}_{}_{safe_id}", cards.len() + 1),
            "name": stem,
            "description": description,
            "image": {"path": text(&path), "data": preview, "name": filename},
        }));
    }
    if cards.is_empty() {
        return Err(format!("No {singular} images were found in:\n{}", folder.display()));
    }
    Ok(json!({"folder": text(&folder), plural: cards, "missing_descriptions": missing}))
}

// -- audio --------------------------------------------------------------------------------------------------------------

/// `_audio_bytes_from_data_url`.
fn audio_bytes(data: &str) -> Result<Vec<u8>, String> {
    let raw = data.trim();
    if raw.is_empty() {
        return Err("Audio data is empty.".into());
    }
    let encoded = match raw.split_once(',') {
        Some((head, rest)) if head.to_lowercase().starts_with("data:") => rest,
        _ => raw,
    };
    base64::engine::general_purpose::STANDARD.decode(encoded.trim()).map_err(|e| format!("Incorrect padding or bad audio data: {e}"))
}

fn peaks_json(env: &Env, path: &Path, count: usize) -> Result<Obj, String> {
    let tools = env.tools()?;
    let info = read_audio_peaks(&tools, path, count).map_err(|e| format!("Could not read audio for waveform. Try a standard WAV, MP3, FLAC, or M4A file. {e}"))?;
    let mut out = Obj::new();
    out.insert("duration".into(), json!(info.duration));
    out.insert("sample_rate".into(), json!(info.sample_rate));
    out.insert("channels".into(), json!(info.channels));
    out.insert("peaks".into(), json!(info.peaks));
    Ok(out)
}

fn merged(mut base: Obj, extra: Obj) -> Value {
    base.extend(extra);
    Value::Object(base)
}

fn ffmpeg(env: &Env, args: &[&str], what: &str) -> Result<(), String> {
    let tools = env.tools()?;
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    tools.ffmpeg_checked(&args, what).map(|_| ()).map_err(|e| e.to_string())
}

fn m4a_to_wav(env: &Env, source: &Path, target: &Path) -> Result<PathBuf, String> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
    }
    let (s, t) = (text(source), text(target));
    ffmpeg(env, &["-hide_banner", "-loglevel", "error", "-y", "-i", &s, "-vn", "-acodec", "pcm_s16le", "-ar", "44100", "-ac", "2", &t], "Could not convert audio to WAV")
        .map_err(|e| format!("Could not convert audio to WAV: {e}"))?;
    if fs::metadata(target).map_or(true, |m| m.len() == 0) {
        return Err("Audio conversion did not create a WAV file.".into());
    }
    Ok(target.to_path_buf())
}

/// `POST .../save_scene_audio`: a scene's own audio, `scene_audio/audio_NNNN.<ext>`.
pub fn save_scene_audio(env: &Env, payload: &Obj) -> Reply {
    let project = project_of(payload)?;
    let scene = int_or(payload, "scene_number", 1)?.max(1);
    let folder = project.join("scene_audio");
    fs::create_dir_all(&folder).map_err(|e| io(&folder, e))?;
    let preserve = payload.get("preserve_source").is_some_and(truthy);
    let audio_name = text_or_empty(payload.get("audio_name")).trim().to_string();
    let allowed = [".wav", ".mp3", ".flac", ".m4a", ".ogg"];
    let clean = |ext: String| if allowed.contains(&ext.as_str()) { ext } else { ".wav".to_string() };
    let target = if !text_or_empty(payload.get("audio_data")).trim().is_empty() {
        let ext = Some(extension(Path::new(&audio_name))).filter(|e| !e.is_empty()).unwrap_or_else(|| ".wav".into());
        let target = if preserve {
            let sources = folder.join("sources");
            fs::create_dir_all(&sources).map_err(|e| io(&sources, e))?;
            sources.join(format!("source_{}{ext}", random_hex()))
        } else {
            folder.join(format!("audio_{scene:04}{}", clean(ext)))
        };
        fs::write(&target, audio_bytes(&text_or_empty(payload.get("audio_data")))?).map_err(|e| io(&target, e))?;
        target
    } else {
        let source = resolve_existing_file(&text_or_empty(payload.get("source_path")), "Audio file")?;
        let ext = Some(extension(&source)).filter(|e| !e.is_empty()).unwrap_or_else(|| ".wav".into());
        let target = if preserve {
            let sources = folder.join("sources");
            fs::create_dir_all(&sources).map_err(|e| io(&sources, e))?;
            sources.join(format!("source_{}{ext}", random_hex()))
        } else {
            folder.join(format!("audio_{scene:04}{}", clean(ext)))
        };
        fs::copy(&source, &target).map_err(|e| io(&target, e))?;
        target
    };
    let info = peaks_json(env, &target, 600)?;
    let mut head = Obj::new();
    head.insert("saved_path".into(), json!(text(&target)));
    head.insert("audio_folder".into(), json!(text(&folder)));
    head.insert("scene_number".into(), json!(scene));
    Ok(merged(head, info))
}

/// `POST .../save_project_audio`: the song, `project_audio/project_audio.<ext>` (an M4A becomes a WAV), with its waveform
/// and beats.
pub fn save_project_audio(env: &Env, payload: &Obj) -> Reply {
    let project = project_of(payload)?;
    fs::create_dir_all(&project).map_err(|e| io(&project, e))?;
    let folder = project.join("project_audio");
    fs::create_dir_all(&folder).map_err(|e| io(&folder, e))?;
    let name = Some(text_or_empty(payload.get("audio_name")).trim().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| "project_audio.wav".into());
    let ext = Some(extension(Path::new(&name))).filter(|e| [".wav", ".mp3", ".flac", ".m4a", ".ogg"].contains(&e.as_str())).unwrap_or_else(|| ".wav".into());
    let mut target = folder.join(format!("project_audio{}", if ext == ".m4a" { ".wav" } else { &ext }));
    let raw_target = if ext == ".m4a" { folder.join(format!("project_audio_source{ext}")) } else { target.clone() };
    if !text_or_empty(payload.get("audio_data")).trim().is_empty() {
        fs::write(&raw_target, audio_bytes(&text_or_empty(payload.get("audio_data")))?).map_err(|e| io(&raw_target, e))?;
    } else {
        let source = resolve_existing_file(&text_or_empty(payload.get("source_path")), "Audio file")?;
        fs::copy(&source, &raw_target).map_err(|e| io(&raw_target, e))?;
    }
    if ext == ".m4a" {
        target = m4a_to_wav(env, &raw_target, &target)?;
        let _ = fs::remove_file(&raw_target);
    }
    let info = peaks_json(env, &target, 1600)?;
    let peaks: Vec<f64> = info.get("peaks").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
    let duration = info.get("duration").and_then(Value::as_f64).unwrap_or(0.0);
    let (beats, tempo) = estimate_beats(&env.tools()?, &target, &peaks, duration);
    let mut head = Obj::new();
    head.insert("saved_path".into(), json!(text(&target)));
    head.insert("audio_folder".into(), json!(text(&folder)));
    let mut out = merged(head, info);
    if let Some(map) = out.as_object_mut() {
        map.insert("beats".into(), json!(beats));
        map.insert("tempo_bpm".into(), json!(tempo));
    }
    Ok(out)
}

/// `POST .../save_project_srt`: the project's `builder_segments.srt` from SRT text.
pub fn save_project_srt(payload: &Obj) -> Reply {
    let project = project_of(payload)?;
    fs::create_dir_all(&project).map_err(|e| io(&project, e))?;
    let srt = text_or_empty(payload.get("srt_text"));
    if srt.trim().is_empty() {
        return Err("SRT text is empty.".into());
    }
    let path = srt_path(&project);
    atomic_write_text(&path, &srt).map_err(|e| io(&path, e))?;
    Ok(json!({"srt_path": text(&path), "segments": parse_srt_segments(&srt)?}))
}

/// `POST .../save_single_scene_srt`: `scene_srt/scene_NNNN.srt` with one block for the scene.
pub fn save_single_scene_srt(payload: &Obj) -> Reply {
    let project = project_of(payload)?;
    let scene = int_or(payload, "scene_number", 1)?;
    let duration = float_or(payload, "duration", 4.0).max(0.1);
    let start = float_or(payload, "start_time", 0.0).max(0.0);
    let label = Some(text_or_empty(payload.get("label")).trim().to_string()).filter(|l| !l.is_empty()).unwrap_or_else(|| format!("Scene {scene}"));
    let folder = project.join("scene_srt");
    fs::create_dir_all(&folder).map_err(|e| io(&folder, e))?;
    let path = folder.join(format!("scene_{scene:04}.srt"));
    let body = ["1".to_string(), format!("{} --> {}", format_srt_time(start), format_srt_time(start + duration)), label, String::new()].join("\n");
    atomic_write_text(&path, &body).map_err(|e| io(&path, e))?;
    Ok(json!({"srt_path": text(&path), "scene_number": scene, "start_time": start, "duration": duration}))
}

/// `POST .../trim_scene_audio`: the scene's slice of a source file as `scene_audio_trimmed/scene_audio_NNNN.wav`.
pub fn trim_scene_audio(env: &Env, payload: &Obj) -> Reply {
    let project = project_of(payload)?;
    let source = resolve_existing_file(&text_or_empty(payload.get("source_path")), "Audio file")?;
    let scene = int_or(payload, "scene_number", 1)?;
    let start = float_or(payload, "start", 0.0).max(0.0);
    let mut duration = float_or(payload, "duration", 0.0).max(0.05);
    let tools = env.tools()?;
    let source_duration = read_audio_peaks(&tools, &source, 16).map(|i| i.duration).unwrap_or(0.0);
    if source_duration > 0.0 {
        let remaining = source_duration - start;
        if remaining <= 0.01 {
            return Err(format!(
                "Scene {scene} audio trim starts after the source audio ends. Trim start: {start:.3}s; audio length: {source_duration:.3}s. Shorten or move the scene, load longer audio, or add silence before rendering."
            ));
        }
        duration = duration.min(remaining.max(0.05));
    }
    let folder = project.join("scene_audio_trimmed");
    fs::create_dir_all(&folder).map_err(|e| io(&folder, e))?;
    let target = folder.join(format!("scene_audio_{scene:04}.wav"));
    let (s, t) = (text(&source), text(&target));
    ffmpeg(env, &["-y", "-ss", &py_float(start), "-i", &s, "-t", &py_float(duration), "-vn", "-ac", "2", "-ar", "44100", "-c:a", "pcm_s16le", &t], "ffmpeg failed to trim scene audio.")?;
    let trimmed = read_audio_peaks(&tools, &target, 16).map(|i| i.duration).unwrap_or(0.0);
    if trimmed <= 0.01 {
        return Err(format!(
            "Scene {scene} audio trim was empty. Trim start: {start:.3}s; requested duration: {duration:.3}s. Shorten or move the scene, load longer audio, or add silence before rendering."
        ));
    }
    Ok(json!({"audio_path": text(&target), "scene_number": scene, "start": start, "duration": trimmed, "requested_duration": float_or(payload, "duration", 0.0), "format": "pcm_s16le_wav"}))
}

/// Python's `str(float)`.
fn py_float(value: f64) -> String {
    h3_core::pyval::py_float_str(value)
}

fn concat_path(path: &Path) -> String {
    absolute(path).display().to_string().replace('\\', "/").replace('\'', "'\\''")
}

/// `POST .../prepare_scene_audio_mix`: join each scene's audio (its own clip, or its slice of the song) into one file
/// with silence in the gaps, and write the matching SRT.
pub fn prepare_scene_audio_mix(env: &Env, payload: &Obj) -> Reply {
    let project = project_of(payload)?;
    let segments = payload.get("segments").and_then(Value::as_array).cloned().unwrap_or_default();
    if segments.is_empty() {
        return Err("No scenes were provided for scene audio mix.".into());
    }
    let allow_missing = payload.get("allow_missing_scene_audio").is_some_and(truthy);
    let global = {
        let path = absolute(Path::new(&path_text(payload, "global_audio_path")));
        if path.is_file() { Some(path) } else { None }
    };
    let folder = project.join("project_audio");
    fs::create_dir_all(&folder).map_err(|e| io(&folder, e))?;
    let parts = folder.join("_scene_audio_mix_parts");
    let _ = fs::remove_dir_all(&parts);
    fs::create_dir_all(&parts).map_err(|e| io(&parts, e))?;

    struct Item {
        index: usize,
        path: Option<PathBuf>,
        start: f64,
        duration: f64,
        source_start: f64,
    }
    let mut items: Vec<Item> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    for (i, segment) in segments.iter().enumerate() {
        let index = i + 1;
        let Some(segment) = segment.as_object() else {
            missing.push(format!("Scene {index}: invalid scene data."));
            continue;
        };
        let raw = text_or_empty(segment.get("custom_audio_path")).trim().trim_matches('"').to_string();
        let seg_start = float_or(segment, "start", 0.0).max(0.0);
        let seg_end = segment.get("end").filter(|v| truthy(v)).and_then(to_float).unwrap_or(seg_start + 4.0).max(seg_start + 0.05);
        if raw.is_empty() {
            let duration = (seg_end - seg_start).max(0.05);
            if let Some(global) = &global {
                items.push(Item { index, path: Some(global.clone()), start: seg_start, duration, source_start: seg_start });
            } else if allow_missing {
                items.push(Item { index, path: None, start: seg_start, duration, source_start: 0.0 });
            } else {
                missing.push(format!("Scene {index}: custom audio is missing."));
            }
            continue;
        }
        let path = absolute(Path::new(&raw));
        if !path.is_file() {
            missing.push(format!("Scene {index}: custom audio file was not found: {}", path.display()));
            continue;
        }
        let start = segment.get("custom_audio_timeline_start").filter(|v| truthy(v)).and_then(to_float).unwrap_or(seg_start).max(0.0);
        let mut duration = float_or(segment, "custom_audio_duration", 0.0);
        if duration <= 0.0 {
            duration = seg_end - seg_start;
        }
        items.push(Item { index, path: Some(path), start, duration: duration.max(0.05), source_start: float_or(segment, "custom_audio_source_start", 0.0).max(0.0) });
    }
    if !missing.is_empty() {
        return Err(missing.join("\n"));
    }
    items.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal).then(a.index.cmp(&b.index)));

    let mut part_paths: Vec<PathBuf> = Vec::new();
    let mut cursor = 0.0f64;
    let mut part_index = 1;
    for item in &items {
        let gap = (item.start - cursor).max(0.0);
        if gap > 0.01 {
            let silence = parts.join(format!("part_{part_index:04}_silence.wav"));
            part_index += 1;
            ffmpeg(env, &["-y", "-f", "lavfi", "-i", "anullsrc=r=44100:cl=stereo", "-t", &format!("{gap:.6}"), "-c:a", "pcm_s16le", &text(&silence)], "ffmpeg failed to create silence.")?;
            part_paths.push(silence);
        }
        let clip = parts.join(format!("part_{part_index:04}_scene_{:04}.wav", item.index));
        part_index += 1;
        match &item.path {
            None => ffmpeg(env, &["-y", "-f", "lavfi", "-i", "anullsrc=r=44100:cl=stereo", "-t", &format!("{:.6}", item.duration), "-c:a", "pcm_s16le", &text(&clip)], "ffmpeg failed to create silence.")?,
            Some(path) => ffmpeg(
                env,
                &["-y", "-ss", &format!("{:.6}", item.source_start), "-i", &text(path), "-t", &format!("{:.6}", item.duration), "-ac", "2", "-ar", "44100", "-c:a", "pcm_s16le", &text(&clip)],
                &format!("ffmpeg failed to prepare scene {} audio.", item.index),
            )?,
        }
        part_paths.push(clip);
        cursor = cursor.max(item.start + item.duration);
    }
    if part_paths.is_empty() {
        return Err("No scene audio parts were created.".into());
    }
    let mix = folder.join("scene_audio_mix.wav");
    let list = parts.join("scene_audio_mix_list.txt");
    atomic_write_text(&list, &part_paths.iter().map(|p| format!("file '{}'\n", concat_path(p))).collect::<String>()).map_err(|e| io(&list, e))?;
    ffmpeg(env, &["-y", "-f", "concat", "-safe", "0", "-i", &text(&list), "-c:a", "pcm_s16le", &text(&mix)], "ffmpeg failed to create scene audio mix.")?;
    let srt = srt_path(&project);
    let field = if text_or_empty(payload.get("srt_text_field")).trim() == "lyric_text" { "lyric_text" } else { "label" };
    atomic_write_text(&srt, &segments_to_srt_field(&segments, field)).map_err(|e| io(&srt, e))?;
    let _ = fs::remove_dir_all(&parts);
    let info = peaks_json(env, &mix, 1600)?;
    let peaks: Vec<f64> = info.get("peaks").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
    let duration = info.get("duration").and_then(Value::as_f64).unwrap_or(cursor);
    let (beats, tempo) = estimate_beats(&env.tools()?, &mix, &peaks, duration);
    Ok(json!({
        "audio_path": text(&mix),
        "srt_path": text(&srt),
        "duration": duration,
        "peaks": peaks,
        "beats": beats,
        "tempo_bpm": tempo,
        "scene_count": items.len(),
        "used_scene_audio": true,
    }))
}

/// `_segments_to_srt(segments, text_field)`: scenes in start order, one SRT block each.
fn segments_to_srt_field(segments: &[Value], field: &str) -> String {
    if field == "label" {
        return segments_to_srt(segments);
    }
    let mut ordered: Vec<&Value> = segments.iter().collect();
    ordered.sort_by(|a, b| float_or(a.as_object().unwrap_or(&Obj::new()), "start", 0.0).partial_cmp(&float_or(b.as_object().unwrap_or(&Obj::new()), "start", 0.0)).unwrap_or(std::cmp::Ordering::Equal));
    let mut lines: Vec<String> = Vec::new();
    for (i, segment) in ordered.iter().enumerate() {
        let empty = Obj::new();
        let segment = segment.as_object().unwrap_or(&empty);
        let start = float_or(segment, "start", 0.0);
        let end = segment.get("end").filter(|v| truthy(v)).and_then(to_float).unwrap_or(start + 4.0).max(start + 0.1);
        // `segment.get(a) or segment.get(b) or ...`: the first value that is truthy, then trimmed.
        let chosen = [field, "label", "t2i_prompt"].iter().find_map(|key| segment.get(*key).filter(|v| truthy(v)));
        let text = chosen.map_or_else(|| format!("Scene {}", i + 1), |v| text_or_empty(Some(v)).trim().to_string());
        lines.extend([(i + 1).to_string(), format!("{} --> {}", format_srt_time(start), format_srt_time(end)), text, String::new()]);
    }
    format!("{}\n", lines.join("\n").trim())
}

/// `POST .../create_silent_audio`: a silent WAV of a chosen length, for a scene or the whole project. Answers with
/// the `ok` inside the body.
pub fn create_silent_audio(env: &Env, payload: &Obj) -> Reply {
    let project = project_of(payload)?;
    fs::create_dir_all(&project).map_err(|e| io(&project, e))?;
    let duration = payload.get("duration").and_then(to_float).filter(|d| d.is_finite() && *d > 0.0).ok_or("Silence duration must be greater than 0 seconds.")?.clamp(0.1, 86_400.0);
    let tag = format!("{duration:.2}").trim_end_matches('0').trim_end_matches('.').replace('.', "_");
    let scope = text_or_empty(payload.get("scope")).trim().to_lowercase();
    let (scope, scene, folder, path, display, peaks) = if scope == "scene" {
        let scene = payload.get("scene_number").and_then(to_int).unwrap_or(1).max(1);
        let folder = project.join("scene_audio");
        let path = folder.join(format!("audio_{scene:04}.wav"));
        ("scene", scene, folder, path, format!("Silence {duration:.2}s"), 600)
    } else {
        let folder = project.join("project_audio");
        let path = folder.join(format!("project_silence_{tag}s.wav"));
        ("project", 0, folder, path, format!("Silent timeline {duration:.2}s"), 1600)
    };
    write_silent_wav(&path, duration)?;
    let info = peaks_json(env, &path, peaks)?;
    let mut head = Obj::new();
    head.insert("ok".into(), json!(true));
    head.insert("audio_path".into(), json!(text(&path)));
    head.insert("saved_path".into(), json!(text(&path)));
    head.insert("audio_folder".into(), json!(text(&folder)));
    head.insert("audio_name".into(), json!(display));
    head.insert("scope".into(), json!(scope));
    head.insert("scene_number".into(), json!(scene));
    Ok(merged(head, info))
}

fn write_silent_wav(path: &Path, duration: f64) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
    }
    let spec = hound::WavSpec { channels: 2, sample_rate: 44_100, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut writer = hound::WavWriter::create(path, spec).map_err(|e| io(path, e))?;
    let frames = (duration * 44_100.0).round_ties_even() as u64;
    for _ in 0..frames * 2 {
        writer.write_sample(0i16).map_err(|e| io(path, e))?;
    }
    writer.finalize().map_err(|e| io(path, e))?;
    if fs::metadata(path).map_or(true, |m| m.len() == 0) {
        return Err("Silent WAV file was not created.".into());
    }
    Ok(())
}

/// `POST .../prepare_audio_clip_mix`: a timeline of audio clips with silence between them as one cached WAV.
pub fn prepare_audio_clip_mix(env: &Env, payload: &Obj) -> Reply {
    let project_raw = text_or_empty(payload.get("project_folder")).trim().to_string();
    let Some(clips) = payload.get("clips").and_then(Value::as_array) else { return Err("Project folder and an audio clip list are required.".into()) };
    if project_raw.is_empty() {
        return Err("Project folder and an audio clip list are required.".into());
    }
    let mut duration = payload.get("duration").filter(|v| truthy(v)).and_then(to_float).unwrap_or(0.0);
    if !duration.is_finite() || duration <= 0.0 || duration > 86_400.0 {
        return Err("Audio timeline duration must be between 0 and 86400 seconds.".into());
    }
    let mut normalized: Vec<Obj> = Vec::new();
    for clip in clips {
        let Some(clip) = clip.as_object() else { return Err("Each audio clip must be an object.".into()) };
        let mut volume = match clip.get("volume") {
            None => 1.0,
            Some(v) => to_float(v).unwrap_or(f64::NAN),
        };
        if !volume.is_finite() || !(0.0..=2.0).contains(&volume) {
            return Err("Audio clip volume must be between 0 and 2 (0% to 200%).".into());
        }
        if clip.get("muted").is_some_and(truthy) {
            volume = 0.0;
        }
        // Without the flag, only dialogue goes into the generation mix.
        let included = match clip.get("include_in_generation") {
            Some(v) => truthy(v),
            None => clip.get("role").is_none_or(|role| role.as_str() == Some("dialogue")),
        };
        let path = absolute(Path::new(&text_or_empty(clip.get("path"))));
        let number = |key: &str| clip.get(key).and_then(to_float).unwrap_or(0.0);
        let (start, source_start, length) = (number("start"), number("source_start"), number("duration"));
        if ![start, source_start, length].iter().all(|v| v.is_finite()) {
            return Err("Audio clip timing must be finite.".into());
        }
        if start < 0.0 || source_start < 0.0 || length <= 0.0 || start + length > 86_400.0 {
            return Err("Audio clip timing is outside the supported range.".into());
        }
        if !path.is_file() {
            return Err(format!("Audio clip source not found: {}", path.display()));
        }
        duration = duration.max(start + length);
        if payload.get("generation_only").is_some_and(truthy) && !included {
            continue;
        }
        let mtime = fs::metadata(&path).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos() as u64);
        // Keys in sorted order, like `json.dumps(sort_keys=True)`, so both apps name the cached mix the same.
        let mut entry = Obj::new();
        for (key, value) in [("duration", json!(length)), ("mtime", json!(mtime)), ("path", json!(text(&path))), ("source_start", json!(source_start)), ("start", json!(start)), ("volume", json!(volume))] {
            entry.insert(key.into(), value);
        }
        normalized.push(entry);
    }
    let key_text = h3_core::pyjson::dumps_inline(&json!([normalized, duration]));
    let key: String = Sha256::digest(key_text.as_bytes()).iter().take(12).map(|b| format!("{b:02x}")).collect();
    let folder = absolute(Path::new(&project_raw)).join("project_audio").join("clip_edits");
    fs::create_dir_all(&folder).map_err(|e| io(&folder, e))?;
    let target = folder.join(format!("audio_edit_{key}.wav"));
    let peaks_of = |path: &Path| -> Result<Value, String> { Ok(peaks_json(env, path, 1600)?.get("peaks").cloned().unwrap_or(json!([]))) };
    if target.is_file() {
        return Ok(json!({"audio_path": text(&target), "duration": duration, "peaks": peaks_of(&target)?}));
    }
    let temporary = folder.join(format!(".{key}_{}.wav", random_hex()));
    let mut command: Vec<String> = ["-y", "-f", "lavfi", "-i", "anullsrc=r=44100:cl=stereo"].iter().map(|s| s.to_string()).collect();
    let mut filters = vec![format!("[0:a]atrim=duration={duration:.9}[silence]")];
    for (i, clip) in normalized.iter().enumerate() {
        let index = i + 1;
        let num = |key: &str| clip.get(key).and_then(Value::as_f64).unwrap_or(0.0);
        command.extend(["-ss".into(), py_float(num("source_start")), "-t".into(), py_float(num("duration")), "-i".into(), text_or_empty(clip.get("path"))]);
        let delay = (num("start") * 44_100.0).round_ties_even() as i64;
        filters.push(format!(
            "[{index}:a]aresample=44100,aformat=channel_layouts=stereo,atrim=duration={:.9},asetpts=PTS-STARTPTS,volume={:.9},adelay={delay}S:all=1[c{index}]",
            num("duration"),
            num("volume")
        ));
    }
    let inputs = format!("[silence]{}", (1..=normalized.len()).map(|i| format!("[c{i}]")).collect::<String>());
    filters.push(format!("{inputs}amix=inputs={}:duration=first:normalize=0[out]", normalized.len() + 1));
    command.extend(["-filter_complex".into(), filters.join(";"), "-map".into(), "[out]".into(), "-t".into(), py_float(duration), "-c:a".into(), "pcm_s16le".into(), text(&temporary)]);
    let tools = env.tools()?;
    let run = tools.ffmpeg_checked(&command, "Could not prepare edited timeline audio.").map_err(|e| e.to_string());
    let finished = run.and_then(|_| fs::rename(&temporary, &target).map_err(|e| io(&target, e)));
    let _ = fs::remove_file(&temporary);
    finished?;
    Ok(json!({"audio_path": text(&target), "duration": duration, "peaks": peaks_of(&target)?}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(value: Value) -> Obj {
        value.as_object().cloned().unwrap()
    }

    fn env() -> Env {
        Env { output: PathBuf::new(), roots: vec![], ffmpeg: "ffmpeg".into(), ffprobe: "ffprobe".into(), luts: PathBuf::new(), overlay_dir: None }
    }

    fn png_data_url() -> String {
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::RgbImage::from_pixel(4, 3, image::Rgb([200, 30, 30])).write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes.into_inner()))
    }

    #[test]
    fn scene_image_from_data_becomes_a_png_in_the_approved_folder() {
        let dir = tempfile::tempdir().unwrap();
        let project = text(dir.path());
        let saved = save_scene_image(&obj(json!({"project_folder": project, "scene_number": 3, "image_data": png_data_url()})), None).unwrap();
        assert!(saved["saved_path"].as_str().unwrap().ends_with("image_0003.png"));
        assert_eq!(image::open(saved["saved_path"].as_str().unwrap()).unwrap().width(), 4);
        let archived = archive_scene_image(&obj(json!({"project_folder": project, "scene_number": 3, "image_data": png_data_url()})), None, &Stamp::now()).unwrap();
        assert!(archived["saved_path"].as_str().unwrap().contains("scene_0003"));
        assert!(save_scene_image(&obj(json!({"project_folder": project, "image_data": "data:text/plain;base64,AAAA"})), None).unwrap_err().contains("base64 image data URL"));
        assert_eq!(save_scene_image(&obj(json!({})), None).unwrap_err(), "Project folder is empty.");
        assert!(save_scene_image(&obj(json!({"project_folder": project, "image": {"filename": "a.png"}})), None).unwrap_err().contains("ComfyUI's folders"));
    }

    #[test]
    fn only_project_files_can_be_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("p");
        fs::create_dir_all(&project).unwrap();
        let inside_file = project.join("a.png");
        fs::write(&inside_file, b"x").unwrap();
        let outside = dir.path().join("b.png");
        fs::write(&outside, b"x").unwrap();
        let request = |path: &Path| obj(json!({"project_folder": text(&project), "path": text(path)}));
        assert_eq!(delete_project_media(&request(&outside)).unwrap_err(), "This file is outside the current project folder, so it was not deleted.");
        assert_eq!(delete_project_media(&request(&inside_file)).unwrap()["deleted"], true);
        assert_eq!(delete_project_media(&request(&inside_file)).unwrap()["reason"], "File was already missing.");
    }

    #[test]
    fn reference_pictures_get_free_names_and_the_kind_decides_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let project = text(dir.path());
        let first = save_flux_reference_image(&obj(json!({"project_folder": project, "reference_type": "subject", "name": "The Singer!", "image_data": png_data_url()})), None).unwrap();
        let second = save_flux_reference_image(&obj(json!({"project_folder": project, "reference_type": "subject", "name": "The Singer!", "image_data": png_data_url()})), None).unwrap();
        assert!(first["saved_path"].as_str().unwrap().ends_with("The Singer.png") || first["saved_path"].as_str().unwrap().ends_with("The Singer_.png"));
        assert!(second["saved_path"].as_str().unwrap().ends_with("_02.png"));
        let odd = save_flux_reference_image(&obj(json!({"project_folder": project, "reference_type": "nonsense", "image_data": png_data_url()})), None).unwrap();
        assert_eq!(odd["reference_type"], "location");
        assert!(odd["folder"].as_str().unwrap().ends_with("locations"));
    }

    #[test]
    fn imported_references_come_from_the_subject_location_folders() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("subject_location").join("subject");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("Mia.png"), base64::engine::general_purpose::STANDARD.decode(png_data_url().split_once(',').unwrap().1).unwrap()).unwrap();
        fs::write(folder.join("Mia.txt"), "tall, red coat\n").unwrap();
        fs::write(folder.join("Jo.png"), b"not a picture").unwrap();
        let result = import_references(&obj(json!({"project_folder": text(dir.path())})), "subject").unwrap();
        let subjects = result["subjects"].as_array().unwrap();
        assert_eq!(subjects.len(), 2);
        assert_eq!(subjects[0]["name"], "Jo");
        assert_eq!(subjects[1]["description"], "tall, red coat");
        assert_eq!(subjects[1]["id"], "subj_import_2_Mia");
        assert!(subjects[1]["image"]["data"].as_str().unwrap().starts_with("data:image/jpeg;base64,"));
        assert_eq!(result["missing_descriptions"], json!(["Jo.txt"]));
        assert!(import_references(&obj(json!({"project_folder": text(dir.path())})), "location").unwrap_err().starts_with("Location folder does not exist"));
    }

    #[test]
    fn srt_and_scene_srt_are_written() {
        let dir = tempfile::tempdir().unwrap();
        let project = text(dir.path());
        let saved = save_project_srt(&obj(json!({"project_folder": project, "srt_text": "1\n00:00:00,000 --> 00:00:02,000\nHello\n"}))).unwrap();
        assert_eq!(saved["segments"].as_array().unwrap().len(), 1);
        assert_eq!(save_project_srt(&obj(json!({"project_folder": project, "srt_text": "  "}))).unwrap_err(), "SRT text is empty.");
        let single = save_single_scene_srt(&obj(json!({"project_folder": project, "scene_number": 2, "start_time": 4.5, "duration": 3.25}))).unwrap();
        let body = fs::read_to_string(single["srt_path"].as_str().unwrap()).unwrap();
        assert_eq!(body, "1\n00:00:04,500 --> 00:00:07,750\nScene 2\n");
    }

    #[test]
    fn silent_audio_has_the_asked_length() {
        if env().tools().is_err() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let project = text(dir.path());
        let silent = create_silent_audio(&env(), &obj(json!({"project_folder": project, "duration": 1.5}))).unwrap();
        assert_eq!(silent["scope"], "project");
        assert!(silent["audio_path"].as_str().unwrap().ends_with("project_silence_1_5s.wav"));
        assert!((silent["duration"].as_f64().unwrap() - 1.5).abs() < 0.01);
        let scene = create_silent_audio(&env(), &obj(json!({"project_folder": project, "duration": 2, "scope": "scene", "scene_number": 4}))).unwrap();
        assert!(scene["audio_path"].as_str().unwrap().ends_with("audio_0004.wav"));
        assert!(create_silent_audio(&env(), &obj(json!({"project_folder": project, "duration": 0}))).is_err());
    }

    #[test]
    fn scene_audio_is_saved_and_the_mix_fills_gaps_with_silence() {
        if env().tools().is_err() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let project = text(dir.path());
        let silent = create_silent_audio(&env(), &obj(json!({"project_folder": project, "duration": 6}))).unwrap();
        let song = silent["audio_path"].as_str().unwrap().to_string();
        let saved = save_scene_audio(&env(), &obj(json!({"project_folder": project, "scene_number": 2, "source_path": song}))).unwrap();
        assert!(saved["saved_path"].as_str().unwrap().ends_with("audio_0002.wav"));
        assert!(!saved["peaks"].as_array().unwrap().is_empty());
        let mix = prepare_scene_audio_mix(
            &env(),
            &obj(json!({"project_folder": project, "global_audio_path": song, "segments": [{"start": 1.0, "end": 3.0, "label": "a"}, {"start": 4.0, "end": 5.5, "label": "b"}]})),
        )
        .unwrap();
        assert_eq!(mix["scene_count"], 2);
        assert!((mix["duration"].as_f64().unwrap() - 5.5).abs() < 0.05, "{}", mix["duration"]);
        let error = prepare_scene_audio_mix(&env(), &obj(json!({"project_folder": project, "segments": [{"start": 0, "end": 2}]}))).unwrap_err();
        assert_eq!(error, "Scene 1: custom audio is missing.");
        let trimmed = trim_scene_audio(&env(), &obj(json!({"project_folder": project, "source_path": song, "scene_number": 1, "start": 1.0, "duration": 2.0}))).unwrap();
        assert!((trimmed["duration"].as_f64().unwrap() - 2.0).abs() < 0.05);
        let late = trim_scene_audio(&env(), &obj(json!({"project_folder": project, "source_path": song, "scene_number": 1, "start": 9.0, "duration": 2.0}))).unwrap_err();
        assert!(late.contains("starts after the source audio ends"));
        let edited = prepare_audio_clip_mix(&env(), &obj(json!({"project_folder": project, "duration": 4, "clips": [{"path": song, "start": 1.0, "source_start": 0.0, "duration": 1.0}]}))).unwrap();
        assert!(edited["audio_path"].as_str().unwrap().contains("audio_edit_"));
        let again = prepare_audio_clip_mix(&env(), &obj(json!({"project_folder": project, "duration": 4, "clips": [{"path": song, "start": 1.0, "source_start": 0.0, "duration": 1.0}]}))).unwrap();
        assert_eq!(edited["audio_path"], again["audio_path"]);
        assert!(prepare_audio_clip_mix(&env(), &obj(json!({"project_folder": project, "duration": 4, "clips": [{"path": song, "start": 1.0, "duration": 1.0, "volume": 3}]}))).unwrap_err().contains("volume"));
    }
}
