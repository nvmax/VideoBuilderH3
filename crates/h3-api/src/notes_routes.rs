//! Timed Timeline Notes: list, add, change and delete. Port of the notes routes in `agent_api/router.py`.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use h3_project::timeline_notes::{self, NoteError};
use h3_project::Stamp;
use serde_json::{json, Value};

use crate::app::{blocking, body_object, if_match, load, Shared};
use crate::error::{respond, success, ApiError, ApiResult};
use crate::views::resolve_project;

fn note_error(e: NoteError, pid: &str) -> ApiError {
    use h3_project::scene_ops::OpError;
    match e {
        NoteError::NotFound(id) => ApiError::new(StatusCode::NOT_FOUND, "TIMELINE_NOTE_NOT_FOUND", format!("Timeline note '{id}' was not found in project '{pid}'.")).details(json!({"note_id": id, "project_id": pid})),
        NoteError::Op(OpError::Validation(m)) => ApiError::validation(m),
        NoteError::Op(OpError::Conflict { current, incoming }) => ApiError::revision_conflict(current, incoming),
        NoteError::Op(OpError::SceneNotFound(id)) => ApiError::scene_not_found(&id, pid),
        NoteError::Op(OpError::Io(e)) => ApiError::internal(e.to_string()),
    }
}

pub async fn list_notes(State(state): Shared, Path(pid): Path<String>) -> ApiResult {
    let loaded = load(&state, &pid).await?;
    let result = timeline_notes::list_notes(&loaded.session);
    let revision = result.get("revision").and_then(Value::as_i64);
    Ok(success(result, revision))
}

async fn run(state: &crate::app::AppState, pid: &str, status: StatusCode, op: impl FnOnce(&std::path::Path, &Stamp) -> Result<Value, NoteError> + Send + 'static) -> ApiResult {
    let folder = resolve_project(&state.roots, pid)?;
    let _guard = state.edit_lock.lock().await;
    let result = blocking(move || op(&folder, &Stamp::now())).await?.map_err(|e| note_error(e, pid))?;
    let revision = result.get("revision").and_then(Value::as_i64);
    Ok(respond(status, result, revision))
}

pub async fn create_note(State(state): Shared, Path(pid): Path<String>, headers: HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    let fields = Value::Object(body_object(body)?);
    let expected = if_match(&headers)?;
    run(&state, &pid, StatusCode::CREATED, move |folder, now| timeline_notes::create_note(folder, &fields, expected, now)).await
}

pub async fn update_note(State(state): Shared, Path((pid, nid)): Path<(String, String)>, headers: HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    let fields = Value::Object(body_object(body)?);
    let expected = if_match(&headers)?;
    run(&state, &pid, StatusCode::OK, move |folder, now| timeline_notes::update_note(folder, &nid, &fields, expected, now)).await
}

pub async fn delete_note(State(state): Shared, Path((pid, nid)): Path<(String, String)>, headers: HeaderMap) -> ApiResult {
    let expected = if_match(&headers)?;
    run(&state, &pid, StatusCode::OK, move |folder, now| timeline_notes::delete_note(folder, &nid, expected, now)).await
}
