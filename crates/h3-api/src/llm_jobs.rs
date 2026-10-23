//! LLM jobs: writing MiniMax prompts for scenes.

#![allow(clippy::result_large_err)]

use std::sync::Arc;

use h3_core::pyval::{text_or_empty, Obj};
use h3_core::timeline::sort_segments;
use h3_jobs::{Handler, JobCtx, JobError, ProgressUpdate};
use h3_pipeline::minimax_prompts::{generate_scene_prompt, PromptError, PromptParams};
use h3_project::{load_session, session_ops, Stamp};
use serde_json::{json, Value};

use crate::app::AppState;
use crate::error::ApiError;
use crate::llm_routes::current;
use crate::views::resolve_project;

pub const MINIMAX_PROMPTS: &str = "minimax.prompts";

type Run = fn(Arc<AppState>, JobCtx) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, JobError>> + Send>>;

pub fn register(state: &Arc<AppState>) {
    let table: [(&str, Run); 1] = [(MINIMAX_PROMPTS, |s, c| Box::pin(minimax_prompts(s, c)))];
    for (kind, run) in table {
        let owned = Arc::clone(state);
        let handler: Handler = Arc::new(move |ctx| run(Arc::clone(&owned), ctx));
        state.jobs.register(kind, handler);
    }
}

fn fail(error: impl Into<ApiError>) -> JobError {
    error.into().into_job_error()
}

fn internal(e: impl std::fmt::Display) -> JobError {
    fail(ApiError::internal(e.to_string()))
}

fn num(params: &Value, key: &str, default: f64) -> f64 {
    params.get(key).and_then(Value::as_f64).unwrap_or(default)
}

async fn read_session(folder: &std::path::Path) -> Result<h3_project::LoadedSession, JobError> {
    let folder = folder.to_path_buf();
    tokio::task::spawn_blocking(move || load_session(&folder)).await.map_err(internal)?.map_err(internal)
}

/// Write a prompt for each scene that has none (or the scenes named), saving after every scene. A scene that fails is
/// listed in `failures` and the rest go on.
async fn minimax_prompts(state: Arc<AppState>, ctx: JobCtx) -> Result<Value, JobError> {
    let pid = ctx.project_id.clone().ok_or_else(|| fail(ApiError::validation("project_id is required.")))?;
    let folder = resolve_project(&state.roots, &pid).map_err(fail)?;
    let loaded = read_session(&folder).await?;
    let mut segments: Vec<Value> = loaded.session.get("segments").and_then(Value::as_array).cloned().unwrap_or_default();
    if segments.is_empty() {
        return Err(fail(ApiError::validation("The project has no scenes. Create the timeline first.")));
    }
    sort_segments(&mut segments);
    let wanted: Vec<String> = ctx.params.get("scene_ids").and_then(Value::as_array).map(|a| a.iter().map(|v| text_or_empty(Some(v))).collect()).unwrap_or_default();
    let replace = ctx.params.get("replace_existing").and_then(Value::as_bool).unwrap_or(false);
    let limit = ctx.params.get("limit").and_then(Value::as_u64).unwrap_or(0) as usize;
    let mut targets: Vec<usize> = (0..segments.len())
        .filter(|i| {
            let id = text_or_empty(segments[*i].get("id"));
            let named = wanted.is_empty() || wanted.contains(&id) || wanted.contains(&(i + 1).to_string());
            named && (replace || !wanted.is_empty() || text_or_empty(segments[*i].get("minimax_h3_prompt")).trim().is_empty())
        })
        .collect();
    if limit > 0 {
        targets.truncate(limit);
    }
    let params = PromptParams { temperature: num(&ctx.params, "temperature", 0.45), top_p: num(&ctx.params, "top_p", 0.92), max_new_tokens: num(&ctx.params, "max_new_tokens", 4000.0) as u32 };
    let total = targets.len();
    let (mut created, mut failures) = (Vec::new(), Vec::new());
    let mut revision = loaded.revision;
    for (done, index) in targets.iter().enumerate() {
        if ctx.is_cancelled() {
            return Err(JobError::cancelled());
        }
        let id = text_or_empty(segments[*index].get("id"));
        ctx.progress(ProgressUpdate {
            percent: 5.0 + 90.0 * done as f64 / total.max(1) as f64,
            stage: "minimax_prompts".into(),
            message: format!("MiniMax prompts {done}/{total} (scene {})", index + 1),
            scene_id: Some(id.clone()),
            scene_index: Some(done as i64 + 1),
            scene_count: Some(total as i64),
            stage_index: done as i64 + 1,
            stage_count: total as i64,
        });
        // The project is read again for each scene, because the scenes written before it changed it.
        let session = read_session(&folder).await?.session;
        match generate_scene_prompt(&current(&state), &session, *index, &params).await {
            Ok(written) => {
                let mut fields = Obj::new();
                fields.insert("minimax_h3_prompt".into(), json!(written.prompt));
                fields.insert("minimax_h3_prompt_origin".into(), json!("gemma"));
                fields.insert("minimax_h3_mode".into(), json!("reference_to_video"));
                fields.insert("video_prompt_type".into(), json!("rtv"));
                let saved = {
                    let _guard = state.edit_lock.lock().await;
                    let (folder, id) = (folder.clone(), id.clone());
                    tokio::task::spawn_blocking(move || session_ops::set_scene_fields(&folder, &id, &fields, &Stamp::now())).await
                };
                let saved = saved.map_err(internal)?.map_err(internal)?;
                revision = saved["revision"].as_i64().unwrap_or(revision);
                created.push(json!({"scene_id": id, "scene_number": index + 1, "characters": written.characters, "shots": written.shots, "attempts": written.attempts, "model": written.model, "warnings": written.warnings}));
            }
            // The LLM being down fails the whole job. A scene's own problem fails only that scene.
            Err(PromptError::Llm(e @ h3_llm::LlmError::Unavailable(_))) => return Err(fail(ApiError::from(e))),
            Err(e) => failures.push(json!({"scene_id": id, "scene_number": index + 1, "error": e.to_string()})),
        }
    }
    if total > 0 && created.is_empty() {
        let first = failures.first().and_then(|f| f["error"].as_str()).unwrap_or("").to_string();
        let mut error = JobError::new("BATCH_FAILED", format!("No prompt could be written. First error: {first}"));
        error.details = json!({"failures": failures});
        return Err(error);
    }
    Ok(json!({"created": created.len(), "failed": failures.len(), "skipped": segments.len() - total, "prompts": created, "failures": failures, "revision": revision}))
}
