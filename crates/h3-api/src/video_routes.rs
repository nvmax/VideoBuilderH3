//! A scene's video after it is rendered: raw takes, re-trim, choosing or recovering a clip, clearing it and matching its
//! opening color. Port of the matching routes and jobs in `agent_api/router.py` and
//! `agent_api/orchestrator/video_orchestrator.py`.

#![allow(clippy::result_large_err)]

use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use h3_core::pyval::{text_or_empty, to_float, truthy, Obj};
use h3_jobs::{Handler, JobCtx, JobError, ProgressUpdate};
use h3_media::video::{apply_color_match, probe_format_duration, restore_scene_video, trim_scene_video, Restored, TrimRequest};
use h3_media::Tools;
use h3_pipeline::video_ops::{find_scene_video_output, list_scene_takes, resolve_scene_take, VideoOpError};
use h3_pipeline::scene_video::apply_scene_video;
use h3_project::{load_session, persist_session, takes, Stamp};
use serde_json::{json, Value};

use crate::app::{blocking, body_object, load, Shared};
use crate::error::{success, ApiError, ApiResult};
use crate::job_handlers::fail;
use crate::views::resolve_project;

pub const VIDEO_TRIM: &str = "video.trim";
pub const VIDEO_MATCH_START_COLOR: &str = "video.match_start_color";
const TAKE_FPS: f64 = 24.0;

type Run = fn(Arc<crate::app::AppState>, JobCtx) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, JobError>> + Send>>;

pub fn register(state: &Arc<crate::app::AppState>) {
    let table: [(&str, Run); 2] = [(VIDEO_TRIM, |s, c| Box::pin(trim_job(s, c))), (VIDEO_MATCH_START_COLOR, |s, c| Box::pin(match_color_job(s, c)))];
    for (kind, run) in table {
        let owned = Arc::clone(state);
        let handler: Handler = Arc::new(move |ctx| run(Arc::clone(&owned), ctx));
        state.jobs.register(kind, handler);
    }
}

fn op_error(e: VideoOpError, pid: &str) -> ApiError {
    match e {
        VideoOpError::SceneNotFound(id) => ApiError::scene_not_found(&id, pid),
        VideoOpError::Invalid(m) => ApiError::validation(m),
    }
}

fn tools(state: &crate::app::AppState) -> Result<Tools, ApiError> {
    Tools::find(&state.cfg.ffmpeg_path, &state.cfg.ffprobe_path).map_err(|e| ApiError::new(axum::http::StatusCode::SERVICE_UNAVAILABLE, "FFMPEG_FAILED", e.to_string()))
}

async fn output_dir(state: &crate::app::AppState) -> Result<String, ApiError> {
    h3_pipeline::context::comfy_dirs(&state.cfg).await.map(|d| d.output).map_err(|e| ApiError::new(axum::http::StatusCode::SERVICE_UNAVAILABLE, "COMFY_UNAVAILABLE", e).retryable())
}

/// `segment.get("id") == scene or str(i + 1) == str(scene)`, whichever scene matches first.
fn scene_index(segments: &[Value], scene: &str) -> Option<usize> {
    segments.iter().enumerate().position(|(i, s)| text_or_empty(s.get("id")) == scene || (i + 1).to_string() == scene)
}

fn segments_of(session: &Obj) -> Vec<Value> {
    session.get("segments").and_then(Value::as_array).cloned().unwrap_or_default()
}

/// Record `video` on the scene like a render does, in a session read just now.
fn record_video(project: &FsPath, scene: &str, video: &str, thumbnail: &str) -> Result<i64, ApiError> {
    let mut session = load_session(project).map_err(|e| ApiError::internal(e.to_string()))?.session;
    let mut segments = segments_of(&session);
    let index = scene_index(&segments, scene).ok_or_else(|| ApiError::validation(format!("Scene '{scene}' was not found.")))?;
    let segment = segments[index].as_object_mut().ok_or_else(|| ApiError::internal("The scene is not an object."))?;
    apply_scene_video(segment, video, thumbnail, (Stamp::now().epoch * 1000.0) as i64);
    session.insert("segments".into(), Value::Array(segments));
    Ok(persist_session(project, session, Stamp::now()).map_err(|e| ApiError::internal(e.to_string()))?.revision)
}

/// `GET .../video/takes`: the scene's raw renders.
pub async fn takes_route(State(state): Shared, Path((pid, sid)): Path<(String, String)>) -> ApiResult {
    let loaded = load(&state, &pid).await?;
    let folder = resolve_project(&state.roots, &pid)?;
    let tools = tools(&state)?;
    let dir = output_dir(&state).await?;
    let listing = blocking(move || list_scene_takes(&tools, &dir, &folder, &loaded.session, &sid)).await?.map_err(|e| op_error(e, &pid))?;
    Ok(success(listing, None))
}

/// `POST .../video/select`: make a recorded clip or backup the scene's clip.
pub async fn select_route(State(state): Shared, Path((pid, sid)): Path<(String, String)>, body: Option<Json<Value>>) -> ApiResult {
    let body = body_object(body)?;
    let source = text_or_empty(body.get("source_path")).trim().to_string();
    if source.is_empty() {
        return Err(ApiError::validation("source_path is required."));
    }
    if !FsPath::new(&source).is_file() {
        return Err(ApiError::validation(format!("Invalid video source path: {source}")));
    }
    restore(&state, &pid, &sid, PathBuf::from(source)).await
}

/// `POST .../video/recover`: link a finished but uncollected clip, named or found in the project's old scratch folders.
pub async fn recover_route(State(state): Shared, Path((pid, sid)): Path<(String, String)>, body: Option<Json<Value>>) -> ApiResult {
    let body = body_object(body)?;
    let given = text_or_empty(body.get("source_path")).trim().to_string();
    let source = if given.is_empty() {
        let loaded = load(&state, &pid).await?;
        let folder = resolve_project(&state.roots, &pid)?;
        let segments = segments_of(&loaded.session);
        let index = scene_index(&segments, &sid).ok_or_else(|| ApiError::scene_not_found(&sid, &pid))?;
        let mode = Some(text_or_empty(loaded.session.get("video_mode"))).filter(|m| !m.is_empty()).unwrap_or_else(|| "i2v".into());
        find_scene_video_output(&folder, &mode, index as u32 + 1).map(|p| p.display().to_string()).unwrap_or_default()
    } else {
        given
    };
    if source.is_empty() || !FsPath::new(&source).is_file() {
        return Err(ApiError::validation(format!("No recoverable scene video found for scene '{sid}'.")));
    }
    restore(&state, &pid, &sid, PathBuf::from(source)).await
}

async fn restore(state: &crate::app::AppState, pid: &str, sid: &str, source: PathBuf) -> ApiResult {
    let loaded = load(state, pid).await?;
    let folder = resolve_project(&state.roots, pid)?;
    let index = scene_index(&segments_of(&loaded.session), sid).ok_or_else(|| ApiError::scene_not_found(sid, pid))?;
    let tools = tools(state)?;
    let _guard = state.edit_lock.lock().await;
    let (scene, project) = (sid.to_string(), folder.clone());
    let result = blocking(move || -> Result<Value, ApiError> {
        let stamp = Stamp::now().compact.replace('_', "-");
        match restore_scene_video(&tools, &project, &source, index as u32 + 1, 0.0, 0.5, false, &stamp).map_err(|e| ApiError::internal(e.to_string()))? {
            Restored::NeedsConfirmation { .. } => Err(ApiError::internal("The clip's length was checked without an expected length.")),
            Restored::Done { video_path, thumbnail_path, .. } => {
                let video = video_path.display().to_string();
                let thumbnail = thumbnail_path.map(|p| p.display().to_string()).unwrap_or_default();
                let revision = record_video(&project, &scene, &video, &thumbnail)?;
                Ok(json!({"scene_id": scene, "video_path": video, "thumbnail_path": thumbnail, "revision": revision}))
            }
        }
    })
    .await??;
    let revision = result.get("revision").and_then(Value::as_i64);
    Ok(crate::error::respond(axum::http::StatusCode::OK, result, revision))
}

/// `DELETE .../video`: clear the scene's clip. The files stay on disk.
pub async fn delete_route(State(state): Shared, Path((pid, sid)): Path<(String, String)>, _headers: HeaderMap) -> ApiResult {
    let folder = resolve_project(&state.roots, &pid)?;
    let _guard = state.edit_lock.lock().await;
    let (scene, project) = (sid.clone(), folder);
    let revision = blocking(move || -> Result<i64, ApiError> {
        let mut session = load_session(&project).map_err(|e| ApiError::internal(e.to_string()))?.session;
        let mut segments = segments_of(&session);
        let index = scene_index(&segments, &scene).ok_or_else(|| ApiError::scene_not_found(&scene, &project.display().to_string()))?;
        if let Some(segment) = segments[index].as_object_mut() {
            for key in ["video_path", "rendered_video_path", "thumbnail_path", "video_thumbnail_path", "video_status"] {
                segment.insert(key.into(), json!(""));
            }
            if segment.get("preview_mode").and_then(Value::as_str) == Some("video") {
                segment.insert("preview_mode".into(), json!("image"));
            }
        }
        session.insert("segments".into(), Value::Array(segments));
        Ok(persist_session(&project, session, Stamp::now()).map_err(|e| ApiError::internal(e.to_string()))?.revision)
    })
    .await??;
    Ok(crate::error::respond(axum::http::StatusCode::OK, json!({"scene_id": sid, "cleared": true, "revision": revision}), Some(revision)))
}

/// `POST .../video/trim`: cut a take or the scene's clip to a start and length, as a job.
pub async fn trim_route(State(state): Shared, Path((pid, sid)): Path<(String, String)>, body: Option<Json<Value>>) -> ApiResult {
    submit(&state, VIDEO_TRIM, &pid, &sid, body).await
}

/// `POST .../video/match-start-color`: match the clip's opening color to the previous scene's clip, as a job.
pub async fn match_color_route(State(state): Shared, Path((pid, sid)): Path<(String, String)>, body: Option<Json<Value>>) -> ApiResult {
    submit(&state, VIDEO_MATCH_START_COLOR, &pid, &sid, body).await
}

async fn submit(state: &crate::app::AppState, kind: &str, pid: &str, sid: &str, body: Option<Json<Value>>) -> ApiResult {
    let mut params = body_object(body)?;
    params.insert("scene_id".into(), json!(sid));
    load(state, pid).await?;
    // Both only run ffmpeg, so they do not wait behind a render.
    let job = state.jobs.submit(kind, Some(pid), Value::Object(params), false)?;
    Ok(crate::app::job_response(&job))
}

// -- jobs -------------------------------------------------------------------------------------------------------------------

fn internal(e: impl std::fmt::Display) -> JobError {
    fail(ApiError::internal(e.to_string()))
}

fn invalid(message: impl Into<String>) -> JobError {
    fail(ApiError::validation(message))
}

fn param_f64(params: &Value, key: &str, default: f64) -> f64 {
    params.get(key).filter(|v| !v.is_null()).and_then(to_float).unwrap_or(default)
}

async fn session_of(folder: &FsPath) -> Result<Obj, JobError> {
    let folder = folder.to_path_buf();
    Ok(tokio::task::spawn_blocking(move || load_session(&folder)).await.map_err(internal)?.map_err(internal)?.session)
}

fn job_tools(state: &crate::app::AppState) -> Result<Tools, JobError> {
    tools(state).map_err(fail)
}

async fn trim_job(state: Arc<crate::app::AppState>, ctx: JobCtx) -> Result<Value, JobError> {
    let pid = ctx.project_id.clone().ok_or_else(|| invalid("project_id is required."))?;
    let scene = text_or_empty(ctx.params.get("scene_id")).trim().to_string();
    if scene.is_empty() {
        return Err(invalid("scene_id is required in params."));
    }
    let folder = resolve_project(&state.roots, &pid).map_err(fail)?;
    let session = session_of(&folder).await?;
    let segments = segments_of(&session);
    let index = scene_index(&segments, &scene).ok_or_else(|| fail(ApiError::scene_not_found(&scene, &pid)))?;
    let segment = &segments[index];
    let tools = job_tools(&state)?;
    let take = ctx.params.get("take").filter(|v| !v.is_null());
    let given_source = text_or_empty(ctx.params.get("source_path")).trim().to_string();
    let from_take = take.is_some() && given_source.is_empty();
    let source = if from_take {
        let dir = output_dir(&state).await.map_err(fail)?;
        let (t, f, s, sc) = (tools.clone(), folder.clone(), session.clone(), scene.clone());
        let listing = tokio::task::spawn_blocking(move || list_scene_takes(&t, &dir, &f, &s, &sc)).await.map_err(internal)?.map_err(|e| fail(op_error(e, &pid)))?;
        resolve_scene_take(&listing, &scene, take.unwrap_or(&Value::Null)).map_err(|e| fail(op_error(e, &pid)))?
    } else if !given_source.is_empty() {
        given_source
    } else {
        ["video_path", "rendered_video_path"].iter().map(|k| text_or_empty(segment.get(*k)).trim().to_string()).find(|p| !p.is_empty()).unwrap_or_default()
    };
    if source.is_empty() || !FsPath::new(&source).is_file() {
        return Err(invalid(format!("No source video found to trim for scene '{scene}'.")));
    }
    let start = param_f64(&ctx.params, "start", 0.0);
    let mut duration = param_f64(&ctx.params, "duration", 0.0);
    let mut frames = param_f64(&ctx.params, "frames", 0.0) as u32;
    if ctx.params.get("to_end").is_some_and(truthy) && duration == 0.0 {
        let (t, s) = (tools.clone(), PathBuf::from(&source));
        let total = tokio::task::spawn_blocking(move || probe_format_duration(&t, &s)).await.map_err(internal)?;
        duration = (total - start - 0.01).max(0.05);
        if frames == 0 {
            frames = (duration * TAKE_FPS) as u32;
        }
        duration = f64::from(frames) / TAKE_FPS;
    }
    if duration == 0.0 {
        duration = 4.0;
    }
    ctx.progress(ProgressUpdate { percent: 20.0, stage: "trimming".into(), message: "Executing video trim...".into(), scene_id: Some(scene.clone()), stage_count: 1, ..Default::default() });
    let label = Some(text_or_empty(ctx.params.get("label")).trim().to_string()).filter(|l| !l.is_empty()).unwrap_or_else(|| if from_take { "retrim".into() } else { "trim".into() });
    let mark = ctx.params.get("mark_as_audio_video").map_or(from_take, truthy);
    let request = TrimRequest { project: folder.clone(), scene: index as u32 + 1, source: PathBuf::from(&source), start, duration, frames, label, mark_as_audio_video: mark, restore_audio: None };
    let trimmed = tokio::task::spawn_blocking(move || trim_scene_video(&tools, &request, &Stamp::now().compact)).await.map_err(internal)?.map_err(internal)?;
    let video = trimmed.video_path.display().to_string();
    let thumbnail = trimmed.thumbnail_path.map(|p| p.display().to_string()).unwrap_or_default();
    let revision = {
        let _guard = state.edit_lock.lock().await;
        let (f, s, v, t) = (folder.clone(), scene.clone(), video.clone(), thumbnail.clone());
        tokio::task::spawn_blocking(move || record_video(&f, &s, &v, &t)).await.map_err(internal)?.map_err(fail)?
    };
    ctx.progress(ProgressUpdate { percent: 100.0, stage: "completed".into(), message: "Video trimmed.".into(), scene_id: Some(scene.clone()), stage_count: 1, ..Default::default() });
    Ok(json!({"scene_id": scene, "video_path": video, "thumbnail_path": thumbnail, "source_path": source, "start": start, "duration": duration, "frames": frames, "revision": revision}))
}

async fn match_color_job(state: Arc<crate::app::AppState>, ctx: JobCtx) -> Result<Value, JobError> {
    let pid = ctx.project_id.clone().ok_or_else(|| invalid("project_id is required."))?;
    let scene = text_or_empty(ctx.params.get("scene_id")).trim().to_string();
    if scene.is_empty() {
        return Err(invalid("scene_id is required in params."));
    }
    let folder = resolve_project(&state.roots, &pid).map_err(fail)?;
    let session = session_of(&folder).await?;
    let segments = segments_of(&session);
    let index = scene_index(&segments, &scene).ok_or_else(|| fail(ApiError::scene_not_found(&scene, &pid)))?;
    let clip_of = |s: &Value| ["video_path", "rendered_video_path"].iter().map(|k| text_or_empty(s.get(*k)).trim().to_string()).find(|p| !p.is_empty()).unwrap_or_default();
    let named = |key: &str| text_or_empty(ctx.params.get(key)).trim().to_string();
    let video = Some(named("video_path")).filter(|p| !p.is_empty()).unwrap_or_else(|| clip_of(&segments[index]));
    let reference = Some(named("reference_video_path")).filter(|p| !p.is_empty()).unwrap_or_else(|| if index > 0 { clip_of(&segments[index - 1]) } else { String::new() });
    if video.is_empty() || !FsPath::new(&video).is_file() {
        return Err(invalid(format!("Scene video not found for scene '{scene}'.")));
    }
    if reference.is_empty() || !FsPath::new(&reference).is_file() {
        return Err(invalid(format!("Reference video not found to match color for scene '{scene}'.")));
    }
    let tools = job_tools(&state)?;
    ctx.progress(ProgressUpdate { percent: 20.0, stage: "matching_color".into(), message: "Matching opening color...".into(), scene_id: Some(scene.clone()), stage_count: 1, ..Default::default() });
    let (fade, strength) = (param_f64(&ctx.params, "fade_seconds", 1.0), param_f64(&ctx.params, "strength", 0.85));
    let number = index as u32 + 1;
    let (project, clip, ref_clip) = (folder.clone(), PathBuf::from(&video), PathBuf::from(&reference));
    let result = tokio::task::spawn_blocking(move || {
        let before = takes::video_fingerprint(&clip);
        let token = format!("{}_{}", Stamp::now().millis(), std::process::id());
        let result = apply_color_match(&tools, &clip, &ref_clip, fade, strength, &token)?;
        if result.applied {
            if let Err(e) = takes::rename_video(&project, number, &before, &takes::video_fingerprint(&clip)) {
                eprintln!("[VRGDG-RS Latent Takes] Could not keep the scene's latent take after the color match: {e}");
            }
        }
        Ok::<_, h3_media::MediaError>(result)
    })
    .await
    .map_err(internal)?
    .map_err(internal)?;
    let thumbnail = result.thumbnail_path.map(|p| p.display().to_string()).unwrap_or_default();
    let revision = {
        let _guard = state.edit_lock.lock().await;
        let (f, s, v, t) = (folder.clone(), scene.clone(), video.clone(), thumbnail.clone());
        tokio::task::spawn_blocking(move || record_video(&f, &s, &v, &t)).await.map_err(internal)?.map_err(fail)?
    };
    ctx.progress(ProgressUpdate { percent: 100.0, stage: "completed".into(), message: "Color match applied.".into(), scene_id: Some(scene.clone()), stage_count: 1, ..Default::default() });
    Ok(json!({"scene_id": scene, "video_path": video, "thumbnail_path": thumbnail, "revision": revision}))
}
