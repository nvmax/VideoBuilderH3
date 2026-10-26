//! The story steps as jobs: `POST /projects/{pid}/story/{step}`. The steps are `h3_builder::story::agent`. This module runs them on the LLM the app is set to
//! use and saves their work under the edit lock. Port of the `storyboard.*` job types of the pack.

#![allow(clippy::result_large_err)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Path as UrlPath, State};
use axum::Json;
use h3_builder::prompt_writers::AppLlm;
use h3_builder::story::agent::{self, Change, SaveFuture, StepFailure, Store, STEPS};
use h3_core::pyval::Obj;
use h3_jobs::{Handler, JobCtx, JobError, ProgressUpdate};
use h3_project::story::edit_session;
use h3_project::{load_session, Stamp};
use serde_json::Value;

use crate::app::{body_object, job_response, load, AppState, Shared};
use crate::error::{ApiError, ApiResult};
use crate::session_routes::op_error;
use crate::views::resolve_project;

/// Each step and the job type it runs as.
const JOB_TYPES: [(&str, &str); 12] = [
    ("arc", "storyboard.story_arc"),
    ("brief", "storyboard.story_brief"),
    ("beats", "storyboard.scene_beats"),
    ("book", "storyboard.story_book"),
    ("chapter", "storyboard.story_chapter"),
    ("split", "storyboard.story_split"),
    ("convert", "storyboard.story_convert"),
    ("cast", "storyboard.story_cast"),
    ("remove_people", "storyboard.story_remove_people"),
    ("acts", "storyboard.story_acts"),
    ("plan", "storyboard.story_plan"),
    ("locations", "storyboard.story_locations"),
];

pub fn register(state: &Arc<AppState>) {
    for (step, kind) in JOB_TYPES {
        let owned = Arc::clone(state);
        let handler: Handler = Arc::new(move |ctx| {
            let state = Arc::clone(&owned);
            Box::pin(run(state, step, ctx))
        });
        state.jobs.register(kind, handler);
    }
}

/// The project a step works on: read from disk, and saved under the edit lock so an agent's other edits do not interleave.
struct ProjectStore {
    state: Arc<AppState>,
    folder: PathBuf,
}

impl Store for ProjectStore {
    fn folder(&self) -> &Path {
        &self.folder
    }

    fn read(&self) -> Result<Obj, StepFailure> {
        load_session(&self.folder).map(|loaded| loaded.session).map_err(|e| StepFailure::Failed(e.to_string()))
    }

    fn save(&self, change: Change) -> SaveFuture<'_> {
        Box::pin(async move {
            let _guard = self.state.edit_lock.lock().await;
            let folder = self.folder.clone();
            let saved = tokio::task::spawn_blocking(move || edit_session(&folder, None, &Stamp::now(), change)).await.map_err(|e| StepFailure::Failed(e.to_string()))?;
            saved.map_err(StepFailure::from)
        })
    }
}

fn job_error(failure: StepFailure, pid: &str) -> JobError {
    match failure {
        StepFailure::Invalid(message) => ApiError::validation(message).into_job_error(),
        StepFailure::Failed(message) => JobError::new("STORY_STEP_FAILED", message),
        StepFailure::Op(op) => op_error(op, pid).into_job_error(),
    }
}

async fn run(state: Arc<AppState>, step: &'static str, ctx: JobCtx) -> Result<Value, JobError> {
    let pid = ctx.project_id.clone().ok_or_else(|| ApiError::validation("project_id is required.").into_job_error())?;
    let folder = resolve_project(&state.roots, &pid).map_err(|e| e.into_job_error())?;
    let params = ctx.params.as_object().cloned().unwrap_or_default();
    let config = crate::vrgdg::llm::base_config(&state);
    let llm = AppLlm { config: &config };
    ctx.progress(ProgressUpdate { percent: 10.0, stage: format!("story_{step}"), message: format!("Writing the story ({step}) with the LLM..."), stage_count: 1, ..Default::default() });
    let report = |done: usize, total: usize, label: &str| {
        ctx.progress(ProgressUpdate {
            percent: 5.0 + 90.0 * done as f64 / total.max(1) as f64,
            stage: format!("story_{step}"),
            message: format!("Story {step} {done}/{total} ({label})"),
            scene_index: Some(done as i64),
            scene_count: Some(total as i64),
            stage_index: done as i64,
            stage_count: total as i64,
            ..Default::default()
        });
    };
    let store = ProjectStore { state, folder };
    agent::run_step(&llm, &store, step, &params, &report).await.map_err(|failure| job_error(failure, &pid))
}

/// `POST /projects/{pid}/story/{step}`: write the story arc, the story brief, the scene beats or a Story Mode step with the project's LLM, as a job.
pub async fn story_step(State(state): Shared, UrlPath((pid, step)): UrlPath<(String, String)>, body: Option<Json<Value>>) -> ApiResult {
    let Some((_, kind)) = JOB_TYPES.iter().find(|(name, _)| *name == step) else {
        return Err(ApiError::validation(format!("step must be one of: {}.", STEPS.join(", "))));
    };
    let params = body_object(body)?;
    load(&state, &pid).await?;
    // The Gemma in ComfyUI takes the GPU. Every other runner has its own process, so the job does not wait behind a render.
    let gpu = crate::llm_routes::current(&state).runner == "comfyui_gemma";
    let job = state.jobs.submit(kind, Some(&pid), Value::Object(params), gpu)?;
    Ok(job_response(&job))
}
