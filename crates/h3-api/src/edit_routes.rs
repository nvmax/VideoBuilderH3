//! Timeline and scene editing routes. Each one runs a golden-tested operation of `h3_project::scene_ops`.

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use h3_project::scene_ops::{self, OpError};
use h3_project::Stamp;
use serde_json::{json, Value};

use crate::app::{blocking, body_object, if_match, Shared};
use crate::error::{respond, success, ApiError, ApiResult};
use crate::views;

/// `seg_` and twelve hex digits, the form the Video Builder uses.
pub fn new_scene_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    format!("seg_{:012x}", hasher.finish() & 0xffff_ffff_ffff)
}

fn op_error(e: OpError, pid: &str) -> ApiError {
    match e {
        OpError::SceneNotFound(id) => ApiError::scene_not_found(&id, pid),
        OpError::Validation(m) => ApiError::validation(m),
        OpError::Conflict { current, incoming } => ApiError::revision_conflict(current, incoming),
        OpError::Io(e) => ApiError::internal(e.to_string()),
    }
}

fn number(body: &h3_core::pyval::Obj, key: &str) -> Option<f64> {
    body.get(key).and_then(h3_core::pyval::to_float)
}

fn text(body: &h3_core::pyval::Obj, key: &str) -> String {
    body.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

/// Run an editing operation on the project folder with edits serialized, and wrap its result.
async fn edit(
    state: &crate::app::AppState,
    pid: &str,
    status: StatusCode,
    op: impl FnOnce(&std::path::Path, &Stamp) -> Result<Value, OpError> + Send + 'static,
) -> ApiResult {
    let folder = views::resolve_project(&state.roots, pid)?;
    let _guard = state.edit_lock.lock().await;
    let result = blocking(move || op(&folder, &Stamp::now())).await?.map_err(|e| op_error(e, pid))?;
    let revision = result.get("revision").and_then(Value::as_i64);
    Ok(respond(status, result, revision))
}

pub async fn create_scene(State(state): Shared, Path(pid): Path<String>, headers: HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    let body = body_object(body)?;
    let expected = if_match(&headers)?;
    edit(&state, &pid, StatusCode::OK, move |folder, now| {
        let mut id = new_scene_id;
        scene_ops::create_scene(
            folder,
            body.get("position").and_then(Value::as_str).unwrap_or("append"),
            body.get("ref_scene_id").and_then(Value::as_str),
            number(&body, "duration").unwrap_or(4.0),
            &text(&body, "label"),
            &text(&body, "t2i_prompt"),
            &text(&body, "i2v_prompt"),
            &text(&body, "notes"),
            expected,
            now,
            &mut id,
        )
    })
    .await
}

pub async fn delete_scene(State(state): Shared, Path((pid, sid)): Path<(String, String)>, headers: HeaderMap, Query(q): Query<HashMap<String, String>>) -> ApiResult {
    let ripple = q.get("ripple").is_none_or(|v| !matches!(v.to_lowercase().as_str(), "0" | "false" | "no"));
    let expected = if_match(&headers)?;
    edit(&state, &pid, StatusCode::OK, move |folder, now| scene_ops::delete_scene(folder, &sid, ripple, expected, now)).await
}

pub async fn split_scene(State(state): Shared, Path((pid, sid)): Path<(String, String)>, headers: HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    let body = body_object(body)?;
    let at = number(&body, "at_time").ok_or_else(|| ApiError::validation("at_time is required."))?;
    let clear = body.get("clear_right_media").and_then(Value::as_bool).unwrap_or(true);
    let expected = if_match(&headers)?;
    edit(&state, &pid, StatusCode::OK, move |folder, now| {
        let mut id = new_scene_id;
        scene_ops::split_scene(folder, &sid, at, clear, expected, now, &mut id)
    })
    .await
}

pub async fn merge_scene(State(state): Shared, Path((pid, sid)): Path<(String, String)>, headers: HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    let body = body_object(body)?;
    let direction = body.get("with_direction").and_then(Value::as_str).unwrap_or("next").to_string();
    let expected = if_match(&headers)?;
    edit(&state, &pid, StatusCode::OK, move |folder, now| scene_ops::merge_scenes(folder, &sid, &direction, expected, now)).await
}

pub async fn move_scene(State(state): Shared, Path((pid, sid)): Path<(String, String)>, headers: HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    let body = body_object(body)?;
    let start = number(&body, "start_time").ok_or_else(|| ApiError::validation("start_time is required."))?;
    let ripple = body.get("ripple").and_then(Value::as_bool).unwrap_or(false);
    let expected = if_match(&headers)?;
    edit(&state, &pid, StatusCode::OK, move |folder, now| scene_ops::move_scene(folder, &sid, start, ripple, expected, now)).await
}

pub async fn resize_scene(State(state): Shared, Path((pid, sid)): Path<(String, String)>, headers: HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    let body = body_object(body)?;
    let (duration, end) = (number(&body, "duration"), number(&body, "end_time"));
    let ripple = body.get("ripple").and_then(Value::as_bool).unwrap_or(false);
    let expected = if_match(&headers)?;
    edit(&state, &pid, StatusCode::OK, move |folder, now| scene_ops::resize_scene(folder, &sid, duration, end, ripple, expected, now)).await
}

pub async fn timeline_bulk(State(state): Shared, Path(pid): Path<String>, body: Option<Json<Value>>) -> ApiResult {
    let body = body_object(body)?;
    let mode = body.get("mode").and_then(Value::as_str).unwrap_or("durations").to_string();
    let action = body.get("action").and_then(Value::as_str).unwrap_or("replace").to_string();
    let text_in = text(&body, "text");
    let append_start = number(&body, "append_start").unwrap_or(0.0);
    edit(&state, &pid, StatusCode::OK, move |folder, now| {
        let mut id = new_scene_id;
        scene_ops::timeline_bulk(folder, &text_in, &mode, &action, append_start, now, &mut id)
    })
    .await
}

pub async fn close_gaps(State(state): Shared, Path(pid): Path<String>) -> ApiResult {
    edit(&state, &pid, StatusCode::OK, |folder, now| scene_ops::close_gaps(folder, now)).await
}

pub async fn snap(State(state): Shared, Path(pid): Path<String>, body: Option<Json<Value>>) -> ApiResult {
    let body = body_object(body)?;
    let scope = body.get("scope").and_then(Value::as_str).unwrap_or("edge").to_string();
    let scene = body.get("scene_id").and_then(Value::as_str).map(str::to_string);
    let edge = body.get("edge").and_then(Value::as_str).unwrap_or("start").to_string();
    edit(&state, &pid, StatusCode::OK, move |folder, now| scene_ops::snap_to_beats(folder, &scope, scene.as_deref(), &edge, now)).await
}

/// Check the timeline for gaps, overlaps and a length past the audio.
pub async fn validate(State(state): Shared, Path(pid): Path<String>) -> ApiResult {
    let loaded = crate::app::load(&state, &pid).await?;
    let mut segs: Vec<Value> = loaded.session.get("segments").and_then(Value::as_array).cloned().unwrap_or_default();
    let audio = loaded.session.get("audio_duration").and_then(h3_core::pyval::to_float);
    let issues = h3_core::timeline::validate_timeline_consistency(&mut segs, audio);
    Ok(success(json!({"valid": issues.is_empty(), "issues": issues}), Some(loaded.revision)))
}
