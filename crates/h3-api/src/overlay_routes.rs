//! Overlays for agents: read and change a project's overlay layers (`GET/PUT /projects/{pid}/overlay-layers`, `POST .../overlay-layers/add`, `.../overlay-layers/lyric_captions`, `.../overlay-layers/media`), look
//! at the overlay library (`GET /overlay-library`), and the overlay pass that follows every stitch. The track is the `overlay_layer_track` and `overlay_layers` keys of the session, the
//! same ones the Video Builder's overlay lane saves, so a project an agent edited opens in the Builder with the clips on the lane.

use std::path::Path as FsPath;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use h3_builder::overlay as overlay_tools;
use h3_builder::overlay_library as library_tools;
use h3_builder::Env;
use h3_core::overlay_layers::{KINDS, TEXT_STYLES};
use h3_core::pyval::{text_or_empty, to_float, truthy, Obj};
use h3_media::fonts::resolve_font;
use h3_pipeline::stitch_plan::StitchSummary;
use h3_pipeline::StitchHook;
use h3_project::overlay_project::{self, Replace};
use h3_project::scene_ops::OpError;
use h3_project::{load_session, Stamp};
use serde_json::{json, Value};

use crate::app::{blocking, body_object, if_match, load, Shared};
use crate::error::{success, ApiError, ApiResult};
use crate::views::resolve_project;

fn op_error(e: OpError) -> ApiError {
    match e {
        OpError::Validation(m) => ApiError::validation(m),
        OpError::Conflict { current, incoming } => ApiError::revision_conflict(current, incoming),
        OpError::SceneNotFound(id) => ApiError::validation(format!("Scene '{id}' was not found.")),
        OpError::Io(e) => ApiError::internal(e.to_string()),
    }
}

fn reject_unknown(fields: &Obj, supported: &[&str]) -> Result<(), ApiError> {
    let unknown: Vec<&String> = fields.keys().filter(|k| !supported.contains(&k.as_str())).collect();
    if unknown.is_empty() {
        return Ok(());
    }
    Err(ApiError::validation(format!("Unknown field(s): {}. Supported: {}.", unknown.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "), supported.join(", ")))
        .details(json!({"unknown": unknown, "supported": supported})))
}

/// What a caller should know about clips that will not draw as asked: a kind the render does not know, a media file that is not there, a LUT or a
/// font that is not installed, text with no words. They are saved anyway (the Builder shows them), and the render skips or substitutes them.
pub fn warnings_for(env: &Env, project: &FsPath, clips: &[Value]) -> Vec<Value> {
    let fonts = env.overlay_library().join("fonts");
    let mut warnings = Vec::new();
    for clip in clips {
        let id = text_or_empty(clip.get("id"));
        let kind = text_or_empty(clip.get("kind"));
        let mut warn = |reason: String| warnings.push(json!({"id": id, "kind": kind, "reason": reason}));
        match kind.as_str() {
            "media" => {
                let path = text_or_empty(clip.get("path")).trim().to_string();
                let full = if FsPath::new(&path).is_relative() { project.join(&path) } else { FsPath::new(&path).to_path_buf() };
                if path.is_empty() {
                    warn("A media clip needs a path to a video file (add a library clip with POST /projects/{pid}/overlay-layers/media).".into());
                } else if !full.is_file() {
                    warn(format!("The media file was not found: {path}"));
                }
            }
            "lut" => {
                let name = text_or_empty(clip.get("lut")).trim().to_string();
                let file = FsPath::new(&name).file_name().map(|n| env.luts.join(n));
                if name.is_empty() || !file.is_some_and(|f| f.is_file()) {
                    warn(format!("The LUT is not installed: {}", if name.is_empty() { "none chosen" } else { name.as_str() }));
                }
            }
            "text" => {
                if text_or_empty(clip.get("text")).trim().is_empty() {
                    warn("The text is empty.".into());
                }
                let font = text_or_empty(clip.get("font"));
                if resolve_font(&fonts, &font).is_none() && !h3_media::fonts::BUNDLED.iter().any(|f| f.name.eq_ignore_ascii_case(font.trim())) {
                    warn(format!("The font '{font}' is not installed, so the default font is used. GET /overlay-library lists the fonts."));
                }
            }
            "grain" | "vignette" => {}
            other => warn(format!("'{other}' is not a clip kind the render knows ({}). It is saved and skipped.", KINDS.join(", "))),
        }
    }
    warnings
}

fn with_warnings(env: &Env, project: &FsPath, mut view: Value) -> Value {
    let clips = view.get("overlay_layers").and_then(Value::as_array).cloned().unwrap_or_default();
    if let Some(map) = view.as_object_mut() {
        map.insert("warnings".into(), Value::Array(warnings_for(env, project, &clips)));
    }
    view
}

/// `GET /projects/{pid}/overlay-layers`: the track switch and the clips as the render sees them, with the project's revision.
pub async fn get_overlay(State(state): Shared, Path(pid): Path<String>) -> ApiResult {
    let loaded = load(&state, &pid).await?;
    let env = state.builder_env();
    let view = blocking(move || with_warnings(&env, &loaded.project_folder, overlay_project::overlay_view(&loaded.session))).await?;
    let revision = view.get("revision").and_then(Value::as_i64);
    Ok(success(view, revision))
}

/// `PUT /projects/{pid}/overlay-layers`: `{overlay_layers?, overlay_layer_track?}`. Replaces the whole clip list and/or the track switch. Values are clamped, and the reply shows what was saved.
pub async fn put_overlay(State(state): Shared, Path(pid): Path<String>, headers: HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    let fields = body_object(body)?;
    let expected = if_match(&headers)?;
    let folder = resolve_project(&state.roots, &pid)?;
    let env = state.builder_env();
    let _guard = state.edit_lock.lock().await;
    let view = blocking(move || overlay_project::put_overlay(&folder, &fields, expected, &Stamp::now()).map(|view| with_warnings(&env, &folder, view))).await?.map_err(op_error)?;
    let revision = view.get("revision").and_then(Value::as_i64);
    Ok(success(view, revision))
}

/// `POST /projects/{pid}/overlay-layers/add`: `{layers: [...]}`. Adds clips to the end of the track without touching the others.
pub async fn add_clips(State(state): Shared, Path(pid): Path<String>, headers: HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    let fields = body_object(body)?;
    reject_unknown(&fields, &["layers"])?;
    let Some(clips) = fields.get("layers").and_then(Value::as_array).cloned() else { return Err(ApiError::validation("layers must be a list of overlay layers.")) };
    let expected = if_match(&headers)?;
    let folder = resolve_project(&state.roots, &pid)?;
    let env = state.builder_env();
    let _guard = state.edit_lock.lock().await;
    let view = blocking(move || overlay_project::add_clips(&folder, &clips, None, expected, &Stamp::now()).map(|view| with_warnings(&env, &folder, view))).await?.map_err(op_error)?;
    let revision = view.get("revision").and_then(Value::as_i64);
    Ok(success(view, revision))
}

/// `POST /projects/{pid}/overlay-layers/lyric_captions`: `{style?, from?, to?, replace?}`. One text clip for each lyric line, timed to the words of the song when
/// the project has them and to each scene when it does not. The earlier captions (those inside `from`..`to` when it is given) are replaced unless
/// `replace` is false.
pub async fn lyric_captions(State(state): Shared, Path(pid): Path<String>, headers: HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    let fields = body_object(body)?;
    reject_unknown(&fields, &["style", "from", "to", "replace"])?;
    let expected = if_match(&headers)?;
    let folder = resolve_project(&state.roots, &pid)?;
    let env = state.builder_env();
    let _guard = state.edit_lock.lock().await;
    let view = blocking(move || -> Result<Value, ApiError> {
        let mut payload = Obj::new();
        payload.insert("project_folder".into(), json!(folder.display().to_string()));
        for key in ["style", "from", "to"] {
            if let Some(value) = fields.get(key).filter(|v| !v.is_null()) {
                payload.insert(key.into(), value.clone());
            }
        }
        if let Some(style) = fields.get("style").filter(|v| !v.is_null()) {
            let name = text_or_empty(Some(style));
            if !TEXT_STYLES.contains(&name.trim()) {
                return Err(ApiError::validation(format!("style must be one of: {}.", TEXT_STYLES.join(", "))));
            }
        }
        let made = overlay_tools::lyric_captions(&env, &payload).map_err(ApiError::validation)?;
        let clips = made.get("clips").and_then(Value::as_array).cloned().unwrap_or_default();
        if clips.is_empty() {
            return Err(ApiError::validation("No lyric line falls inside that range."));
        }
        let range = match (fields.get("from").and_then(to_float), fields.get("to").and_then(to_float)) {
            (Some(a), Some(b)) if b > a => Some((a, b)),
            _ => None,
        };
        let replace = fields.get("replace").is_none_or(truthy).then_some(Replace { group: "lyric_captions", range });
        let mut view = overlay_project::add_clips(&folder, &clips, replace, expected, &Stamp::now()).map_err(op_error)?;
        if let Some(map) = view.as_object_mut() {
            for key in ["source", "style", "count"] {
                if let Some(value) = made.get(key) {
                    map.insert(if key == "count" { "captions".into() } else { key.into() }, value.clone());
                }
            }
        }
        Ok(with_warnings(&env, &folder, view))
    })
    .await??;
    let revision = view.get("revision").and_then(Value::as_i64);
    Ok(success(view, revision))
}

/// The end of the last scene on the timeline, or 0 when the project has no scenes.
fn timeline_end(session: &Obj) -> f64 {
    session.get("segments").and_then(Value::as_array).into_iter().flatten().filter_map(|s| s.get("end").and_then(to_float)).fold(0.0, f64::max)
}

/// `POST /projects/{pid}/overlay-layers/media`: `{library_id, start?, end?, ...clip settings}`. Copies a clip of the overlay library into the project's `overlay` folder and
/// adds it to the track. It starts at 0 and covers the whole timeline (looping its file) unless `start` and `end` say otherwise.
pub async fn add_media(State(state): Shared, Path(pid): Path<String>, headers: HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    let mut fields = body_object(body)?;
    let library_id = text_or_empty(fields.remove("library_id").as_ref()).trim().to_string();
    if library_id.is_empty() {
        return Err(ApiError::validation("library_id is required. GET /overlay-library lists the clips and their ids."));
    }
    let expected = if_match(&headers)?;
    let loaded = load(&state, &pid).await?;
    let env = state.overlay_env().await;
    let _guard = state.edit_lock.lock().await;
    let view = blocking(move || -> Result<Value, ApiError> {
        let folder = loaded.project_folder.clone();
        let listed = library_tools::list(&env).map_err(ApiError::validation)?;
        let item = listed.get("items").and_then(Value::as_array).and_then(|items| items.iter().find(|i| i.get("id").and_then(Value::as_str) == Some(library_id.as_str()))).cloned();
        let Some(item) = item else { return Err(ApiError::validation(format!("There is no clip '{library_id}' in the overlay library. GET /overlay-library lists the clips and their ids."))) };
        let mut payload = Obj::new();
        payload.insert("id".into(), json!(library_id));
        payload.insert("project_folder".into(), json!(folder.display().to_string()));
        let used = library_tools::use_in_project(&env, &payload).map_err(ApiError::validation)?;
        let start = fields.get("start").and_then(to_float).unwrap_or(0.0).max(0.0);
        let covers = timeline_end(&loaded.session).max(item.get("duration").and_then(to_float).unwrap_or(0.0) + start);
        let blend = if item.get("suggested_blend").and_then(Value::as_str) == Some("normal") { "normal" } else { "screen" };
        let mut clip = Obj::new();
        clip.insert("kind".into(), json!("media"));
        clip.insert("path".into(), used.get("path").cloned().unwrap_or(Value::Null));
        clip.insert("start".into(), json!(start));
        clip.insert("end".into(), json!(covers));
        clip.insert("loop".into(), json!(true));
        clip.insert("blend_mode".into(), json!(blend));
        clip.insert("locked".into(), json!(false));
        for (key, value) in fields {
            clip.insert(key, value);
        }
        let mut view = overlay_project::add_clips(&folder, &[Value::Object(clip)], None, expected, &Stamp::now()).map_err(op_error)?;
        if let Some(map) = view.as_object_mut() {
            map.insert("library_id".into(), json!(library_id));
            map.insert("path".into(), used.get("path").cloned().unwrap_or(Value::Null));
        }
        Ok(with_warnings(&env, &folder, view))
    })
    .await??;
    let revision = view.get("revision").and_then(Value::as_i64);
    Ok(success(view, revision))
}

/// `GET /overlay-library`: the clips in the overlay library (`id`, category, size, length, suggested blend), the fonts text can use, the installed LUTs, the caption
/// styles and the clip kinds.
pub async fn library(State(state): Shared) -> ApiResult {
    let env = state.overlay_env().await;
    let reply = blocking(move || -> Result<Value, ApiError> {
        let mut listed = library_tools::list(&env).map_err(ApiError::validation)?;
        let fonts = overlay_tools::fonts(&env).map_err(ApiError::internal)?;
        let luts: Vec<String> = std::fs::read_dir(&env.luts)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.to_lowercase().ends_with(".cube"))
            .collect();
        if let Some(map) = listed.as_object_mut() {
            let names: Vec<Value> = fonts.get("fonts").and_then(Value::as_array).into_iter().flatten().filter_map(|font| font.get("name").cloned()).collect();
            map.insert("fonts".into(), Value::Array(names));
            map.insert("fonts_folder".into(), fonts.get("folder").cloned().unwrap_or(Value::Null));
            map.insert("luts".into(), json!(luts));
            map.insert("caption_styles".into(), json!(TEXT_STYLES));
            map.insert("kinds".into(), json!(KINDS));
        }
        Ok(listed)
    })
    .await??;
    Ok(success(reply, None))
}

/// The overlay pass of a stitch: draws the project's overlays onto the stitched video, replacing the file, when the track is on. Nothing is reported for a
/// project without overlays. A failed pass leaves the plain video and says why, like the Builder's Export.
fn apply_to_stitched(env: &Env, project: &FsPath, video: &FsPath, offset: f64) -> Option<Value> {
    let loaded = load_session(project).ok()?;
    let view = overlay_project::overlay_view(&loaded.session);
    let clips = view.get("overlay_layers").and_then(Value::as_array).cloned().unwrap_or_default();
    if clips.is_empty() {
        return None;
    }
    if view["overlay_layer_track"]["enabled"] == Value::Bool(false) {
        return Some(json!({"applied": false, "reason": "The overlays are off."}));
    }
    let mut payload = Obj::new();
    payload.insert("input_path".into(), json!(video.display().to_string()));
    payload.insert("overlay_layers".into(), Value::Array(clips));
    payload.insert("replace_source".into(), json!(true));
    payload.insert("project_folder".into(), json!(project.display().to_string()));
    payload.insert("time_offset".into(), json!(offset));
    Some(match overlay_tools::render(env, &payload) {
        Ok(reply) => json!({
            "applied": reply["changed"] == Value::Bool(true), "clips": reply["applied"], "skipped": reply["skipped"], "elapsed_seconds": reply["elapsed_seconds"],
        }),
        Err(error) => {
            eprintln!("[VRGDG-RS Overlays] The overlay pass on {} failed, the plain video is kept: {error}", video.display());
            json!({"applied": false, "error": error})
        }
    })
}

/// The hook a stitch runs on its video (see `RunContext::after_stitch`).
pub fn stitch_hook(env: Env) -> StitchHook {
    Arc::new(move |project: &FsPath, video: &FsPath, summary: &StitchSummary| apply_to_stitched(&env, project, video, summary.timeline_offset))
}

