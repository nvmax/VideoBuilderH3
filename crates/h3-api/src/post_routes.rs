//! Color grading of scene clips with `.cube` LUTs. Port of the LUT parts of `agent_api/orchestrator/post_orchestrator.py`.
//! LUTs live in a `luts` folder next to the app's config file. Film grain, color adjust, previews and face fix are not
//! part of this app.

#![allow(clippy::result_large_err)]

use std::collections::HashMap;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::Json;
use base64::Engine;
use h3_core::pyval::{text_or_empty, to_float, truthy, Obj};
use h3_jobs::{Handler, JobCtx, JobError, ProgressUpdate};
use h3_media::post::{apply_lut_to_video, normalize_encode_crf};
use h3_media::video::create_thumbnail;
use h3_media::Tools;
use h3_pipeline::scene_video::apply_scene_video;
use h3_project::{atomic_write_text, load_session, persist_session, Stamp};
use serde_json::{json, Value};

use crate::app::{blocking, load, AppState, Shared};
use crate::error::{success, ApiError, ApiResult};
use crate::job_handlers::fail;
use crate::views::resolve_project;

pub const POST_LUT: &str = "post.lut";
pub const POST_APPLY_ALL: &str = "post.apply_all";

type Run = fn(Arc<AppState>, JobCtx) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, JobError>> + Send>>;

pub fn register(state: &Arc<AppState>) {
    let table: [(&str, Run); 2] = [(POST_LUT, |s, c| Box::pin(lut_job(s, c))), (POST_APPLY_ALL, |s, c| Box::pin(apply_all_job(s, c)))];
    for (kind, run) in table {
        let owned = Arc::clone(state);
        let handler: Handler = Arc::new(move |ctx| run(Arc::clone(&owned), ctx));
        state.jobs.register(kind, handler);
    }
}

fn tools(state: &AppState) -> Result<Tools, ApiError> {
    Tools::find(&state.cfg.ffmpeg_path, &state.cfg.ffprobe_path).map_err(|e| ApiError::new(axum::http::StatusCode::SERVICE_UNAVAILABLE, "FFMPEG_FAILED", e.to_string()))
}

/// The `.cube` file a LUT name points to, kept inside the LUT folder.
fn lut_file(dir: &FsPath, name: &str) -> Result<PathBuf, ApiError> {
    let base = FsPath::new(name.trim().trim_matches('"')).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if base.is_empty() {
        return Err(ApiError::validation("LUT name is required."));
    }
    let path = dir.join(&base);
    if !path.is_file() {
        return Err(ApiError::validation(format!("LUT file was not found: {base}")));
    }
    if path.extension().map(|e| e.to_string_lossy().to_lowercase()) != Some("cube".into()) {
        return Err(ApiError::validation("Only .cube LUT files are supported."));
    }
    Ok(path)
}

/// `GET /post/luts`: the `.cube` files in the LUT folder.
pub async fn list_luts(State(state): Shared) -> ApiResult {
    let dir = state.luts_dir();
    let listing = blocking(move || {
        let mut items: Vec<(String, Value)> = Vec::new();
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if !path.is_file() || path.extension().map(|e| e.to_string_lossy().to_lowercase()) != Some("cube".into()) {
                continue;
            }
            let meta = std::fs::metadata(&path).ok();
            let stem = FsPath::new(&name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let modified = meta.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0.0, |d| d.as_secs_f64());
            items.push((name.to_lowercase(), json!({"name": name, "label": stem.replace('_', " "), "path": path.display().to_string(), "example_name": "", "example_url": "", "size": meta.map_or(0, |m| m.len()), "modified": modified})));
        }
        items.sort_by(|a, b| a.0.cmp(&b.0));
        json!({"luts": items.into_iter().map(|(_, v)| v).collect::<Vec<_>>(), "luts_dir": dir.display().to_string(), "examples_dir": dir.join("examples").display().to_string()})
    })
    .await?;
    Ok(success(listing, None))
}

/// `POST /post/luts/upload`: add a `.cube` file. The body is the file itself (name in `?filename=`) or JSON with
/// `filename` and base64 `data`.
pub async fn upload_lut(State(state): Shared, Query(query): Query<HashMap<String, String>>, headers: HeaderMap, body: Bytes) -> ApiResult {
    let is_json = headers.get("content-type").and_then(|v| v.to_str().ok()).is_some_and(|c| c.starts_with("application/json"));
    if headers.get("content-type").and_then(|v| v.to_str().ok()).is_some_and(|c| c.starts_with("multipart/")) {
        return Err(ApiError::validation("Send the .cube file as the request body with ?filename=, or as JSON with filename and base64 data."));
    }
    let (mut filename, content) = if is_json {
        let parsed: Value = serde_json::from_slice(&body).map_err(|e| ApiError::validation(format!("The body is not valid JSON: {e}")))?;
        let data = text_or_empty(parsed.get("data").filter(|v| truthy(v)).or_else(|| parsed.get("content")));
        let bytes = if data.trim().is_empty() { Vec::new() } else { base64::engine::general_purpose::STANDARD.decode(data.trim()).map_err(|e| ApiError::validation(format!("The data is not valid base64: {e}")))? };
        (text_or_empty(parsed.get("filename")), bytes)
    } else {
        (String::new(), body.to_vec())
    };
    if let Some(q) = query.get("filename").filter(|f| !f.is_empty()) {
        filename = q.clone();
    }
    if filename.trim().is_empty() {
        filename = "custom.cube".into();
    }
    let clean = FsPath::new(filename.trim()).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if !clean.to_lowercase().ends_with(".cube") {
        return Err(ApiError::validation(format!("LUT file must end with .cube, got: {clean}")));
    }
    let dir = state.luts_dir();
    let target = dir.join(&clean);
    let size = content.len();
    let written = target.clone();
    blocking(move || -> std::io::Result<()> {
        std::fs::create_dir_all(&dir)?;
        // A LUT is text. Writing it through the same atomic write the project files use keeps a half written file out.
        atomic_write_text(&written, &String::from_utf8_lossy(&content))
    })
    .await?
    .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(success(json!({"name": clean, "path": target.display().to_string(), "size": size}), None))
}

/// `POST .../post/lut`: grade a scene's clip, as a job.
pub async fn lut_route(State(state): Shared, Path((pid, sid)): Path<(String, String)>, body: Option<Json<Value>>) -> ApiResult {
    let mut params = crate::app::body_object(body)?;
    params.insert("scene_id".into(), json!(sid));
    load(&state, &pid).await?;
    let job = state.jobs.submit(POST_LUT, Some(&pid), Value::Object(params), false)?;
    Ok(crate::app::job_response(&job))
}

/// `POST /projects/{pid}/post/apply-all`: grade several scenes, as a job.
pub async fn apply_all_route(State(state): Shared, Path(pid): Path<String>, body: Option<Json<Value>>) -> ApiResult {
    let params = crate::app::body_object(body)?;
    load(&state, &pid).await?;
    let job = state.jobs.submit(POST_APPLY_ALL, Some(&pid), Value::Object(params), false)?;
    Ok(crate::app::job_response(&job))
}

// -- jobs -------------------------------------------------------------------------------------------------------------------

fn internal(e: impl std::fmt::Display) -> JobError {
    fail(ApiError::internal(e.to_string()))
}

fn invalid(message: impl Into<String>) -> JobError {
    fail(ApiError::validation(message))
}

fn scene_index(segments: &[Value], scene: &str) -> Option<usize> {
    segments.iter().enumerate().position(|(i, s)| text_or_empty(s.get("id")) == scene || (i + 1).to_string() == scene)
}

/// The scene's current clip, which must exist and sit inside the project folder.
fn scene_clip(project: &FsPath, segment: &Value, scene: &str) -> Result<PathBuf, JobError> {
    let mut path = ["video_path", "rendered_video_path"].iter().map(|k| text_or_empty(segment.get(*k)).trim().to_string()).find(|p| !p.is_empty()).unwrap_or_default();
    if path.is_empty() {
        path = segment.get("video_history").and_then(Value::as_array).and_then(|a| a.last()).map(|v| text_or_empty(Some(v))).unwrap_or_default();
    }
    if path.is_empty() {
        return Err(invalid(format!("Scene '{scene}' has no rendered video to post-process.")));
    }
    let resolved = h3_core::abspath::absolute(&path).unwrap_or_else(|_| PathBuf::from(&path));
    if !resolved.is_file() {
        return Err(invalid(format!("Scene '{scene}' video was not found on disk: {}", resolved.display())));
    }
    let root = h3_core::abspath::absolute(project).unwrap_or_else(|_| project.to_path_buf());
    let inside = |a: &FsPath, b: &FsPath| if cfg!(windows) { a.display().to_string().to_lowercase().starts_with(&b.display().to_string().to_lowercase()) } else { a.starts_with(b) };
    if !inside(&resolved, &root) {
        return Err(invalid(format!("Scene video must be inside project folder '{}', got '{}'.", project.display(), resolved.display())));
    }
    Ok(resolved)
}

/// Record a graded clip on the scene like a render does.
fn record(project: &FsPath, index: usize, video: &str, thumbnail: &str) -> Result<(i64, usize, usize), String> {
    let mut session = load_session(project).map_err(|e| e.to_string())?.session;
    let mut segments: Vec<Value> = session.get("segments").and_then(Value::as_array).cloned().unwrap_or_default();
    let segment = segments.get_mut(index).and_then(Value::as_object_mut).ok_or("The scene is not there any more.")?;
    apply_scene_video(segment, video, thumbnail, (Stamp::now().epoch * 1000.0) as i64);
    let history_index = segment.get("video_history_index").and_then(Value::as_u64).unwrap_or(0) as usize;
    let history_count = segment.get("video_history").and_then(Value::as_array).map_or(0, Vec::len);
    session.insert("segments".into(), Value::Array(segments));
    let revision = persist_session(project, session, Stamp::now()).map_err(|e| e.to_string())?.revision;
    Ok((revision, history_index, history_count))
}

struct Grade<'a> {
    state: &'a AppState,
    project: &'a FsPath,
    index: usize,
    clip: PathBuf,
    cube: PathBuf,
    lut: String,
    strength: f64,
    preserve_audio: bool,
    crf: i64,
    preset: String,
}

/// Grade the clip into `video_NNNN-lut_<stamp>.mp4`, record it on the scene and return what the pack's job returns.
async fn grade(g: Grade<'_>, scene: &str) -> Result<Value, JobError> {
    let tools = tools(g.state).map_err(fail)?;
    let stamp = Stamp::now().compact;
    let number = g.index + 1;
    let target = g.project.join("rendered_scene_videos").join(format!("video_{number:04}-lut_{stamp}.mp4"));
    let (clip, output, cube) = (g.clip.clone(), target.clone(), g.cube.clone());
    let (strength, preserve, crf, preset) = (g.strength, g.preserve_audio, g.crf, g.preset.clone());
    let (tools_for_render, tools_for_thumb) = (tools.clone(), tools);
    let mut report = tokio::task::spawn_blocking(move || apply_lut_to_video(&tools_for_render, &clip, &output, &cube, strength, preserve, crf, &preset)).await.map_err(internal)?.map_err(internal)?;
    let thumb_target = target.clone();
    let thumbnail = tokio::task::spawn_blocking(move || create_thumbnail(&tools_for_thumb, &thumb_target, None)).await.map_err(internal)?.map(|p| p.display().to_string()).unwrap_or_default();
    let video = target.display().to_string();
    let _guard = g.state.edit_lock.lock().await;
    let (project, v, t) = (g.project.to_path_buf(), video.clone(), thumbnail.clone());
    let (revision, history_index, history_count) = tokio::task::spawn_blocking(move || record(&project, g.index, &v, &t)).await.map_err(internal)?.map_err(internal)?;
    if let Some(map) = report.as_object_mut() {
        map.insert("input".into(), json!(g.clip.display().to_string()));
        map.insert("output".into(), json!(video));
        map.insert("lut".into(), json!(g.lut));
        map.insert("strength".into(), json!(g.strength));
        map.insert("replace_source".into(), json!(false));
        map.insert("thumbnail_path".into(), json!(thumbnail));
        map.insert("output_path".into(), json!(video));
        map.insert("scene_id".into(), json!(scene));
        map.insert("video_path".into(), json!(video));
        map.insert("history_index".into(), json!(history_index));
        map.insert("history_count".into(), json!(history_count));
        map.insert("revision".into(), json!(revision));
    }
    Ok(report)
}

fn number(params: &Value, key: &str, default: f64) -> f64 {
    params.get(key).filter(|v| !v.is_null()).and_then(to_float).unwrap_or(default)
}

async fn lut_job(state: Arc<AppState>, ctx: JobCtx) -> Result<Value, JobError> {
    let pid = ctx.project_id.clone().ok_or_else(|| invalid("project_id and scene_id are required."))?;
    let scene = text_or_empty(ctx.params.get("scene_id")).trim().to_string();
    if scene.is_empty() {
        return Err(invalid("project_id and scene_id are required."));
    }
    if text_or_empty(ctx.params.get("media_type")).trim().to_lowercase() == "image" {
        return Err(invalid("LUTs on scene images are not supported by this app yet. Grade the video."));
    }
    let folder = resolve_project(&state.roots, &pid).map_err(fail)?;
    let session = tokio::task::spawn_blocking({
        let folder = folder.clone();
        move || load_session(&folder)
    })
    .await
    .map_err(internal)?
    .map_err(internal)?
    .session;
    let segments: Vec<Value> = session.get("segments").and_then(Value::as_array).cloned().unwrap_or_default();
    let index = scene_index(&segments, &scene).ok_or_else(|| fail(ApiError::scene_not_found(&scene, &pid)))?;
    let clip = scene_clip(&folder, &segments[index], &scene)?;
    let lut = text_or_empty(ctx.params.get("lut_name"));
    let cube = lut_file(&state.luts_dir(), &lut).map_err(fail)?;
    ctx.progress(ProgressUpdate { percent: 10.0, stage: "processing".into(), message: "Applying LUT to media...".into(), scene_id: Some(scene.clone()), stage_count: 1, ..Default::default() });
    let params = &ctx.params;
    let result = grade(
        Grade {
            state: &state,
            project: &folder,
            index,
            clip,
            cube: cube.clone(),
            lut: cube.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            strength: number(params, "strength", 10.0),
            preserve_audio: params.get("preserve_audio").is_none_or(truthy),
            crf: normalize_encode_crf(params.get("encode_crf").and_then(to_float)),
            preset: text_or_empty(params.get("encode_preset")),
        },
        &scene,
    )
    .await?;
    ctx.progress(ProgressUpdate { percent: 100.0, stage: "completed".into(), message: "LUT applied to video.".into(), scene_id: Some(scene), stage_count: 1, ..Default::default() });
    Ok(result)
}

async fn apply_all_job(state: Arc<AppState>, ctx: JobCtx) -> Result<Value, JobError> {
    let pid = ctx.project_id.clone().ok_or_else(|| invalid("project_id is required."))?;
    let stack: Obj = ctx.params.get("stack").and_then(Value::as_object).cloned().unwrap_or_default();
    let adjust_wanted = stack.get("adjust").and_then(|a| a.get("settings")).is_some_and(truthy);
    let grain_wanted = stack.get("film_grain").and_then(Value::as_object).is_some_and(|g| g.get("enabled").is_none_or(truthy));
    if adjust_wanted || grain_wanted {
        return Err(invalid("Color adjust and film grain are not supported by this app. Send only a LUT in the stack."));
    }
    let lut_config = stack.get("lut").and_then(Value::as_object).cloned();
    let lut = lut_config.as_ref().map(|l| text_or_empty(l.get("lut_name"))).unwrap_or_default();
    let folder = resolve_project(&state.roots, &pid).map_err(fail)?;
    let session = tokio::task::spawn_blocking({
        let folder = folder.clone();
        move || load_session(&folder)
    })
    .await
    .map_err(internal)?
    .map_err(internal)?
    .session;
    let segments: Vec<Value> = session.get("segments").and_then(Value::as_array).cloned().unwrap_or_default();
    let wanted: Vec<String> = ctx.params.get("scene_ids").and_then(Value::as_array).map(|a| a.iter().map(|v| text_or_empty(Some(v))).collect()).unwrap_or_default();
    let mut targets: Vec<(usize, String, PathBuf)> = Vec::new();
    for (i, segment) in segments.iter().enumerate() {
        let id = Some(text_or_empty(segment.get("id"))).filter(|s| !s.is_empty()).unwrap_or_else(|| (i + 1).to_string());
        if !wanted.is_empty() && !wanted.contains(&id) && !wanted.contains(&(i + 1).to_string()) {
            continue;
        }
        let clip = ["video_path", "rendered_video_path"].iter().map(|k| text_or_empty(segment.get(*k)).trim().to_string()).find(|p| !p.is_empty()).unwrap_or_default();
        if !clip.is_empty() && FsPath::new(&clip).is_file() {
            targets.push((i, id, PathBuf::from(clip)));
        }
    }
    if targets.is_empty() {
        return Ok(json!({"project_id": pid, "processed_scenes": [], "count": 0}));
    }
    let cube = if lut.is_empty() { None } else { Some(lut_file(&state.luts_dir(), &lut).map_err(fail)?) };
    let total = targets.len();
    let mut results = Vec::new();
    for (n, (index, id, clip)) in targets.into_iter().enumerate() {
        if ctx.is_cancelled() {
            return Err(JobError::cancelled());
        }
        ctx.progress(ProgressUpdate {
            percent: n as f64 / total as f64 * 100.0,
            stage: "processing".into(),
            message: format!("Applying stack to scene {id} ({}/{total})...", n + 1),
            scene_id: Some(id.clone()),
            stage_count: total as i64,
            ..Default::default()
        });
        let Some(cube) = cube.clone() else { continue };
        let config = lut_config.clone().unwrap_or_default();
        let result = grade(
            Grade {
                state: &state,
                project: &folder,
                index,
                clip,
                cube: cube.clone(),
                lut: cube.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                strength: config.get("strength").and_then(to_float).unwrap_or(10.0),
                preserve_audio: true,
                crf: 23,
                preset: "medium".into(),
            },
            &id,
        )
        .await?;
        results.push(json!({"scene_id": id, "video_path": result["video_path"], "thumbnail_path": result["thumbnail_path"], "history_index": result["history_index"], "history_count": result["history_count"], "revision": result["revision"]}));
    }
    ctx.progress(ProgressUpdate { percent: 100.0, stage: "completed".into(), message: format!("Applied stack to {} scenes.", results.len()), stage_count: 1, ..Default::default() });
    Ok(json!({"project_id": pid, "processed_scenes": results, "count": results.len()}))
}
