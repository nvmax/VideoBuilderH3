//! Full pipelines for agents: the dry-run plan, the full video build and song to video. Port of
//! `agent_api/orchestrator/pipeline_orchestrator.py`. The LTX first and last frame pipeline is not part of this app.

#![allow(clippy::result_large_err)]

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::Json;
use h3_core::pyval::{py_str, text_or_empty, to_float, truthy, Obj};
use h3_jobs::{Handler, JobCtx, JobError, ProgressUpdate};
use h3_pipeline::pipeline_plan::{clamped_int, filter_target_scenes, pipeline_plan, plan_scene_boundaries, DEFAULT_SCENE_SECONDS};
use h3_pipeline::{render_scene_full, stitch_project};
use h3_project::{images, load_session, persist_session, scene_ops, session_ops, Stamp};
use serde_json::{json, Value};

use crate::app::{blocking, body_object, job_response, load, AppState, Shared};
use crate::error::{success, ApiError, ApiResult};
use crate::job_handlers::{context_for, fail, finish_options, render_params, reporter_in};
use crate::views::resolve_project;

pub const PIPELINE_FULL_VIDEO: &str = "pipeline.build_full_video";
pub const PIPELINE_FROM_SONG: &str = "pipeline.from_song";
pub const LATENTS_REBUILD: &str = "latents.rebuild";

type Run = fn(Arc<AppState>, JobCtx) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, JobError>> + Send>>;

pub fn register(state: &Arc<AppState>) {
    let table: [(&str, Run); 3] = [
        (PIPELINE_FULL_VIDEO, |s, c| Box::pin(full_video(s, c))),
        (PIPELINE_FROM_SONG, |s, c| Box::pin(from_song(s, c))),
        (LATENTS_REBUILD, |s, c| Box::pin(rebuild_latents(s, c))),
    ];
    for (kind, run) in table {
        let owned = Arc::clone(state);
        let handler: Handler = Arc::new(move |ctx| run(Arc::clone(&owned), ctx));
        state.jobs.register(kind, handler);
    }
}

// -- routes -----------------------------------------------------------------------------------------------------------------

/// `GET /projects/{pid}/pipelines/plan`: what a run would do, without doing it.
pub async fn plan_route(State(state): Shared, Path(pid): Path<String>, Query(query): Query<std::collections::HashMap<String, String>>) -> ApiResult {
    let loaded = load(&state, &pid).await?;
    let params: Obj = query.into_iter().map(|(k, v)| (k, json!(v))).collect();
    let plan = blocking(move || pipeline_plan(&pid, &loaded.session, &params)).await?;
    Ok(success(plan, None))
}

/// `POST /projects/{pid}/pipelines/build-full-video`: images, prompts, videos and the stitch, as one job.
pub async fn full_video_route(State(state): Shared, Path(pid): Path<String>, body: Option<Json<Value>>) -> ApiResult {
    let mut params = body_object(body)?;
    load(&state, &pid).await?;
    params.insert("project_id".into(), json!(pid));
    let job = state.jobs.submit(PIPELINE_FULL_VIDEO, Some(&pid), Value::Object(params), true)?;
    Ok(job_response(&job))
}

/// `POST /pipelines/from-song`: a song to a final video. Creates the project when only `project_name` is given.
pub async fn from_song_route(State(state): Shared, body: Option<Json<Value>>) -> ApiResult {
    let mut params = body_object(body)?;
    let mut project_id = text_or_empty(params.get("project_id")).trim().to_string();
    if project_id.is_empty() {
        let name = text_or_empty(params.get("project_name")).trim().to_string();
        if name.is_empty() {
            return Err(ApiError::validation("Provide project_id for an existing project, or project_name to create one."));
        }
        let root = state.cfg.project_root.clone();
        let _guard = state.edit_lock.lock().await;
        let (folder, _) = blocking(move || session_ops::create_project(&root, &name, &Stamp::now())).await?.map_err(|e| ApiError::validation(e.to_string()))?;
        project_id = crate::views::project_id(&folder);
    } else {
        load(&state, &project_id).await?;
    }
    params.insert("project_id".into(), json!(project_id));
    let job = state.jobs.submit(PIPELINE_FROM_SONG, Some(&project_id), Value::Object(params), true)?;
    let mut data = serde_json::Map::new();
    data.insert("project_id".into(), json!(project_id));
    data.insert("job_id".into(), json!(job.id));
    data.insert("status".into(), json!(job.status));
    data.insert("job".into(), job.to_api());
    Ok(crate::error::respond(axum::http::StatusCode::ACCEPTED, Value::Object(data), None))
}

// -- jobs -------------------------------------------------------------------------------------------------------------------

fn progress(ctx: &JobCtx, percent: f64, stage: &str, message: String, scene_id: Option<String>) {
    ctx.progress(ProgressUpdate { percent, stage: stage.into(), message, scene_id, stage_count: 1, ..Default::default() });
}

fn has_image(seg: &Value) -> bool {
    seg.get("approved_image_path").is_some_and(truthy) || seg.get("custom_image_path").is_some_and(truthy) || seg.get("image_history").and_then(Value::as_array).is_some_and(|a| !a.is_empty())
}

fn has_video(seg: &Value) -> bool {
    seg.get("video_path").is_some_and(truthy) || seg.get("rendered_video_path").is_some_and(truthy) || seg.get("video_history").and_then(Value::as_array).is_some_and(|a| !a.is_empty())
}

fn text_of(seg: &Value, key: &str) -> String {
    h3_core::pyval::text_or_empty(seg.get(key)).trim().to_string()
}

fn id_list(value: Option<&Value>) -> Option<Vec<String>> {
    match value {
        Some(Value::String(s)) => Some(s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()),
        Some(Value::Array(a)) => Some(a.iter().map(|v| py_str(Some(v)).trim().to_string()).filter(|x| !x.is_empty()).collect()),
        _ => None,
    }
}

async fn edit<T: Send + 'static>(state: &AppState, work: impl FnOnce() -> T + Send + 'static) -> Result<T, JobError> {
    let _guard = state.edit_lock.lock().await;
    tokio::task::spawn_blocking(work).await.map_err(|e| fail(ApiError::internal(e.to_string())))
}

fn io_fail(e: impl std::fmt::Display) -> JobError {
    fail(ApiError::internal(e.to_string()))
}

async fn full_video(state: Arc<AppState>, ctx: JobCtx) -> Result<Value, JobError> {
    run_full_video(&state, &ctx, &ctx.params.clone()).await
}

/// Images for scenes without one, a prompt for scenes without one, every missing video, then the stitch.
async fn run_full_video(state: &Arc<AppState>, ctx: &JobCtx, params: &Value) -> Result<Value, JobError> {
    let pid = ctx.project_id.clone().ok_or_else(|| fail(ApiError::validation("project_id is required.")))?;
    let folder = resolve_project(&state.roots, &pid).map_err(fail)?;
    let loaded = load_session(&folder).map_err(io_fail)?;
    let session = loaded.session;
    let segments: Vec<Value> = session.get("segments").and_then(Value::as_array).cloned().unwrap_or_default();
    let as_obj = params.as_object().cloned().unwrap_or_default();

    let build_mode = Some(text_or_empty(as_obj.get("build_mode")).trim().to_lowercase()).filter(|s| !s.is_empty()).unwrap_or_else(|| "resume_missing".into());
    let scope = Some(text_or_empty(as_obj.get("scope")).trim().to_lowercase()).filter(|s| !s.is_empty()).unwrap_or_else(|| "all".into());
    let scene_ids = id_list(as_obj.get("scene_ids"));
    let retries = clamped_int(&as_obj, "max_auto_retries", 3, 0, 5) as usize;
    let randomize = text_or_empty(as_obj.get("video_seed_mode")).trim().to_lowercase() != "fixed";
    let stitch = as_obj.get("stitch").is_none_or(truthy);
    let video_mode = Some(text_or_empty(as_obj.get("video_mode").filter(|v| truthy(v)).or(session.get("video_mode").filter(|v| truthy(v)))).trim().to_lowercase()).filter(|s| !s.is_empty()).unwrap_or_else(|| "minimax_h3".into());

    let targets = filter_target_scenes(&segments, &scope, scene_ids.as_deref());
    if targets.is_empty() {
        progress(ctx, 100.0, "completed", "No scenes to process.".into(), None);
        return Ok(json!({"project_id": pid, "scenes_processed": 0, "images_generated": 0, "videos_rendered": 0}));
    }
    let total = targets.len();
    let image_applicable = !matches!(video_mode.as_str(), "t2v" | "rtv") && build_mode != "redo_videos";
    let (mut images_made, mut videos_made) = (0usize, 0usize);
    let mut results: Vec<Value> = Vec::new();

    // Stage 1: pictures.
    if image_applicable {
        progress(ctx, 5.0, "images_pass", "Starting image generation pass...".into(), None);
        let mut context = None;
        for (i, (idx, sid, seg)) in targets.iter().enumerate() {
            if ctx.is_cancelled() {
                return Err(JobError::cancelled());
            }
            if !(build_mode == "fresh_rebuild" || !has_image(seg)) {
                continue;
            }
            let percent = 5.0 + i as f64 / total as f64 * 30.0;
            progress(ctx, percent, "generating_image", format!("Generating image for scene {sid} ({}/{total})...", i + 1), Some(sid.clone()));
            if text_of(seg, "t2i_prompt").is_empty() {
                let text = Some(text_of(seg, "lyric_text")).filter(|t| !t.is_empty()).unwrap_or_else(|| format!("Scene {} visual depiction", idx + 1));
                let (f, s) = (folder.clone(), sid.clone());
                edit(state, move || session_ops::set_scene_prompt(&f, &s, "t2i_prompt", &text, "pipeline", None, &Stamp::now())).await?.map_err(io_fail)?;
            }
            if context.is_none() {
                context = Some(context_for(state, ctx).await?);
            }
            let (run, _) = context.as_ref().expect("set above");
            let made = crate::image_jobs::generate_for_scene(state, ctx, run, &folder, &(idx + 1).to_string(), params, percent, 30.0 / total as f64).await?;
            let saved = text_or_empty(made.get("saved_path"));
            let (f, index) = (folder.clone(), *idx);
            edit(state, move || -> std::io::Result<()> {
                let mut session = load_session(&f)?.session;
                images::approve_scene_image(&mut session, index, Some(&saved))?;
                persist_session(&f, session, Stamp::now()).map(|_| ())
            })
            .await?
            .map_err(io_fail)?;
            images_made += 1;
        }
    }

    // Stage 2: a prompt for every scene that has none (an existing prompt is never replaced).
    if build_mode != "redo_videos" {
        progress(ctx, 35.0, "prompts_pass", "Checking video prompts...".into(), None);
        let fresh = load_session(&folder).map_err(io_fail)?.session;
        let current: Vec<Value> = fresh.get("segments").and_then(Value::as_array).cloned().unwrap_or_default();
        for (idx, sid, _) in &targets {
            if ctx.is_cancelled() {
                return Err(JobError::cancelled());
            }
            let seg = current.get(*idx).cloned().unwrap_or(Value::Null);
            if !text_of(&seg, "minimax_h3_prompt").is_empty() {
                continue;
            }
            let text = [text_of(&seg, "lyric_text"), text_of(&seg, "t2i_prompt")].into_iter().find(|t| !t.is_empty()).unwrap_or_else(|| format!("Cinematic motion sequence for scene {}", idx + 1));
            let (f, s) = (folder.clone(), sid.clone());
            edit(state, move || session_ops::set_scene_prompt(&f, &s, "minimax_h3_prompt", &text, "pipeline", None, &Stamp::now())).await?.map_err(io_fail)?;
        }
    }

    // Stage 3: videos.
    progress(ctx, 45.0, "videos_pass", "Starting video rendering pass...".into(), None);
    let (mut run, _) = context_for(state, ctx).await?;
    run.before_scene = Some(crate::continued_prompt::hook(state));
    let mut render_input = as_obj.clone();
    render_input.insert("randomize_seed".into(), json!(randomize));
    for key in ["build_mode", "max_auto_retries", "video_seed_mode", "stitch", "project_id", "image_mode", "video_mode", "preset_id"] {
        render_input.remove(key);
    }
    let render_value = Value::Object(render_input);
    let current: Vec<Value> = load_session(&folder).map_err(io_fail)?.session.get("segments").and_then(Value::as_array).cloned().unwrap_or_default();
    for (i, (idx, sid, _)) in targets.iter().enumerate() {
        if ctx.is_cancelled() {
            return Err(JobError::cancelled());
        }
        let seg = current.get(*idx).cloned().unwrap_or(Value::Null);
        let need = !has_video(&seg) || matches!(build_mode.as_str(), "fresh_rebuild" | "redo_videos" | "redo_i2v_prompts_videos");
        if !need {
            let path = ["video_path", "rendered_video_path"].iter().map(|k| text_of(&seg, k)).find(|p| !p.is_empty()).unwrap_or_default();
            results.push(json!({"scene_id": sid, "status": "skipped_existing", "video_path": path}));
            continue;
        }
        let percent = 45.0 + i as f64 / total as f64 * 40.0;
        progress(ctx, percent, "rendering_video", format!("Rendering video for scene {sid} ({}/{total})...", i + 1), Some(sid.clone()));
        let mut attempt = 0;
        loop {
            if ctx.is_cancelled() {
                return Err(JobError::cancelled());
            }
            // The scene's own progress fills its share of the video pass (45% to 85%).
            let done = render_scene_full(&run, &folder, &(idx + 1).to_string(), render_params(&render_value), finish_options(&render_value), reporter_in(ctx, Some(sid.clone()), percent, 40.0 / total as f64)).await;
            match done {
                Ok(scene) => {
                    videos_made += 1;
                    results.push(json!({"scene_id": sid, "status": "success", "video_path": scene.video_path}));
                    break;
                }
                Err(e) => {
                    let error: ApiError = e.into();
                    if error.code == "JOB_CANCELLED" || ctx.is_cancelled() {
                        return Err(JobError::cancelled());
                    }
                    if attempt >= retries {
                        eprintln!("[VRGDG-RS Pipeline] Scene {sid} video render failed after {} attempts: {}", attempt + 1, error.message);
                        return Err(error.into_job_error());
                    }
                    eprintln!("[VRGDG-RS Pipeline] Scene {sid} render attempt {} failed ({}); retrying...", attempt + 1, error.message);
                    attempt += 1;
                }
            }
        }
    }

    // Stage 4: stitch.
    let mut final_video = String::new();
    if stitch {
        progress(ctx, 88.0, "stitching", "Stitching scene videos into final video...".into(), None);
        match stitch_project(&run, &folder, &Obj::new(), false).await {
            Ok((done, _)) => final_video = done.final_video_path.display().to_string(),
            Err(e) => eprintln!("[VRGDG-RS Pipeline] Final video stitch after the full pipeline failed: {e}"),
        }
    }
    progress(ctx, 100.0, "completed", "Full video pipeline completed.".into(), None);
    Ok(json!({
        "project_id": pid, "pipeline": "full_video", "build_mode": build_mode, "scenes_processed": total, "images_generated": images_made,
        "videos_rendered": videos_made, "final_video_path": final_video, "results": results,
    }))
}

/// Prepare a project from a song (audio, lyrics, scenes), then build the full video.
async fn from_song(state: Arc<AppState>, ctx: JobCtx) -> Result<Value, JobError> {
    let pid = ctx.project_id.clone().ok_or_else(|| fail(ApiError::validation("project_id is required.")))?;
    let folder = resolve_project(&state.roots, &pid).map_err(fail)?;
    let params = ctx.params.clone();
    progress(&ctx, 2.0, "preparing_project", "Checking project audio...".into(), None);

    let given = text_or_empty(params.get("audio_path")).trim().trim_matches('"').to_string();
    if !given.is_empty() {
        let tools = h3_media::Tools::find(&state.cfg.ffmpeg_path, &state.cfg.ffprobe_path).map_err(io_fail)?;
        let source = PathBuf::from(&given);
        let name = source.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "audio.wav".into());
        let f = folder.clone();
        let _guard = state.edit_lock.lock().await;
        let info = tokio::task::spawn_blocking(move || crate::session_routes::save_project_audio(&tools, &f, Some(source), None, &name)).await.map_err(io_fail)?.map_err(fail)?;
        let f = folder.clone();
        tokio::task::spawn_blocking(move || session_ops::attach_audio(&f, &info, None, &Stamp::now())).await.map_err(io_fail)?.map_err(io_fail)?;
    } else if h3_pipeline::stitch_plan::session_audio_path(&load_session(&folder).map_err(io_fail)?.session).is_empty() {
        return Err(fail(ApiError::validation("Provide audio_path, or attach project audio before running the song pipeline.")));
    }
    if let Some(text) = params.get("lyrics_text").and_then(Value::as_str).filter(|t| !t.trim().is_empty()) {
        let (f, text) = (folder.clone(), text.to_string());
        edit(&state, move || session_ops::set_lyrics(&f, Some(&text), None, &Stamp::now())).await?.map_err(io_fail)?;
    }
    if ctx.is_cancelled() {
        return Err(JobError::cancelled());
    }
    let session = load_session(&folder).map_err(io_fail)?.session;
    let mut created = 0usize;
    if session.get("segments").and_then(Value::as_array).is_none_or(Vec::is_empty) {
        let duration = session.get("audio_duration").filter(|v| truthy(v)).and_then(to_float).unwrap_or(0.0);
        let seconds = params.get("scene_seconds").filter(|v| truthy(v)).and_then(to_float).unwrap_or(DEFAULT_SCENE_SECONDS);
        let boundaries = plan_scene_boundaries(duration, seconds);
        if boundaries.is_empty() {
            return Err(fail(ApiError::validation("The project audio has no readable duration, so scenes cannot be planned.")));
        }
        for (n, (start, end)) in boundaries.iter().enumerate() {
            if ctx.is_cancelled() {
                return Err(JobError::cancelled());
            }
            progress(&ctx, 5.0 + 10.0 * (n + 1) as f64 / boundaries.len() as f64, "creating_scenes", format!("Creating scene {}/{}...", n + 1, boundaries.len()), None);
            let (f, duration, label) = (folder.clone(), end - start, format!("Scene {}", n + 1));
            edit(&state, move || {
                let mut id = crate::edit_routes::new_scene_id;
                scene_ops::create_scene(&f, "append", None, duration, &label, "", "", "", None, &Stamp::now(), &mut id)
            })
            .await?
            .map_err(io_fail)?;
            created += 1;
        }
        if params.get("snap_to_beats").is_none_or(truthy) {
            let f = folder.clone();
            // No beats loaded: the scenes keep their even spacing.
            let _ = edit(&state, move || scene_ops::snap_to_beats(&f, "all", None, "start", &Stamp::now())).await?;
        }
    }
    let mut build = params.as_object().cloned().unwrap_or_default();
    for key in ["audio_path", "lyrics_text", "scene_seconds", "snap_to_beats", "project_name"] {
        build.remove(key);
    }
    let done = run_full_video(&state, &ctx, &Value::Object(build)).await?;
    let mut out = done.as_object().cloned().unwrap_or_default();
    out.insert("pipeline".into(), json!("from_song"));
    out.insert("scenes_created".into(), json!(created));
    Ok(Value::Object(out))
}

/// `POST /projects/{pid}/latents/rebuild`: render every scene whose latent is out of date, in order.
pub async fn rebuild_latents_route(State(state): Shared, Path(pid): Path<String>, body: Option<Json<Value>>) -> ApiResult {
    let mut params = body_object(body)?;
    load(&state, &pid).await?;
    params.insert("project_id".into(), json!(pid));
    let job = state.jobs.submit(LATENTS_REBUILD, Some(&pid), Value::Object(params), true)?;
    Ok(job_response(&job))
}

async fn rebuild_latents(state: Arc<AppState>, ctx: JobCtx) -> Result<Value, JobError> {
    let pid = ctx.project_id.clone().ok_or_else(|| fail(ApiError::validation("project_id is required.")))?;
    let folder = resolve_project(&state.roots, &pid).map_err(fail)?;
    let mut dirty = h3_project::latents::list_dirty(&folder);
    if dirty.is_empty() {
        progress(&ctx, 100.0, "completed", "No dirty latents found.".into(), None);
        return Ok(json!({"project_id": pid, "rebuilt_scenes": [], "count": 0, "message": "No dirty latents found"}));
    }
    dirty.sort_unstable();
    let segments: Vec<Value> = load_session(&folder).map_err(io_fail)?.session.get("segments").and_then(Value::as_array).cloned().unwrap_or_default();
    let (mut run, _) = context_for(&state, &ctx).await?;
    run.before_scene = Some(crate::continued_prompt::hook(&state));
    let render = ctx.params.get("render_params").cloned().unwrap_or_else(|| json!({}));
    let total = dirty.len();
    let mut rebuilt: Vec<Value> = Vec::new();
    for (i, number) in dirty.iter().enumerate() {
        if ctx.is_cancelled() {
            return Err(JobError::cancelled());
        }
        let Some(seg) = segments.get(*number as usize - 1) else {
            eprintln!("[VRGDG-RS Pipeline] Scene number {number} is outside the project's scenes ({}). Skipping.", segments.len());
            continue;
        };
        let sid = h3_core::pyval::text_or_empty(seg.get("id"));
        let sid = if sid.is_empty() { number.to_string() } else { sid };
        progress(&ctx, i as f64 / total as f64 * 100.0, "rendering", format!("Rebuilding scene {number:03} latent ({}/{total})...", i + 1), Some(sid.clone()));
        let share = 100.0 / total as f64;
        let done = render_scene_full(&run, &folder, &number.to_string(), render_params(&render), finish_options(&render), reporter_in(&ctx, Some(sid.clone()), i as f64 * share, share)).await.map_err(fail)?;
        h3_project::latents::clear_dirty(&folder, *number);
        rebuilt.push(json!({"scene_number": number, "scene_id": sid, "video_path": done.video_path}));
    }
    progress(&ctx, 100.0, "completed", format!("Rebuilt {} scene latents.", rebuilt.len()), None);
    Ok(json!({"project_id": pid, "rebuilt_scenes": rebuilt, "count": rebuilt.len()}))
}

