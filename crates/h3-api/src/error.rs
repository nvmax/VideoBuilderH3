//! The `/api/v1` envelopes and error codes. Port of `agent_api/envelope.py` and `agent_api/errors.py`.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use h3_comfy::ComfyError;
use h3_jobs::{JobError, ManagerError};
use h3_pipeline::RenderError;
use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
    pub details: Value,
    pub retryable: bool,
}

pub type ApiResult = Result<Response, ApiError>;

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self { status, code, message: message.into(), details: json!({}), retryable: false }
    }

    pub fn details(mut self, details: Value) -> Self {
        self.details = details;
        self
    }

    pub fn retryable(mut self) -> Self {
        self.retryable = true;
        self
    }

    pub fn validation(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "VALIDATION_ERROR", message)
    }

    pub fn project_not_found(id: &str) -> Self {
        Self::new(StatusCode::NOT_FOUND, "PROJECT_NOT_FOUND", format!("Project '{id}' was not found.")).details(json!({"project_id": id}))
    }

    pub fn scene_not_found(scene: &str, project: &str) -> Self {
        Self::new(StatusCode::NOT_FOUND, "SCENE_NOT_FOUND", format!("Scene '{scene}' was not found in project '{project}'."))
            .details(json!({"scene_id": scene, "project_id": project}))
    }

    pub fn revision_conflict(current: i64, incoming: i64) -> Self {
        Self::new(
            StatusCode::CONFLICT,
            "REVISION_CONFLICT",
            format!("Project revision conflict: incoming revision {incoming} < saved revision {current}."),
        )
        .details(json!({"current_revision": current, "incoming_revision": incoming}))
        .retryable()
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR", message)
    }

    pub fn body(&self) -> Value {
        json!({"code": self.code, "message": self.message, "details": self.details, "retryable": self.retryable})
    }

    /// The same error as a job failure, for a job that ends with it.
    pub fn into_job_error(self) -> JobError {
        JobError { code: self.code.to_string(), message: self.message, details: self.details, retryable: self.retryable }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({"ok": false, "error": self.body()}))).into_response()
    }
}

impl From<ManagerError> for ApiError {
    fn from(e: ManagerError) -> Self {
        match e {
            ManagerError::NotFound(id) => Self::new(StatusCode::NOT_FOUND, "JOB_NOT_FOUND", format!("Job '{id}' was not found.")).details(json!({"job_id": id})),
            ManagerError::Invalid(message) => Self::validation(message),
            ManagerError::Store(e) => Self::internal(e.to_string()),
        }
    }
}

impl From<RenderError> for ApiError {
    fn from(e: RenderError) -> Self {
        let message = e.to_string();
        match e {
            RenderError::SceneNotFound(scene) => Self::new(StatusCode::NOT_FOUND, "SCENE_NOT_FOUND", message).details(json!({"scene_id": scene})),
            RenderError::Invalid(_) => Self::validation(message),
            RenderError::PredecessorMissing { scene, predecessor } => Self::new(StatusCode::CONFLICT, "PREDECESSOR_MISSING", message)
                .details(json!({"scene_number": scene, "predecessor_scene": predecessor, "required_scene": predecessor})),
            RenderError::LatentStale { scene, stale } => {
                Self::new(StatusCode::CONFLICT, "LATENT_STALE", message).details(json!({"scene_number": scene, "stale_scene": stale, "required_scene": stale}))
            }
            RenderError::Graph(_) => Self::validation(message),
            RenderError::Comfy(c) => c.into(),
            RenderError::Other(_) => Self::internal(message),
        }
    }
}

impl From<ComfyError> for ApiError {
    fn from(e: ComfyError) -> Self {
        let message = e.to_string();
        match e {
            ComfyError::Queue { node_errors, .. } => Self::new(StatusCode::BAD_GATEWAY, "COMFY_QUEUE_REJECTED", message).details(json!({"node_errors": node_errors})),
            ComfyError::Execution(_) => Self::new(StatusCode::BAD_GATEWAY, "COMFY_NODE_ERRORS", message).retryable(),
            ComfyError::Cancelled => Self::new(StatusCode::CONFLICT, "JOB_CANCELLED", "Job execution was cancelled."),
            ComfyError::Timeout { prompt_id, seconds } => {
                Self::new(StatusCode::GATEWAY_TIMEOUT, "COMFY_TIMEOUT", message).details(json!({"prompt_id": prompt_id, "timeout_seconds": seconds})).retryable()
            }
            _ => Self::new(StatusCode::BAD_GATEWAY, "COMFY_UNAVAILABLE", message).retryable(),
        }
    }
}

/// `{"ok": true, "data": ..., "revision": n}` with status 200.
pub fn success(data: Value, revision: Option<i64>) -> Response {
    respond(StatusCode::OK, data, revision)
}

pub fn respond(status: StatusCode, data: Value, revision: Option<i64>) -> Response {
    let mut body = json!({"ok": true, "data": data});
    if let Some(revision) = revision {
        body["revision"] = json!(revision);
    }
    (status, Json(body)).into_response()
}
