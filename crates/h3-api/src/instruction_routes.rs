//! The prompt-writing instructions (the system prompts the LLM steps use) and their presets, for agents. They are the Builder's own instruction
//! editor (`h3_builder::instructions`) behind the agent API's paths, like `agent_api/router.py` does.

use std::collections::HashMap;

use axum::extract::{Path, Query};
use axum::Json;
use h3_builder::instructions;
use h3_core::pyval::Obj;
use serde_json::{json, Value};

use crate::app::{blocking, body_object, Shared};
use crate::error::{success, ApiError, ApiResult};
use crate::views;

/// The instruction that is listed when none is named.
const DEFAULT_KEY: &str = "zimage_t2i";

fn text_fields(query: HashMap<String, String>) -> Obj {
    query.into_iter().map(|(key, value)| (key, json!(value))).collect()
}

/// A request that names a project is run against that project's own instructions.
fn with_project(state: &crate::app::AppState, payload: &mut Obj) -> Result<(), ApiError> {
    if let Some(pid) = payload.get("project_id").and_then(Value::as_str).map(str::to_string) {
        let folder = views::resolve_project(&state.roots, &pid)?;
        payload.insert("project_folder".into(), json!(folder.display().to_string()));
    }
    Ok(())
}

fn answer(result: Result<Value, String>) -> ApiResult {
    result.map(|value| success(value, None)).map_err(ApiError::validation)
}

/// `GET /instructions`: the presets saved for an instruction (`key`, default `zimage_t2i`).
pub async fn list(state: Shared, Query(query): Query<HashMap<String, String>>) -> ApiResult {
    let mut payload = text_fields(query);
    payload.entry("key").or_insert_with(|| json!(DEFAULT_KEY));
    let env = state.builder_env();
    answer(blocking(move || instructions::list_presets(&env, &payload)).await?)
}

/// `GET /instructions/{key}`: the text in use, and whether it is the default or an override.
pub async fn get_one(state: Shared, Path(key): Path<String>, Query(query): Query<HashMap<String, String>>) -> ApiResult {
    let mut payload = text_fields(query);
    payload.insert("key".into(), json!(key));
    with_project(&state, &mut payload)?;
    answer(blocking(move || instructions::get_instruction(&payload)).await?)
}

/// `PUT /instructions/{key}`: save an override, for every project or (with `project_id`) for one.
pub async fn save_one(state: Shared, Path(key): Path<String>, body: Option<Json<Value>>) -> ApiResult {
    let mut payload = body_object(body)?;
    payload.insert("key".into(), json!(key));
    with_project(&state, &mut payload)?;
    answer(blocking(move || instructions::save_instruction(&payload)).await?)
}

/// `DELETE /instructions/{key}/override`: go back to the default text.
pub async fn reset_one(state: Shared, Path(key): Path<String>, body: Option<Json<Value>>) -> ApiResult {
    let mut payload = body_object(body)?;
    payload.insert("key".into(), json!(key));
    with_project(&state, &mut payload)?;
    answer(blocking(move || instructions::reset_instruction(&payload)).await?)
}

/// `PUT /instructions/presets/{name}`: keep the text now in use as a named preset.
pub async fn save_preset(state: Shared, Path(name): Path<String>, body: Option<Json<Value>>) -> ApiResult {
    let mut payload = body_object(body)?;
    payload.insert("name".into(), json!(name));
    payload.entry("key").or_insert_with(|| json!(DEFAULT_KEY));
    let env = state.builder_env();
    answer(blocking(move || instructions::save_preset(&env, &payload)).await?)
}

/// `POST /instructions/presets/{name}/load`: use a named preset's text.
pub async fn load_preset(state: Shared, Path(name): Path<String>, body: Option<Json<Value>>) -> ApiResult {
    let mut payload = body_object(body)?;
    payload.insert("name".into(), json!(name));
    payload.entry("key").or_insert_with(|| json!(DEFAULT_KEY));
    let env = state.builder_env();
    answer(blocking(move || instructions::load_preset(&env, &payload)).await?)
}
