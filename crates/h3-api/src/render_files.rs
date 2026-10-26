//! Files an agent makes for a render: a saved RefMod from pictures (`POST /refmods`) and the song split into stems (`POST /projects/{pid}/audio/stems`).
//! Both run as GPU jobs. The work is the same code the Builder's own buttons call. Ports of `agent_api/refmod_stems.py`.

#![allow(clippy::result_large_err)]

use std::path::Path;
use std::sync::Arc;

use axum::extract::{Path as UrlPath, State};
use axum::Json;
use h3_builder::audio_mask;
use h3_builder::refmod_studio;
use h3_core::pyval::{text_or_empty, Obj};
use h3_jobs::{Handler, JobCtx, JobError, ProgressUpdate};
use serde_json::{json, Value};

use crate::app::{blocking, body_object, job_response, load, AppState, Shared};
use crate::error::{ApiError, ApiResult};
use crate::vrgdg::audio_mask::demucs_in_comfy;
use crate::vrgdg::refmod::{create_refmod, CreateError};
use crate::views::resolve_project;

pub const REFMOD_CREATE: &str = "refmod.create";
pub const AUDIO_STEMS: &str = "audio.stems";

pub fn register(state: &Arc<AppState>) {
    let owned = Arc::clone(state);
    let handler: Handler = Arc::new(move |ctx| Box::pin(refmod_job(Arc::clone(&owned), ctx)));
    state.jobs.register(REFMOD_CREATE, handler);
    let owned = Arc::clone(state);
    let handler: Handler = Arc::new(move |ctx| Box::pin(stems_job(Arc::clone(&owned), ctx)));
    state.jobs.register(AUDIO_STEMS, handler);
}

fn started(ctx: &JobCtx, stage: &str, message: &str) {
    ctx.progress(ProgressUpdate { percent: 10.0, stage: stage.into(), message: message.into(), stage_count: 1, ..Default::default() });
}

// -- a RefMod from pictures -------------------------------------------------------------------------------------------------

/// `POST /refmods`: make a saved RefMod from pictures, like RefMods Studio's Create, as a GPU job. A request that cannot work is a 400 with no job.
pub async fn create(State(state): Shared, body: Option<Json<Value>>) -> ApiResult {
    let body = body_object(body)?;
    let params: Obj = ["name", "type", "paths", "crops", "quality", "mode", "steps", "description", "overwrite"]
        .iter()
        .filter_map(|key| body.get(*key).filter(|v| !v.is_null()).map(|v| (key.to_string(), v.clone())))
        .collect();
    refmod_studio::validate_request(&params).map_err(ApiError::validation)?;
    let job = state.jobs.submit(REFMOD_CREATE, None, Value::Object(params), true)?;
    Ok(job_response(&job))
}

async fn refmod_job(state: Arc<AppState>, ctx: JobCtx) -> Result<Value, JobError> {
    started(&ctx, "refmod", "Encoding the pictures into a RefMod...");
    let payload = ctx.params.as_object().cloned().unwrap_or_default();
    match create_refmod(&state, payload).await {
        Ok(made) => Ok(made),
        Err(CreateError::Exists { path, message }) => Err(ApiError::validation(format!("{message} Send overwrite: true to replace it.")).details(json!({"exists": true, "path": path})).into_job_error()),
        Err(CreateError::Failed(message)) => Err(ApiError::validation(message).into_job_error()),
    }
}

// -- the song as stems ------------------------------------------------------------------------------------------------------

/// What the split is made from: the song (the project's own unless `audio_path` is sent) and the settings.
fn stems_request(folder: &Path, session: &Obj, params: &Obj) -> Result<Obj, ApiError> {
    let given = text_or_empty(params.get("audio_path")).trim().trim_matches('"').to_string();
    let song = if given.is_empty() { h3_pipeline::stitch_plan::session_audio_path(session) } else { given };
    if song.is_empty() || !Path::new(&song).is_file() {
        return Err(ApiError::validation("The project has no song to split. Attach one with PUT /audio, or send audio_path.").details(json!({"audio_path": song})));
    }
    let mut request = Obj::new();
    request.insert("project_folder".into(), json!(folder.display().to_string()));
    request.insert("audio_path".into(), json!(song));
    for key in ["model_name", "device", "input_gain_db"] {
        if let Some(value) = params.get(key).filter(|v| !v.is_null()) {
            request.insert(key.into(), value.clone());
        }
    }
    Ok(request)
}

/// `POST /projects/{pid}/audio/stems`: split the project's song into stems with Demucs and write a vocal-free mix, as a GPU job.
pub async fn stems(State(state): Shared, UrlPath(pid): UrlPath<String>, body: Option<Json<Value>>) -> ApiResult {
    let body = body_object(body)?;
    let loaded = load(&state, &pid).await?;
    stems_request(&loaded.project_folder, &loaded.session, &body)?;
    let params: Obj = ["audio_path", "model_name", "device", "input_gain_db"].iter().filter_map(|key| body.get(*key).filter(|v| !v.is_null()).map(|v| (key.to_string(), v.clone()))).collect();
    let job = state.jobs.submit(AUDIO_STEMS, Some(&pid), Value::Object(params), true)?;
    Ok(job_response(&job))
}

async fn stems_job(state: Arc<AppState>, ctx: JobCtx) -> Result<Value, JobError> {
    let pid = ctx.project_id.clone().ok_or_else(|| ApiError::validation("project_id is required.").into_job_error())?;
    let folder = resolve_project(&state.roots, &pid).map_err(|e| e.into_job_error())?;
    started(&ctx, "stems", "Splitting the song into stems with Demucs...");
    let loaded = load(&state, &pid).await.map_err(|e| e.into_job_error())?;
    let params = ctx.params.as_object().cloned().unwrap_or_default();
    let request = stems_request(&folder, &loaded.session, &params).map_err(|e| e.into_job_error())?;
    let env = state.builder_env();
    let base = state.cfg.comfy_base().to_string();
    let handle = tokio::runtime::Handle::current();
    let split = blocking(move || {
        let separator = |context: &Path, model: &str, device: &str| handle.block_on(demucs_in_comfy(&base, &env, context, model, device));
        audio_mask::separate_project_audio(&env, &request, &separator)
    })
    .await
    .map_err(|e| e.into_job_error())?;
    let result = split.map_err(|message| ApiError::validation(message).into_job_error())?;
    ctx.progress(ProgressUpdate { percent: 100.0, stage: "stems".into(), message: "The song is split.".into(), stage_count: 1, ..Default::default() });
    Ok(result)
}
