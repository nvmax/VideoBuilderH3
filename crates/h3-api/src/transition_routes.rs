//! Transitions for agents: the catalog (`GET /transitions`), and a project's transitions, the blocks of the timeline's transition lane (`GET/PUT
//! /projects/{pid}/transitions`, `POST .../transitions/set`). They are the `scene_transitions` key of the session, the same one the Video Builder's
//! timeline saves, and every stitch draws them.

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use h3_media::transition::TransitionItem;
use h3_project::transition_project;
use h3_project::scene_ops::OpError;
use h3_project::Stamp;
use serde_json::{json, Value};

use crate::app::{blocking, body_object, if_match, load, Shared};
use crate::error::{success, ApiError, ApiResult};
use crate::views::resolve_project;

fn op_error(e: OpError) -> ApiError {
    match e {
        OpError::Validation(m) => ApiError::validation(m),
        OpError::Conflict { current, incoming } => ApiError::revision_conflict(current, incoming),
        OpError::SceneNotFound(id) => ApiError::validation(format!("Scene '{id}' was not found.")),
        OpError::Io(e) => ApiError::internal(e.to_string()),
    }
}

/// An entry is checked the way a stitch reads it: a known transition, parameters of the right shape, a `seconds` that means something. Where it sits
/// (`start`, or `after_scene` and `align`) is read against the project's scenes when it is saved.
fn check_entry(entry: &Value) -> Result<(), String> {
    let mut map = entry.as_object().ok_or("It must be an object.")?.clone();
    if map.get("start").is_none_or(Value::is_null) {
        map.insert("start".into(), json!(0));
    }
    TransitionItem::from_json(&Value::Object(map)).map(|_| ())
}

/// `GET /transitions`: every transition with its author, license, tags and parameters (name, type, default, range).
pub async fn catalog() -> ApiResult {
    let list = h3_transitions::catalog();
    Ok(success(json!({"count": list.len(), "transitions": list}), None))
}

/// `GET /projects/{pid}/transitions`: the project's transitions in timeline order, each as `{id, transition, start, end, seconds, params}`, with the revision.
pub async fn get_transitions(State(state): Shared, Path(pid): Path<String>) -> ApiResult {
    let loaded = load(&state, &pid).await?;
    let view = blocking(move || transition_project::transitions_view(&loaded.session)).await?;
    let revision = view.get("revision").and_then(Value::as_i64);
    Ok(success(view, revision))
}

/// `PUT /projects/{pid}/transitions`: `{scene_transitions: [...]}`. Replaces the whole list. An entry is `{transition, start, seconds?, params?, id?}`, or
/// `{transition, after_scene, align?, seconds?, params?}` to put it on a cut.
pub async fn put_transitions(State(state): Shared, Path(pid): Path<String>, headers: HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    let fields = body_object(body)?;
    if let Some(unknown) = fields.keys().find(|k| *k != transition_project::KEY) {
        return Err(ApiError::validation(format!("Unknown field `{unknown}`. Send {}.", transition_project::KEY)));
    }
    let Some(list) = fields.get(transition_project::KEY).and_then(Value::as_array).cloned() else {
        return Err(ApiError::validation(format!("{} must be a list of {{transition, start, seconds, params}} items.", transition_project::KEY)));
    };
    for (n, entry) in list.iter().enumerate() {
        check_entry(entry).map_err(|e| ApiError::validation(format!("Transition {}: {e}", n + 1)))?;
    }
    let expected = if_match(&headers)?;
    let folder = resolve_project(&state.roots, &pid)?;
    let _guard = state.edit_lock.lock().await;
    let view = blocking(move || transition_project::put_transitions(&folder, &list, expected, &Stamp::now())).await?.map_err(op_error)?;
    let revision = view.get("revision").and_then(Value::as_i64);
    Ok(success(view, revision))
}

/// `POST /projects/{pid}/transitions/set`: `{id?, transition, start | after_scene, align?, seconds?, params?}`. Adds a transition at `start`, or on the cut after
/// a scene (`after_scene`, placed around the cut by `align`: center, end or start). With the `id` of an existing one it changes that one (only the fields sent).
/// A `transition` that is null, empty or `none` removes the one with that `id`, or those on the cut after `after_scene`. The others are untouched.
pub async fn set_transition(State(state): Shared, Path(pid): Path<String>, headers: HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    let fields = body_object(body)?;
    let supported = ["id", "after_scene", "start", "transition", "seconds", "align", "params"];
    if let Some(unknown) = fields.keys().find(|k| !supported.contains(&k.as_str())) {
        return Err(ApiError::validation(format!("Unknown field `{unknown}`. Supported: {}.", supported.join(", "))));
    }
    let named = fields.get("transition").is_some_and(|t| !t.is_null() && !t.as_str().is_some_and(|s| s.trim().is_empty() || s.trim().eq_ignore_ascii_case("none")));
    if named {
        check_entry(&Value::Object(fields.clone())).map_err(ApiError::validation)?;
    }
    let expected = if_match(&headers)?;
    let folder = resolve_project(&state.roots, &pid)?;
    let _guard = state.edit_lock.lock().await;
    let view = blocking(move || transition_project::set_transition(&folder, &fields, expected, &Stamp::now())).await?.map_err(op_error)?;
    let revision = view.get("revision").and_then(Value::as_i64);
    Ok(success(view, revision))
}
