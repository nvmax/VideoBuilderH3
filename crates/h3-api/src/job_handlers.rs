//! The job types the API runs: one scene render, a batch render and a stitch.

// A job's error carries its API body (code, message, details), so it is a large value on a path that runs once per job.
#![allow(clippy::result_large_err)]

use std::sync::Arc;

use h3_core::pyval::Obj;
use h3_jobs::{Handler, JobCtx, JobError, ProgressUpdate};
use h3_pipeline::{connect, render_all, render_scene_full, stitch_project, BatchOptions, FinishOptions, RenderEvent, RenderParams, RunContext};
use serde_json::{json, Value};

use crate::app::AppState;
use crate::error::ApiError;
use crate::views::resolve_project;

/// Params that steer the job. Every other key replaces a render setting for this run, like the pack's render params.
const CONTROL_KEYS: [&str; 12] =
    ["scene_id", "mode", "timeout_hours", "match_start_color", "resume", "run_mode", "scope", "scene_ids", "range", "force", "skip_final_stitch", "audio_mode"];

pub const VIDEO_RENDER: &str = "video.render";
pub const VIDEOS_RENDER_BATCH: &str = "videos.render_batch";
pub const VIDEO_STITCH: &str = "video.stitch";

type Run = fn(Arc<AppState>, JobCtx) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, JobError>> + Send>>;

pub fn register(state: &Arc<AppState>) {
    let table: [(&str, Run); 3] = [
        (VIDEO_RENDER, |s, c| Box::pin(render_one(s, c))),
        (VIDEOS_RENDER_BATCH, |s, c| Box::pin(render_batch(s, c))),
        (VIDEO_STITCH, |s, c| Box::pin(stitch(s, c))),
    ];
    for (kind, run) in table {
        let owned = Arc::clone(state);
        let handler: Handler = Arc::new(move |ctx| run(Arc::clone(&owned), ctx));
        state.jobs.register(kind, handler);
    }
}

pub(crate) fn fail(error: impl Into<ApiError>) -> JobError {
    error.into().into_job_error()
}

pub(crate) async fn context_for(state: &AppState, ctx: &JobCtx) -> Result<(RunContext, std::path::PathBuf), JobError> {
    let pid = ctx.project_id.clone().ok_or_else(|| fail(ApiError::validation("project_id is required.")))?;
    let folder = resolve_project(&state.roots, &pid).map_err(fail)?;
    let mut run = connect(&state.cfg).await.map_err(|e| {
        fail(ApiError::new(axum::http::StatusCode::SERVICE_UNAVAILABLE, "COMFY_UNAVAILABLE", e).retryable())
    })?;
    run.cancel = Arc::clone(&ctx.cancel);
    Ok((run, folder))
}

pub(crate) fn render_params(params: &Value) -> RenderParams {
    let mut overrides = Obj::new();
    if let Some(map) = params.as_object() {
        for (key, value) in map {
            if !CONTROL_KEYS.contains(&key.as_str()) && !value.is_null() {
                overrides.insert(key.clone(), value.clone());
            }
        }
    }
    RenderParams { overrides, timeout_hours: params.get("timeout_hours").and_then(Value::as_f64) }
}

pub(crate) fn finish_options(params: &Value) -> FinishOptions {
    FinishOptions { match_start_color: params.get("match_start_color").and_then(Value::as_bool).unwrap_or(false), ..FinishOptions::default() }
}

/// "Rendering video for scene 3 of 8 (seg_0003)..." gives `(3, 8)`.
fn batch_position(message: &str) -> Option<(i64, i64)> {
    let rest = message.strip_prefix("Rendering video for scene ")?;
    let (index, rest) = rest.split_once(" of ")?;
    let (count, _) = rest.split_once(' ')?;
    Some((index.parse().ok()?, count.parse().ok()?))
}

pub(crate) fn reporter(ctx: &JobCtx, scene_id: Option<String>) -> impl FnMut(RenderEvent) + Send + use<> {
    let ctx = ctx.clone();
    move |event: RenderEvent| {
        let position = batch_position(&event.message);
        ctx.log("info", &event.message);
        ctx.progress(ProgressUpdate {
            percent: event.percent,
            stage: event.stage.to_string(),
            message: event.message,
            scene_id: scene_id.clone(),
            scene_index: position.map(|p| p.0),
            scene_count: position.map(|p| p.1),
            stage_index: position.map_or(0, |p| p.0),
            stage_count: position.map_or(1, |p| p.1),
        });
    }
}

fn scene_ref(params: &Value) -> Result<String, JobError> {
    match params.get("scene_id") {
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(s.trim().to_string()),
        Some(Value::Number(n)) => Ok(n.to_string()),
        _ => Err(fail(ApiError::validation("scene_id is required."))),
    }
}

async fn render_one(state: Arc<AppState>, ctx: JobCtx) -> Result<Value, JobError> {
    let scene = scene_ref(&ctx.params)?;
    let (mut run, folder) = context_for(&state, &ctx).await?;
    run.before_scene = Some(crate::continued_prompt::hook(&state));
    let done = render_scene_full(&run, &folder, &scene, render_params(&ctx.params), finish_options(&ctx.params), reporter(&ctx, Some(scene.clone())))
        .await
        .map_err(fail)?;
    Ok(json!({
        "scene_id": done.scene_id,
        "scene_number": done.scene_number,
        "video_path": done.video_path,
        "thumbnail_path": done.thumbnail_path,
        "backup_path": done.backup_path,
        "raw_video_path": done.raw_video_path,
        "history_index": done.history_index,
        "history_count": done.history_count,
        "revision": done.revision,
    }))
}

fn scene_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|v| match v {
                    Value::String(s) => Some(s.clone()),
                    Value::Number(n) => Some(n.to_string()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

async fn render_batch(state: Arc<AppState>, ctx: JobCtx) -> Result<Value, JobError> {
    let params = &ctx.params;
    let (mut run, folder) = context_for(&state, &ctx).await?;
    run.before_scene = Some(crate::continued_prompt::hook(&state));
    let range = params.get("range").and_then(Value::as_array).filter(|r| r.len() == 2).and_then(|r| Some((r[0].as_u64()? as usize, r[1].as_u64()? as usize)));
    let resume = params.get("resume").and_then(Value::as_bool).unwrap_or(false) || params.get("run_mode").and_then(Value::as_str) == Some("resume_missing");
    let options = BatchOptions {
        scope: params.get("scope").and_then(Value::as_str).unwrap_or("all").to_string(),
        scene_ids: scene_list(params.get("scene_ids")),
        range,
        force: params.get("force").and_then(Value::as_bool).unwrap_or(false),
        skip_existing: resume || params.get("run_mode").and_then(Value::as_str) == Some("missing"),
        skip_final_stitch: params.get("skip_final_stitch").and_then(Value::as_bool).unwrap_or(false),
        params: render_params(params),
        finish: finish_options(params),
    };
    let result = render_all(&run, &folder, options, reporter(&ctx, None)).await.map_err(fail)?;
    if result.results.is_empty() {
        return Ok(json!({"processed": 0, "message": "No scenes matched batch video render criteria."}));
    }
    let results: Vec<Value> = result
        .results
        .iter()
        .map(|r| match &r.error {
            Some(error) => json!({"scene_id": r.scene_id, "status": "error", "error": error}),
            None => json!({"scene_id": r.scene_id, "status": "success", "video_path": r.video_path}),
        })
        .collect();
    let failed = result.results.iter().filter(|r| r.error.is_some()).count();
    // The pack ends such a job as succeeded with an error per scene. A job where nothing worked fails, so it can be retried.
    if failed == results.len() {
        let first = result.results.iter().find_map(|r| r.error.clone()).unwrap_or_default();
        let mut error = JobError::new("BATCH_FAILED", format!("Every scene failed. First error: {first}"));
        error.details = json!({"results": results});
        error.retryable = true;
        return Err(error);
    }
    Ok(json!({
        "processed": results.len(),
        "succeeded": results.len() - failed,
        "failed": failed,
        "results": results,
        "final_video_path": result.final_video_path.clone().unwrap_or_default(),
    }))
}

async fn stitch(state: Arc<AppState>, ctx: JobCtx) -> Result<Value, JobError> {
    let (run, folder) = context_for(&state, &ctx).await?;
    let params: Obj = ctx.params.as_object().cloned().unwrap_or_default();
    ctx.progress(ProgressUpdate { percent: 10.0, stage: "stitching".into(), message: "Stitching the scene videos...".into(), stage_count: 1, ..Default::default() });
    let (result, summary) = stitch_project(&run, &folder, &params, true).await.map_err(fail)?;
    Ok(json!({
        "final_video_path": result.final_video_path.display().to_string(),
        "video_folder": result.video_folder.display().to_string(),
        "scene_count": result.scene_count,
        "scene_numbers": summary.scene_numbers,
        "audio": summary.audio,
        "used_scene_audio": result.used_scene_audio,
        "embedded_audio_mode": result.embedded_audio_mode,
        "timeline_frame_sync": result.timeline_frame_sync,
        "timeline_frame_count": result.timeline_frame_count,
        "output_width": result.output_width,
        "output_height": result.output_height,
        "output_duration": result.output_duration,
        "output_audio_duration": result.output_audio_duration,
    }))
}
