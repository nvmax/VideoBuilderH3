//! Effects and filters for agents: the catalogs (`GET /effects`, `GET /filters`), and a project's blocks of the effect lane and of the filter lane (`GET/PUT
//! /projects/{pid}/effects` and `/filters`, `POST .../effects/set` and `.../filters/set`). They are the `video_effects` and `video_filters` keys of the session,
//! the same ones the Video Builder's timeline saves, and every stitch draws them (filters first, then effects).

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use h3_media::looks::{Kind, LookItem};
use h3_project::looks_project::{self, LookKind};
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

fn engine(kind: LookKind) -> Kind {
    match kind {
        LookKind::Effect => Kind::Effect,
        LookKind::Filter => Kind::Filter,
    }
}

/// An entry is checked the way a stitch reads it: a known look of the right kind, settings of the right shape, a number for `seconds` and `intensity`.
fn check_entry(kind: LookKind, entry: &Value) -> Result<(), String> {
    let mut map = entry.as_object().ok_or("It must be an object.")?.clone();
    if map.get("start").is_none_or(Value::is_null) {
        map.insert("start".into(), json!(0));
    }
    LookItem::from_json(engine(kind), &Value::Object(map)).map(|_| ())
}

fn catalog(kind: LookKind) -> ApiResult {
    let list = h3_transitions::look_catalog(engine(kind));
    Ok(success(json!({"count": list.len(), format!("{}s", kind.name()): list}), None))
}

async fn get_looks(state: Shared, pid: String, kind: LookKind) -> ApiResult {
    let State(state) = state;
    let loaded = load(&state, &pid).await?;
    let view = blocking(move || looks_project::looks_view(&loaded.session, kind)).await?;
    let revision = view.get("revision").and_then(Value::as_i64);
    Ok(success(view, revision))
}

async fn put_looks(state: Shared, pid: String, kind: LookKind, headers: HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    let State(state) = state;
    let fields = body_object(body)?;
    let key = kind.key();
    if let Some(unknown) = fields.keys().find(|k| *k != key) {
        return Err(ApiError::validation(format!("Unknown field `{unknown}`. Send {key}.")));
    }
    let Some(list) = fields.get(key).and_then(Value::as_array).cloned() else {
        return Err(ApiError::validation(format!("{key} must be a list of {{look, start, seconds, intensity, params}} items.")));
    };
    for (n, entry) in list.iter().enumerate() {
        check_entry(kind, entry).map_err(|e| ApiError::validation(format!("{} {}: {e}", kind.name(), n + 1)))?;
    }
    let expected = if_match(&headers)?;
    let folder = resolve_project(&state.roots, &pid)?;
    let _guard = state.edit_lock.lock().await;
    let view = blocking(move || looks_project::put_looks(&folder, kind, &list, expected, &Stamp::now())).await?.map_err(op_error)?;
    let revision = view.get("revision").and_then(Value::as_i64);
    Ok(success(view, revision))
}

async fn set_look(state: Shared, pid: String, kind: LookKind, headers: HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    let State(state) = state;
    let fields = body_object(body)?;
    let supported = ["id", "look", "start", "seconds", "intensity", "params"];
    if let Some(unknown) = fields.keys().find(|k| !supported.contains(&k.as_str())) {
        return Err(ApiError::validation(format!("Unknown field `{unknown}`. Supported: {}.", supported.join(", "))));
    }
    let named = fields.get("look").is_some_and(|t| !t.is_null() && !t.as_str().is_some_and(|s| s.trim().is_empty() || s.trim().eq_ignore_ascii_case("none")));
    // A new entry is checked whole; a change to an existing one is checked against what it becomes, when it is saved.
    if named && fields.get("id").is_none_or(Value::is_null) {
        check_entry(kind, &Value::Object(fields.clone())).map_err(ApiError::validation)?;
    } else if named {
        let mut probe = fields.clone();
        probe.entry("start").or_insert(json!(0));
        check_entry(kind, &Value::Object(probe)).map_err(ApiError::validation)?;
    }
    let expected = if_match(&headers)?;
    let folder = resolve_project(&state.roots, &pid)?;
    let _guard = state.edit_lock.lock().await;
    let view = blocking(move || looks_project::set_look(&folder, kind, &fields, expected, &Stamp::now())).await?.map_err(op_error)?;
    let revision = view.get("revision").and_then(Value::as_i64);
    Ok(success(view, revision))
}

/// `GET /effects`: every effect with its category, tags and settings (name, type, default, range).
pub async fn effects_catalog() -> ApiResult {
    catalog(LookKind::Effect)
}

/// `GET /filters`: every filter with its category, tags and settings (name, type, default, range).
pub async fn filters_catalog() -> ApiResult {
    catalog(LookKind::Filter)
}

/// `GET /projects/{pid}/effects`: the project's effects in timeline order, each as `{id, look, start, end, seconds, intensity, params}`, with the revision.
pub async fn get_effects(state: Shared, Path(pid): Path<String>) -> ApiResult {
    get_looks(state, pid, LookKind::Effect).await
}

/// `GET /projects/{pid}/filters`: the project's filters in timeline order, each as `{id, look, start, end, seconds, intensity, params}`, with the revision.
pub async fn get_filters(state: Shared, Path(pid): Path<String>) -> ApiResult {
    get_looks(state, pid, LookKind::Filter).await
}

/// `PUT /projects/{pid}/effects`: `{video_effects: [...]}`. Replaces the whole list.
pub async fn put_effects(state: Shared, Path(pid): Path<String>, headers: HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    put_looks(state, pid, LookKind::Effect, headers, body).await
}

/// `PUT /projects/{pid}/filters`: `{video_filters: [...]}`. Replaces the whole list.
pub async fn put_filters(state: Shared, Path(pid): Path<String>, headers: HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    put_looks(state, pid, LookKind::Filter, headers, body).await
}

/// `POST /projects/{pid}/effects/set`: `{id?, look, start, seconds?, intensity?, params?}`. Adds an effect, changes the one with that `id`, or removes it when
/// `look` is null, empty or `none`.
pub async fn set_effect(state: Shared, Path(pid): Path<String>, headers: HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    set_look(state, pid, LookKind::Effect, headers, body).await
}

/// `POST /projects/{pid}/filters/set`: the same for a filter.
pub async fn set_filter(state: Shared, Path(pid): Path<String>, headers: HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    set_look(state, pid, LookKind::Filter, headers, body).await
}
