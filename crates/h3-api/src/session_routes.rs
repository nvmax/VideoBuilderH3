//! Routes for projects, scene fields, prompts, lyrics, references, audio and media files.

use std::collections::HashMap;
use std::path::PathBuf;

use axum::body::Body;
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use h3_media::beats::estimate_beats;
use h3_media::peaks::read_audio_peaks;
use h3_media::Tools;
use h3_project::scene_ops::OpError;
use h3_project::session_ops::{self, AudioInfo};
use h3_project::Stamp;
use serde_json::{json, Value};
use tower::ServiceExt;
use tower_http::services::ServeFile;

use crate::app::{blocking, body_object, if_match, load, Shared};
use crate::error::{respond, success, ApiError, ApiResult};
use crate::views;

fn op_error(e: OpError, pid: &str) -> ApiError {
    match e {
        OpError::SceneNotFound(id) => ApiError::scene_not_found(&id, pid),
        OpError::Validation(m) => ApiError::validation(m),
        OpError::Conflict { current, incoming } => ApiError::revision_conflict(current, incoming),
        OpError::Io(e) => ApiError::internal(e.to_string()),
    }
}

/// Run an edit of the project with edits serialized.
async fn edit(state: &crate::app::AppState, pid: &str, op: impl FnOnce(&std::path::Path, &Stamp) -> Result<Value, OpError> + Send + 'static) -> ApiResult {
    let folder = views::resolve_project(&state.roots, pid)?;
    let _guard = state.edit_lock.lock().await;
    let result = blocking(move || op(&folder, &Stamp::now())).await?.map_err(|e| op_error(e, pid))?;
    let revision = result.get("revision").and_then(Value::as_i64);
    Ok(respond(StatusCode::OK, result, revision))
}

// -- projects ---------------------------------------------------------------------------------------------------------------

pub async fn create_project(State(state): Shared, body: Option<Json<Value>>) -> ApiResult {
    let body = body_object(body)?;
    let name = body.get("name").and_then(Value::as_str).unwrap_or("").to_string();
    let root = state.cfg.project_root.clone();
    let (folder, revision) = blocking(move || session_ops::create_project(&root, &name, &Stamp::now())).await?.map_err(|e| op_error(e, ""))?;
    let id = views::project_id(&folder);
    Ok(respond(
        StatusCode::OK,
        json!({"project_id": id, "name": body.get("name").cloned().unwrap_or(Value::Null), "project_folder": folder.display().to_string(), "revision": revision}),
        Some(revision),
    ))
}

/// `POST /projects/{pid}/duplicate`: branch the project under `new_name`.
pub async fn duplicate_project(State(state): Shared, Path(pid): Path<String>, body: Option<Json<Value>>) -> ApiResult {
    let body = body_object(body)?;
    let name = body.get("new_name").and_then(Value::as_str).unwrap_or("").to_string();
    let source = views::resolve_project(&state.roots, &pid)?;
    let root = state.cfg.project_root.clone();
    let target = blocking(move || h3_project::duplicate::duplicate_project(&root, &source, &name, &Stamp::now())).await?.map_err(|e| op_error(e, &pid))?;
    Ok(respond(StatusCode::CREATED, json!({"project_id": views::project_id(&target), "project_folder": target.display().to_string(), "revision": 1}), Some(1)))
}

pub async fn delete_project(State(state): Shared, Path(pid): Path<String>, Query(q): Query<HashMap<String, String>>) -> ApiResult {
    if q.get("confirm").map(|c| c.trim()) != Some(pid.trim()) {
        return Err(ApiError::validation(format!("Confirm parameter must match project_id '{pid}'.")));
    }
    let folder = views::resolve_project(&state.roots, &pid)?;
    let _guard = state.edit_lock.lock().await;
    blocking(move || session_ops::delete_project(&folder)).await?.map_err(|e| op_error(e, &pid))?;
    Ok(success(json!({"deleted": true, "project_id": pid}), None))
}

pub async fn list_finals(State(state): Shared, Path(pid): Path<String>) -> ApiResult {
    let folder = views::resolve_project(&state.roots, &pid)?;
    let finals = blocking(move || {
        let mut found: Vec<(f64, Value)> = std::fs::read_dir(&folder)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("mp4")))
            .filter_map(|e| {
                let meta = e.metadata().ok()?;
                let modified = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs_f64();
                Some((modified, json!({"filename": e.file_name().to_string_lossy(), "path": e.path().display().to_string(), "bytes": meta.len(), "mtime": modified as i64})))
            })
            .collect();
        found.sort_by(|a, b| b.0.total_cmp(&a.0));
        found.into_iter().map(|(_, v)| v).collect::<Vec<_>>()
    })
    .await?;
    Ok(success(Value::Array(finals), None))
}

// -- scene fields and prompts -----------------------------------------------------------------------------------------------

pub async fn patch_scene(State(state): Shared, Path((pid, sid)): Path<(String, String)>, headers: HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    let patch = body_object(body)?;
    let expected = if_match(&headers)?;
    edit(&state, &pid, move |folder, now| session_ops::patch_scene(folder, &sid, &patch, expected, now)).await
}

pub async fn put_prompt(State(state): Shared, Path((pid, sid, field)): Path<(String, String, String)>, headers: HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    let body = body_object(body)?;
    let prompt = body.get("prompt").and_then(Value::as_str).unwrap_or("").to_string();
    let origin = body.get("origin").and_then(Value::as_str).unwrap_or("agent").to_string();
    let expected = if_match(&headers)?;
    edit(&state, &pid, move |folder, now| session_ops::set_scene_prompt(folder, &sid, &field, &prompt, &origin, expected, now)).await
}

/// Check a MiniMax prompt against the length, reference and format rules. Port of `validate_minimax_prompt_endpoint`.
pub async fn validate_prompt(State(state): Shared, Path((pid, sid)): Path<(String, String)>, body: Option<Json<Value>>) -> ApiResult {
    let body = body_object(body)?;
    let loaded = load(&state, &pid).await?;
    let idx = views::find_scene(&loaded, &sid).ok();
    let scene = idx.map(|i| views::segments(&loaded.session)[i].clone());
    let prompt = body.get("prompt").and_then(Value::as_str).unwrap_or("").to_string();
    let given = body.get("mode").and_then(Value::as_str).map(str::to_string);
    let resolved = h3_core::prompt::assembly::effective_mode(&scene.clone().unwrap_or_default(), &loaded.session, given.as_deref());
    let result = h3_core::prompt::assembly::validate_minimax_h3_prompt(&prompt, scene.as_ref(), Some(resolved), true);
    Ok(success(result, None))
}

/// `GET /projects/{pid}/scenes/{sid}/prompts/context`: what an outside agent needs to write this scene's shots.
pub async fn prompt_context(State(state): Shared, Path((pid, sid)): Path<(String, String)>) -> ApiResult {
    let loaded = load(&state, &pid).await?;
    let idx = views::find_scene(&loaded, &sid)?;
    let scene = views::segments(&loaded.session)[idx].clone();
    let session = loaded.session.clone();
    let context = blocking(move || h3_core::prompt::assembly::build_minimax_prompt_context(&scene, &session, None)).await?;
    Ok(success(Value::Object(context), None))
}

/// `POST /projects/{pid}/scenes/{sid}/prompts/minimax/assemble`: build the official prompt from shot descriptions.
pub async fn assemble_prompt(State(state): Shared, Path((pid, sid)): Path<(String, String)>, headers: HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    let body = body_object(body)?;
    let shots: Vec<String> = body.get("shots").and_then(Value::as_array).map(|a| a.iter().map(|v| h3_core::pyval::py_str(Some(v))).collect()).unwrap_or_default();
    let mode = body.get("mode").and_then(Value::as_str).map(str::to_string);
    let save = body.get("save").is_some_and(h3_core::pyval::truthy);
    let expected = if_match(&headers)?;
    edit(&state, &pid, move |folder, now| session_ops::assemble_scene_prompt(folder, &sid, &shots, mode.as_deref(), save, expected, now)).await
}

// -- lyrics -----------------------------------------------------------------------------------------------------------------

pub async fn get_lyrics(State(state): Shared, Path(pid): Path<String>) -> ApiResult {
    let loaded = load(&state, &pid).await?;
    let mut text = h3_core::pyval::text_or_empty(loaded.session.get("canonical_lyrics"));
    if text.is_empty() {
        for candidate in [loaded.project_folder.join("project_context").join("full_lyrics.txt"), loaded.project_folder.join("full_lyrics.txt")] {
            if let Ok(found) = std::fs::read_to_string(&candidate) {
                if !found.trim().is_empty() {
                    text = found;
                    break;
                }
            }
        }
    }
    let srt = std::fs::read_to_string(h3_project::layout::srt_path(&loaded.project_folder)).unwrap_or_default();
    Ok(success(json!({"lyrics_text": text, "srt_text": srt, "has_srt": !srt.trim().is_empty()}), Some(loaded.revision)))
}

pub async fn put_lyrics(State(state): Shared, Path(pid): Path<String>, headers: HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    let body = body_object(body)?;
    let text = body.get("lyrics_text").and_then(Value::as_str).map(str::to_string);
    let expected = if_match(&headers)?;
    edit(&state, &pid, move |folder, now| session_ops::set_lyrics(folder, text.as_deref(), expected, now)).await
}

// -- references -------------------------------------------------------------------------------------------------------------

pub async fn put_reference(State(state): Shared, Path((pid, kind, rid)): Path<(String, String, String)>, headers: HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    let payload = body_object(body)?;
    let expected = if_match(&headers)?;
    edit(&state, &pid, move |folder, now| session_ops::upsert_reference(folder, &kind, &rid, &payload, expected, now)).await
}

pub async fn delete_reference(State(state): Shared, Path((pid, kind, rid)): Path<(String, String, String)>, headers: HeaderMap) -> ApiResult {
    let expected = if_match(&headers)?;
    edit(&state, &pid, move |folder, now| session_ops::delete_reference(folder, &kind, &rid, expected, now)).await
}

pub async fn get_scene_mapping(State(state): Shared, Path(pid): Path<String>) -> ApiResult {
    let mut loaded = load(&state, &pid).await?;
    Ok(success(json!({"scene_mapping": session_ops::scene_mapping_view(&mut loaded.session)}), Some(loaded.revision)))
}

pub async fn put_scene_mapping(State(state): Shared, Path(pid): Path<String>, headers: HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    let mapping = body_object(body)?;
    let expected = if_match(&headers)?;
    edit(&state, &pid, move |folder, now| session_ops::update_scene_mapping(folder, &mapping, expected, now)).await
}

/// `GET .../story`: the saved story layer as it is.
pub async fn get_story(State(state): Shared, Path(pid): Path<String>) -> ApiResult {
    let loaded = load(&state, &pid).await?;
    Ok(success(h3_project::story::saved_story(&loaded.session), None))
}

/// `PUT .../story`: replace the story layer. The answer is the story, with the new revision in the envelope.
pub async fn put_story(State(state): Shared, Path(pid): Path<String>, headers: HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    let story = Value::Object(body_object(body)?);
    let expected = if_match(&headers)?;
    let folder = views::resolve_project(&state.roots, &pid)?;
    let _guard = state.edit_lock.lock().await;
    let result = blocking(move || h3_project::story::put_story(&folder, &story, expected, &Stamp::now())).await?.map_err(|e| op_error(e, &pid))?;
    let revision = result.get("revision").and_then(Value::as_i64);
    Ok(respond(StatusCode::OK, result["story"].clone(), revision))
}

/// Save the Storyboard scene defaults and the story idea. Body: `{"defaults": {...}, "story": {...}}`.
pub async fn story_settings(State(state): Shared, Path(pid): Path<String>, headers: HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    let params = body_object(body)?;
    let expected = if_match(&headers)?;
    edit(&state, &pid, move |folder, now| h3_project::story::set_story_settings(folder, &params, expected, now)).await
}

/// The story steps (arc, brief, scene beats, Story Mode) write with the Storyboard Builder, which this app does not have.
pub async fn story_step(Path((_pid, step)): Path<(String, String)>) -> ApiResult {
    Err(ApiError::new(
        StatusCode::NOT_IMPLEMENTED,
        "NOT_SUPPORTED",
        format!("The story step '{step}' is part of the Storyboard Builder, which this app does not include. Save the story idea with PUT story/settings and write scene prompts with minimax-prompts."),
    ))
}

/// Assign characters and locations to scenes by pattern. `dry_run` returns the plan and saves nothing.
pub async fn assign_scenes(State(state): Shared, Path(pid): Path<String>, headers: HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    let body = body_object(body)?;
    let expected = if_match(&headers)?;
    let text = |k: &str, default: &str| body.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or(default).to_string();
    let int = |k: &str, default: i64| body.get(k).and_then(h3_core::pyval::to_int).filter(|n| *n != 0).unwrap_or(default);
    let flag = |k: &str, default: bool| body.get(k).map_or(default, h3_core::pyval::truthy);
    let params = h3_project::assignment::AssignParams {
        scope: text("scope", "all"),
        range_start: int("range_start", 1),
        range_end: body.get("range_end").and_then(h3_core::pyval::to_int),
        scene_ids: body.get("scene_ids").and_then(Value::as_array).map(|a| a.iter().map(|v| h3_core::pyval::py_str(Some(v))).collect()).unwrap_or_default(),
        character_pattern: text("character_pattern", "unchanged"),
        character_block_size: int("character_block_size", 10),
        location_pattern: text("location_pattern", "unchanged"),
        location_block_size: int("location_block_size", 4),
        replace_existing: flag("replace_existing", false),
        avoid_location_repeat: flag("avoid_location_repeat", true),
        seed: body.get("seed").and_then(Value::as_u64),
    };
    let dry_run = flag("dry_run", false);
    edit(&state, &pid, move |folder, now| h3_project::assignment::assign_scenes(folder, &params, dry_run, expected, now)).await
}

// -- audio ------------------------------------------------------------------------------------------------------------------

fn tools(state: &crate::app::AppState) -> Result<Tools, ApiError> {
    Tools::find(&state.cfg.ffmpeg_path, &state.cfg.ffprobe_path).map_err(|e| ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "FFMPEG_FAILED", e.to_string()))
}

/// Copy or decode the song into `project_audio/` and analyse it. An `.m4a` is converted to WAV, as the pack does.
pub(crate) fn save_project_audio(tools: &Tools, folder: &std::path::Path, source: Option<PathBuf>, data: Option<(Vec<u8>, String)>, name: &str) -> Result<AudioInfo, ApiError> {
    let dir = folder.join("project_audio");
    std::fs::create_dir_all(&dir).map_err(|e| ApiError::internal(e.to_string()))?;
    let ext = std::path::Path::new(name).extension().map(|e| format!(".{}", e.to_string_lossy().to_lowercase())).filter(|e| matches!(e.as_str(), ".wav" | ".mp3" | ".flac" | ".m4a" | ".ogg")).unwrap_or_else(|| ".wav".into());
    let target = dir.join(format!("project_audio{}", if ext == ".m4a" { ".wav" } else { &ext }));
    let raw = if ext == ".m4a" { dir.join("project_audio_source.m4a") } else { target.clone() };
    match (data, source) {
        (Some((bytes, _)), _) => std::fs::write(&raw, bytes).map_err(|e| ApiError::internal(e.to_string()))?,
        (None, Some(path)) => {
            if !path.is_file() {
                return Err(ApiError::validation(format!("Audio file not found: {}", path.display())));
            }
            std::fs::copy(&path, &raw).map_err(|e| ApiError::internal(e.to_string()))?;
        }
        (None, None) => return Err(ApiError::validation("Send audio_path or audio_data.")),
    }
    if ext == ".m4a" {
        let args: Vec<String> = ["-hide_banner", "-loglevel", "error", "-y", "-i", &raw.display().to_string(), "-vn", "-acodec", "pcm_s16le", "-ar", "44100", "-ac", "2", &target.display().to_string()].iter().map(|s| s.to_string()).collect();
        tools.ffmpeg_checked(&args, "Could not convert audio to WAV").map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "FFMPEG_FAILED", e.to_string()))?;
        let _ = std::fs::remove_file(&raw);
    }
    let info = read_audio_peaks(tools, &target, 1600).map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "FFMPEG_FAILED", format!("Could not read audio for waveform: {e}")))?;
    let (beats, tempo) = estimate_beats(tools, &target, &info.peaks, info.duration);
    Ok(AudioInfo { saved_path: target.display().to_string(), duration: info.duration, peaks: info.peaks, beats, tempo_bpm: tempo })
}

/// Attach the song: a file path (`audio_path`) or uploaded data (`audio_data`, `audio_name`).
pub async fn put_audio(State(state): Shared, Path(pid): Path<String>, headers: HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    let body = body_object(body)?;
    let expected = if_match(&headers)?;
    let folder = views::resolve_project(&state.roots, &pid)?;
    let tools = tools(&state)?;
    let source = body.get("audio_path").and_then(Value::as_str).map(str::trim).filter(|p| !p.is_empty()).map(PathBuf::from);
    let name = body.get("audio_name").and_then(Value::as_str).map(str::to_string).or_else(|| source.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned())).unwrap_or_else(|| "audio.wav".into());
    let data = match body.get("audio_data").and_then(Value::as_str).filter(|d| !d.trim().is_empty()) {
        Some(d) => Some(h3_project::images::decode_image_data(d).map_err(|e| ApiError::validation(e.to_string()))?),
        None => None,
    };
    let _guard = state.edit_lock.lock().await;
    let analysed = blocking({
        let folder = folder.clone();
        move || save_project_audio(&tools, &folder, source, data, &name)
    })
    .await??;
    let result = blocking(move || session_ops::attach_audio(&folder, &analysed, expected, &Stamp::now())).await?.map_err(|e| op_error(e, &pid))?;
    let revision = result.get("revision").and_then(Value::as_i64);
    Ok(respond(StatusCode::OK, result, revision))
}

/// A silent 16 bit stereo WAV of `duration` seconds at 44.1 kHz.
fn write_silent_wav(path: &std::path::Path, duration: f64) -> std::io::Result<()> {
    use std::io::Write;
    const RATE: u32 = 44100;
    const CHANNELS: u16 = 2;
    let frames = (duration * f64::from(RATE)).round_ties_even() as u64;
    let data_bytes = (frames * u64::from(CHANNELS) * 2) as u32;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::io::BufWriter::new(std::fs::File::create(path)?);
    file.write_all(b"RIFF")?;
    file.write_all(&(36 + data_bytes).to_le_bytes())?;
    file.write_all(b"WAVEfmt ")?;
    file.write_all(&16u32.to_le_bytes())?;
    file.write_all(&1u16.to_le_bytes())?;
    file.write_all(&CHANNELS.to_le_bytes())?;
    file.write_all(&RATE.to_le_bytes())?;
    file.write_all(&(RATE * u32::from(CHANNELS) * 2).to_le_bytes())?;
    file.write_all(&(CHANNELS * 2).to_le_bytes())?;
    file.write_all(&16u16.to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(&data_bytes.to_le_bytes())?;
    let second = vec![0u8; (RATE as usize) * usize::from(CHANNELS) * 2];
    let mut remaining = u64::from(data_bytes);
    while remaining > 0 {
        let count = remaining.min(second.len() as u64) as usize;
        file.write_all(&second[..count])?;
        remaining -= count as u64;
    }
    file.flush()
}

/// `POST .../audio/silent`: use a silent track of `duration` seconds as the project's audio, for a project without a song.
pub async fn silent_audio(State(state): Shared, Path(pid): Path<String>, headers: HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    let body = body_object(body)?;
    let expected = if_match(&headers)?;
    let duration = body.get("duration").filter(|v| !v.is_null()).map_or(Some(10.0), h3_core::pyval::to_float).filter(|d| d.is_finite() && *d > 0.0).ok_or_else(|| ApiError::validation("Silence duration must be greater than 0 seconds."))?.clamp(0.1, 86400.0);
    let scope = body.get("scope").and_then(Value::as_str).unwrap_or("project").trim().to_lowercase();
    if scope == "scene" {
        return Err(ApiError::validation("Silence for a single scene is only used by Speaking scenes, which this app does not have. Use scope 'project'."));
    }
    let folder = views::resolve_project(&state.roots, &pid)?;
    let tools = tools(&state)?;
    let label = format!("{duration:.2}").trim_end_matches('0').trim_end_matches('.').replace('.', "_");
    let _guard = state.edit_lock.lock().await;
    let analysed = blocking({
        let folder = folder.clone();
        move || -> Result<AudioInfo, ApiError> {
            let target = folder.join("project_audio").join(format!("project_silence_{label}s.wav"));
            write_silent_wav(&target, duration).map_err(|e| ApiError::internal(e.to_string()))?;
            let info = read_audio_peaks(&tools, &target, 1600).map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "FFMPEG_FAILED", e.to_string()))?;
            Ok(AudioInfo { saved_path: target.display().to_string(), duration: info.duration, peaks: info.peaks, beats: vec![], tempo_bpm: 0.0 })
        }
    })
    .await??;
    let result = blocking(move || session_ops::attach_audio(&folder, &analysed, expected, &Stamp::now())).await?.map_err(|e| op_error(e, &pid))?;
    let revision = result.get("revision").and_then(Value::as_i64);
    Ok(respond(StatusCode::OK, result, revision))
}

pub async fn get_waveform(State(state): Shared, Path(pid): Path<String>, Query(q): Query<HashMap<String, String>>) -> ApiResult {
    let loaded = load(&state, &pid).await?;
    let audio = std::path::PathBuf::from(h3_core::pyval::text_or_empty(loaded.session.get("audio_path")));
    let target = q.get("peaks").and_then(|p| p.parse::<usize>().ok()).unwrap_or(1600).clamp(8, 20000);
    if !audio.is_file() {
        return Ok(success(json!({"peaks": [], "duration": 0.0}), Some(loaded.revision)));
    }
    // The saved peaks serve the default size. Other sizes are cut from the file.
    if target == 1600 {
        if let Some(saved) = loaded.session.get("audio_peaks").filter(|p| p.as_array().is_some_and(|a| !a.is_empty())) {
            let duration = loaded.session.get("audio_duration").cloned().unwrap_or(json!(0.0));
            return Ok(success(json!({"peaks": saved, "duration": duration}), Some(loaded.revision)));
        }
    }
    let tools = tools(&state)?;
    let info = blocking(move || read_audio_peaks(&tools, &audio, target)).await?.map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "FFMPEG_FAILED", e.to_string()))?;
    Ok(success(json!({"peaks": info.peaks, "duration": info.duration}), Some(loaded.revision)))
}

pub async fn get_beats(State(state): Shared, Path(pid): Path<String>) -> ApiResult {
    let loaded = load(&state, &pid).await?;
    let tempo = ["detected_tempo_bpm", "tempo_bpm", "bpm"].iter().filter_map(|k| loaded.session.get(*k)).find(|v| h3_core::pyval::truthy(v)).cloned().unwrap_or(Value::Null);
    Ok(success(json!({"beats": session_ops::session_beats(&loaded.session), "tempo_bpm": tempo, "beat_calibration": loaded.session.get("beat_calibration").cloned().unwrap_or(Value::Null)}), Some(loaded.revision)))
}

pub async fn put_beats(State(state): Shared, Path(pid): Path<String>, headers: HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    let body = body_object(body)?;
    let beats = body.get("beats").and_then(Value::as_array).cloned().ok_or_else(|| ApiError::validation("beats must be a list."))?;
    let tempo = body.get("tempo_bpm").and_then(h3_core::pyval::to_float);
    let expected = if_match(&headers)?;
    edit(&state, &pid, move |folder, now| session_ops::set_beats(folder, beats, tempo, expected, now)).await
}

pub async fn calibrate(State(state): Shared, Path(pid): Path<String>, headers: HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    let body = body_object(body)?;
    let offset = body.get("offset_seconds").and_then(h3_core::pyval::to_float).unwrap_or(0.0);
    let expected = if_match(&headers)?;
    edit(&state, &pid, move |folder, now| session_ops::calibrate_beats(folder, offset, expected, now)).await
}

// -- media files ------------------------------------------------------------------------------------------------------------

/// Serve a file from a project root or ComfyUI's output folder, with range requests so a video can be scrubbed.
pub async fn serve_file(State(state): Shared, Query(q): Query<HashMap<String, String>>, request: Request<Body>) -> Response {
    let Some(wanted) = q.get("path").filter(|p| !p.trim().is_empty()) else {
        return ApiError::validation("path is required.").into_response();
    };
    let Ok(path) = std::fs::canonicalize(wanted.trim()) else {
        return ApiError::new(StatusCode::NOT_FOUND, "ASSET_NOT_FOUND", "File not found.").into_response();
    };
    let mut roots: Vec<PathBuf> = state.roots.clone();
    if !state.cfg.comfy_output_dir.trim().is_empty() {
        roots.push(PathBuf::from(state.cfg.comfy_output_dir.trim()));
    }
    let allowed = roots.iter().filter_map(|r| std::fs::canonicalize(r).ok()).any(|r| path.starts_with(&r));
    if !allowed || !path.is_file() {
        return ApiError::new(StatusCode::FORBIDDEN, "PATH_OUTSIDE_ROOT", "That file is outside the project folders.").into_response();
    }
    match ServeFile::new(path).oneshot(request).await {
        Ok(response) => response.into_response(),
        Err(e) => ApiError::internal(e.to_string()).into_response(),
    }
}

/// `GET /projects/{pid}/assets`: the media files of the project.
pub async fn project_assets(State(state): Shared, Path(pid): Path<String>) -> ApiResult {
    let folder = views::resolve_project(&state.roots, &pid)?;
    let items = blocking(move || views::project_assets(&folder)).await?;
    Ok(success(json!({"items": items, "count": items.len()}), None))
}

/// `POST /projects/{pid}/export`: package the project as a portable zip next to the app's config.
pub async fn export_project(State(state): Shared, Path(pid): Path<String>) -> ApiResult {
    let folder = views::resolve_project(&state.roots, &pid)?;
    let out = state.config_path.parent().unwrap_or_else(|| std::path::Path::new(".")).join("exports");
    let now = Stamp::now();
    let (zip, name) = blocking(move || h3_project::export::export_project(&folder, &out, &now)).await?.map_err(ApiError::internal)?;
    let bytes = std::fs::metadata(&zip).map(|m| m.len()).unwrap_or(0);
    Ok(success(json!({"zip_path": zip.display().to_string(), "filename": name, "bytes": bytes}), None))
}

/// `POST /projects/{pid}/scenes/bulk`: move, resize and patch several scenes in one save.
pub async fn bulk_scenes(State(state): Shared, Path(pid): Path<String>, headers: HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    let body = body_object(body)?;
    let operations: Vec<Value> = body.get("operations").and_then(Value::as_array).cloned().unwrap_or_default();
    let expected = if_match(&headers)?;
    edit(&state, &pid, move |folder, now| session_ops::bulk_scene_ops(folder, &operations, expected, now)).await
}
