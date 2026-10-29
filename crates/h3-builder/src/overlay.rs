//! The overlay routes of the Builder: what this build can draw, and the render that lays the overlays over a video. The pack answers the same
//! two paths once its Python twin exists, and the Builder shows its overlay lane only when the first one answers.

use std::fs;
use std::path::{Path, PathBuf};

use h3_core::overlay_layers::{caption_clips, caption_lines_from_scenes, caption_lines_from_timestamped, shift_overlay_layers, TEXT_STYLES};
use h3_core::pyval::{to_float, truthy, Obj};
use h3_media::fonts::{ensure_fonts, list_all_fonts, resolve_font, DEFAULT_FONT};
use h3_media::overlay::{render_overlay, render_overlay_frame};
use h3_media::post::{normalize_encode_crf, normalize_encode_preset};
use serde_json::{json, Value};

use crate::overlay_library::resolve_clip_paths;
use crate::payload::path_text;
use crate::Env;

type Reply = Result<Value, String>;

const VIDEO_EXTENSIONS: [&str; 5] = [".mp4", ".mov", ".avi", ".webm", ".mkv"];

/// Bumped when the overlay fields or the render change in a way the Builder has to know about.
const OVERLAY_VERSION: u32 = 1;

/// `GET /vrgdg/music_builder/overlay_layers/capabilities`.
pub fn capabilities(env: &Env) -> Reply {
    Ok(json!({
        "version": OVERLAY_VERSION,
        "kinds": ["media", "grain", "vignette", "lut"],
        "ffmpeg": env.tools().is_ok(),
    }))
}

fn absolute(path: &Path) -> PathBuf {
    h3_core::abspath::absolute(path).unwrap_or_else(|_| path.to_path_buf()).components().collect()
}

fn extension(path: &Path) -> String {
    path.extension().map(|e| format!(".{}", e.to_string_lossy().to_lowercase())).unwrap_or_default()
}

/// `POST /vrgdg/music_builder/overlay_layers/render`: `{input_path, overlay_layers, overlay_layer_track?, output_path?, replace_source?, encode_crf?, encode_preset?}`.
/// Writes the video with the overlays on it and says which clips it applied and which it left out. Nothing is written when no clip applies, and
/// `changed` is false.
pub fn render(env: &Env, payload: &Obj) -> Reply {
    let wanted = path_text(payload, "input_path");
    let input = absolute(Path::new(wanted.trim().trim_matches('"')));
    if wanted.trim().is_empty() {
        return Err("input_path is required.".into());
    }
    if !input.is_file() {
        return Err(format!("Input video was not found: {}", input.display()));
    }
    if !VIDEO_EXTENSIONS.contains(&extension(&input).as_str()) {
        return Err("Input video type is not supported.".into());
    }
    if payload.get("overlay_layer_track").and_then(|t| t.get("enabled")).is_some_and(|v| v == &Value::Bool(false)) {
        return Ok(json!({"changed": false, "applied": [], "skipped": [], "output": input.display().to_string(), "reason": "The overlays are off."}));
    }
    let clips = payload.get("overlay_layers").cloned().unwrap_or(Value::Null);
    if !clips.is_array() {
        return Err("overlay_layers must be a list.".into());
    }
    // A video made from a few scenes starts part way into the timeline: `time_offset` is where, in seconds.
    let clips = shift_overlay_layers(&clips, payload.get("time_offset").and_then(to_float).unwrap_or(0.0));
    // A clip saved from the library has a path inside the project (`overlay/dust.mov`).
    let clips = resolve_clip_paths(&clips, &path_text(payload, "project_folder"));
    let replace = payload.get("replace_source").is_some_and(truthy);
    let stem = input.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let extension = input.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_else(|| ".mp4".into());
    let asked = path_text(payload, "output_path");
    let output = if replace {
        input.with_file_name(format!(".{stem}.vrgdg_overlay{extension}"))
    } else if asked.trim().is_empty() {
        input.with_file_name(format!("{stem}_overlay{extension}"))
    } else {
        absolute(Path::new(asked.trim()))
    };
    let crf = normalize_encode_crf(Some(payload.get("encode_crf").and_then(to_float).unwrap_or(21.0)));
    let preset = normalize_encode_preset(&path_text(payload, "encode_preset"));
    let tools = env.tools()?;
    let luts = env.luts.clone();
    let resolve_lut = |name: &str| {
        let file = Path::new(name.trim()).file_name()?.to_string_lossy().into_owned();
        Some(luts.join(file)).filter(|p| p.is_file())
    };
    let fonts = env.overlay_library().join("fonts");
    let _ = ensure_fonts(&fonts);
    let resolve_font_file = |name: &str| resolve_font(&fonts, name);
    let mut reply = render_overlay(&tools, &input, &output, &clips, &resolve_lut, &resolve_font_file, crf, &preset).map_err(|e| e.to_string())?;
    if replace && reply["changed"] == true {
        fs::rename(&output, &input).map_err(|e| {
            let _ = fs::remove_file(&output);
            format!("The video with the overlays could not replace {}: {e}", input.display())
        })?;
        reply["output"] = json!(input.display().to_string());
    }
    reply["replace_source"] = json!(replace);
    Ok(reply)
}

/// `POST /vrgdg/music_builder/overlay_layers/preview_frame`: `{input_path, seek, time_offset, overlay_layers, overlay_layer_track?, project_folder}`. One frame of a scene video
/// with the overlays on it, made by the same render Export uses. `seek` is how far into the scene video the frame is and `time_offset` where on the
/// timeline that video starts (the scene's start), so the clips land where they are on the timeline. The picture comes back as a PNG data
/// address in `image`, with the clips the render applied and left out.
pub fn preview_frame(env: &Env, payload: &Obj) -> Reply {
    let wanted = path_text(payload, "input_path");
    if wanted.trim().is_empty() {
        return Err("input_path is required.".into());
    }
    let input = absolute(Path::new(wanted.trim().trim_matches('"')));
    if !input.is_file() {
        return Err(format!("Input video was not found: {}", input.display()));
    }
    if !VIDEO_EXTENSIONS.contains(&extension(&input).as_str()) {
        return Err("Input video type is not supported.".into());
    }
    let seek = payload.get("seek").and_then(to_float).unwrap_or(0.0).max(0.0);
    let off = payload.get("overlay_layer_track").and_then(|t| t.get("enabled")).is_some_and(|v| v == &Value::Bool(false));
    let clips = if off {
        json!([])
    } else {
        let list = payload.get("overlay_layers").cloned().unwrap_or(Value::Null);
        if !list.is_array() {
            return Err("overlay_layers must be a list.".into());
        }
        // The picture is `seek` seconds into a video that starts at `time_offset` on the timeline.
        let moved = shift_overlay_layers(&list, payload.get("time_offset").and_then(to_float).unwrap_or(0.0) + seek);
        resolve_clip_paths(&moved, &path_text(payload, "project_folder"))
    };
    let tools = env.tools()?;
    let luts = env.luts.clone();
    let resolve_lut = |name: &str| {
        let file = Path::new(name.trim()).file_name()?.to_string_lossy().into_owned();
        Some(luts.join(file)).filter(|p| p.is_file())
    };
    let fonts = env.overlay_library().join("fonts");
    let _ = ensure_fonts(&fonts);
    let resolve_font_file = |name: &str| resolve_font(&fonts, name);
    let (mut reply, png) = render_overlay_frame(&tools, &input, seek, &clips, &resolve_lut, &resolve_font_file).map_err(|e| e.to_string())?;
    reply["image"] = json!(format!("data:image/png;base64,{}", base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &png)));
    reply["track_off"] = json!(off);
    Ok(reply)
}

/// `GET /vrgdg/music_builder/overlay_layers/fonts`: the fonts a text clip can use: the bundled ones, any font file in the fonts folder and the
/// fonts installed on the computer (`system`, read from the system font folders again on every request).
pub fn fonts(env: &Env) -> Reply {
    let dir = env.overlay_library().join("fonts");
    let _ = ensure_fonts(&dir);
    let list: Vec<Value> = list_all_fonts(&dir, true).into_iter().map(|f| json!({"name": f.name, "file": f.file, "bundled": f.bundled, "system": f.system})).collect();
    Ok(json!({"fonts": list, "default": DEFAULT_FONT, "folder": dir.display().to_string()}))
}

/// The font file a name resolves to, for the page to load it in the preview.
pub fn font_path(env: &Env, name: &str) -> Result<PathBuf, String> {
    let dir = env.overlay_library().join("fonts");
    let _ = ensure_fonts(&dir);
    resolve_font(&dir, name).ok_or_else(|| format!("There is no font called {name}."))
}

/// `POST /vrgdg/music_builder/overlay_layers/lyric_captions`: `{project_folder, style?, font?, from?, to?}`. One text clip for each line of the song's lyrics, with the
/// time the line is sung: the project's word-timed lyrics when it has them, otherwise the lyrics of each scene over the whole scene. The clips
/// come back for the page to add (they carry `group: "lyric_captions"`). A `font` (any name of the fonts list) replaces the font of the style.
pub fn lyric_captions(env: &Env, payload: &Obj) -> Reply {
    let font = path_text(payload, "font");
    let font = font.trim();
    if !font.is_empty() {
        font_path(env, font)?;
    }
    let folder = path_text(payload, "project_folder");
    let folder = Path::new(folder.trim());
    if !folder.join("vrgdg_builder_session.json").is_file() {
        return Err("project_folder is not a project folder.".into());
    }
    let style = path_text(payload, "style");
    let style = if TEXT_STYLES.contains(&style.trim()) { style.trim().to_string() } else { "clean".to_string() };
    let read = |path: PathBuf| std::fs::read(&path).ok().and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    let timed = read(folder.join("project_context").join("timestamped_lyrics.json")).map(|doc| caption_lines_from_timestamped(&doc)).unwrap_or_default();
    let (source, lines) = if timed.is_empty() {
        let session = read(folder.join("vrgdg_builder_session.json")).unwrap_or(Value::Null);
        ("scenes", caption_lines_from_scenes(&session))
    } else {
        ("timestamped_lyrics", timed)
    };
    if lines.is_empty() {
        return Err("The project has no lyrics to caption. Add lyrics to the scenes or run the timestamped lyrics step first.".into());
    }
    let from = payload.get("from").and_then(to_float);
    let to = payload.get("to").and_then(to_float);
    let range = match (from, to) {
        (Some(a), Some(b)) if b > a => Some((a, b)),
        _ => None,
    };
    let mut clips = caption_clips(&lines, &style, range);
    if !font.is_empty() {
        for clip in clips.iter_mut().filter_map(Value::as_object_mut) {
            clip.insert("font".into(), json!(font));
        }
    }
    Ok(json!({"source": source, "style": style, "styles": TEXT_STYLES, "count": clips.len(), "clips": clips}))
}
