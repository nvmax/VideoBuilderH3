//! LLM settings and checks. The app never loads, unloads or switches a model in LM Studio.

use std::collections::HashMap;

use axum::extract::{Query, State};
use axum::Json;
use h3_llm::{chat, describe_active, loaded_lm_studio_models, ChatRequest, LlmConfig};
use serde_json::{json, Value};

use crate::app::{blocking, body_object, Shared};
use crate::error::{success, ApiError, ApiResult};

pub fn current(state: &crate::app::AppState) -> LlmConfig {
    state.llm.read().unwrap_or_else(|e| e.into_inner()).clone()
}

pub async fn get_settings(State(state): Shared) -> ApiResult {
    Ok(success(current(&state).redacted(), None))
}

/// Change the sent settings and save them in the config file. A key field that is sent replaces the saved key.
pub async fn put_settings(State(state): Shared, body: Option<Json<Value>>) -> ApiResult {
    let patch = body_object(body)?;
    let mut value = serde_json::to_value(current(&state)).unwrap_or_default();
    let map = value.as_object_mut().expect("config is an object");
    for (key, new) in &patch {
        if key.ends_with("_set") || !map.contains_key(key) {
            return Err(ApiError::validation(format!("Unknown LLM setting '{key}'.")));
        }
        if std::mem::discriminant(&map[key]) != std::mem::discriminant(new) {
            return Err(ApiError::validation(format!("LLM setting '{key}' has the wrong type.")));
        }
        map.insert(key.clone(), new.clone());
    }
    let updated: LlmConfig = serde_json::from_value(value).map_err(|e| ApiError::validation(e.to_string()))?;
    if !matches!(updated.runner.as_str(), "lm_studio" | "own_server" | "openai" | "anthropic" | "google") {
        return Err(ApiError::validation("runner must be lm_studio, own_server, openai, anthropic or google."));
    }
    let path = state.config_path.clone();
    let mut file = state.cfg.clone();
    file.llm = updated.clone();
    blocking(move || file.save(&path)).await?.map_err(|e| ApiError::internal(format!("The settings could not be saved: {e}")))?;
    *state.llm.write().unwrap_or_else(|e| e.into_inner()) = updated.clone();
    Ok(success(updated.redacted(), None))
}

/// Which LLM a request would use right now. For LM Studio this is the model that is loaded.
pub async fn active(State(state): Shared) -> ApiResult {
    let info = describe_active(&current(&state)).await?;
    Ok(success(
        json!({
            "runner": info.runner,
            "model": info.model,
            "context_length": info.context_length,
            "base_url": info.base_url,
            "saved_model_setting": info.saved_model_setting,
            "uses_saved_setting": info.saved_model_setting == info.model,
        }),
        None,
    ))
}

/// The models the runner offers. For LM Studio only the loaded ones, read from its status call.
pub async fn models(State(state): Shared, Query(q): Query<HashMap<String, String>>) -> ApiResult {
    let mut cfg = current(&state);
    if let Some(runner) = q.get("provider").filter(|p| !p.is_empty()) {
        cfg.runner = runner.clone();
    }
    match cfg.runner.as_str() {
        "lm_studio" => {
            let loaded = loaded_lm_studio_models(&cfg).await?;
            Ok(success(json!({"runner": "lm_studio", "models": loaded.iter().map(|m| json!({"id": m.id, "loaded": true, "context_length": m.context_length, "vision": m.vision})).collect::<Vec<_>>()}), None))
        }
        "own_server" => {
            let url = format!("{}/models", cfg.own_server_url.trim_end_matches('/'));
            let mut request = reqwest::Client::new().get(&url).timeout(std::time::Duration::from_secs(10));
            if !cfg.own_server_api_key.is_empty() {
                request = request.bearer_auth(&cfg.own_server_api_key);
            }
            let data: Value = request.send().await.map_err(|e| ApiError::from(h3_llm::LlmError::Unavailable(format!("Could not reach {url}: {e}"))))?.json().await.unwrap_or(Value::Null);
            let ids: Vec<Value> = data.get("data").and_then(Value::as_array).map(|a| a.iter().filter_map(|m| m.get("id")).map(|id| json!({"id": id})).collect()).unwrap_or_default();
            Ok(success(json!({"runner": "own_server", "models": ids}), None))
        }
        other => Ok(success(json!({"runner": other, "models": [], "note": "Type the model name in the LLM settings."}), None)),
    }
}

/// Write MiniMax prompts for scenes that have none, with the project's LLM, as a job.
pub async fn write_prompts(State(state): Shared, axum::extract::Path(pid): axum::extract::Path<String>, body: Option<Json<Value>>) -> ApiResult {
    let params = body_object(body)?;
    crate::app::load(&state, &pid).await?;
    // The LLM runs in its own process, so this job does not wait behind a render.
    let job = state.jobs.submit(crate::llm_jobs::MINIMAX_PROMPTS, Some(&pid), Value::Object(params), false)?;
    Ok(crate::app::job_response(&job))
}

/// Describe a character or location picture with the LLM and save the description on the card, as a job.
pub async fn describe_reference(State(state): Shared, axum::extract::Path((pid, kind, rid)): axum::extract::Path<(String, String, String)>) -> ApiResult {
    if kind != "subjects" && kind != "locations" {
        return Err(ApiError::validation("kind must be 'subjects' or 'locations'."));
    }
    let loaded = crate::app::load(&state, &pid).await?;
    if h3_project::images::reference_card(&loaded.session, &kind, &rid).is_none() {
        return Err(ApiError::validation(format!("Reference '{rid}' was not found in the Reference Builder.")));
    }
    let params = json!({"kind": kind, "ref_id": rid});
    let job = state.jobs.submit(crate::llm_jobs::REFERENCE_DESCRIBE, Some(&pid), params, false)?;
    Ok(crate::app::job_response(&job))
}

/// Ask the LLM for filming locations from the lyrics and add them to the Reference Builder, as a job.
pub async fn extract_locations(State(state): Shared, axum::extract::Path(pid): axum::extract::Path<String>, body: Option<Json<Value>>) -> ApiResult {
    let params = body_object(body)?;
    crate::app::load(&state, &pid).await?;
    let job = state.jobs.submit(crate::llm_jobs::EXTRACT_LOCATIONS, Some(&pid), Value::Object(params), false)?;
    Ok(crate::app::job_response(&job))
}

/// Send one short request and report whether the model answers.
pub async fn test(State(state): Shared) -> ApiResult {
    let cfg = current(&state);
    let reply = chat(&cfg, &ChatRequest::text("Reply with the single word OK.", 0.0, 1.0, 16)).await?;
    Ok(success(json!({"ok": true, "model": reply.model, "runner": reply.runner, "reply": h3_llm::clean::clean_plain_text(&reply.text)}), None))
}
