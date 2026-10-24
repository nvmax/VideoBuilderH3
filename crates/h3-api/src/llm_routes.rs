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
    let mut updated: LlmConfig = serde_json::from_value(value).map_err(|e| ApiError::validation(e.to_string()))?;
    // The ComfyUI address is not an LLM setting. It is not in the saved file, so it is carried over from the app settings.
    updated.comfy_url = state.cfg.comfy_url.clone();
    if !matches!(updated.runner.as_str(), "lm_studio" | "own_server" | "comfyui_gemma" | "openai" | "anthropic" | "google") {
        return Err(ApiError::validation("runner must be lm_studio, own_server, comfyui_gemma, openai, anthropic or google."));
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

/// The models a runner offers. For LM Studio only the loaded ones, read from its status call. Own server, OpenAI,
/// Anthropic and Google list what their API reports. Nothing is loaded or changed anywhere.
async fn list_models(cfg: &LlmConfig) -> Result<Value, ApiError> {
    let client = reqwest::Client::new();
    let unreachable = |url: &str, e: reqwest::Error| ApiError::from(h3_llm::LlmError::Unavailable(format!("Could not reach {url}: {e}")));
    let refused = |url: &str, status: u16| {
        let why = if matches!(status, 401 | 403) { "The key was refused." } else { "The server did not list its models." };
        ApiError::from(h3_llm::LlmError::Unavailable(format!("{url} answered {status}. {why}")))
    };
    let mut note = "";
    let mut mmproj: Vec<String> = Vec::new();
    let models: Vec<Value> = match cfg.runner.as_str() {
        "comfyui_gemma" => {
            let found = h3_llm::comfy_gemma::files(cfg).await?;
            mmproj = found.mmproj.clone();
            if found.models.is_empty() {
                note = "ComfyUI has no Gemma GGUF model. Put a .gguf file with \"gemma\" in its name in ComfyUI's models/LLM folder.";
            }
            found.models.iter().map(|m| json!({"id": m})).collect()
        }
        "lm_studio" => {
            let loaded = loaded_lm_studio_models(cfg).await?;
            loaded.iter().map(|m| json!({"id": m.id, "loaded": true, "context_length": m.context_length, "vision": m.vision})).collect()
        }
        "own_server" | "openai" => {
            let (url, key) = if cfg.runner == "openai" { ("https://api.openai.com/v1".to_string(), cfg.api_key.clone()) } else { (cfg.own_server_url.trim_end_matches('/').to_string(), cfg.own_server_api_key.clone()) };
            let url = format!("{url}/models");
            let mut request = client.get(&url).timeout(std::time::Duration::from_secs(15));
            if !key.is_empty() {
                request = request.bearer_auth(&key);
            }
            let response = request.send().await.map_err(|e| unreachable(&url, e))?;
            if !response.status().is_success() {
                return Err(refused(&url, response.status().as_u16()));
            }
            let data: Value = response.json().await.unwrap_or(Value::Null);
            let mut ids: Vec<String> = data.get("data").and_then(Value::as_array).map(|a| a.iter().filter_map(|m| m.get("id").and_then(Value::as_str)).map(str::to_string).collect()).unwrap_or_default();
            ids.sort();
            ids.into_iter().map(|id| json!({"id": id})).collect()
        }
        "anthropic" => {
            let url = "https://api.anthropic.com/v1/models?limit=100";
            let response = client.get(url).header("x-api-key", &cfg.api_key).header("anthropic-version", "2023-06-01").timeout(std::time::Duration::from_secs(15)).send().await.map_err(|e| unreachable(url, e))?;
            if !response.status().is_success() {
                return Err(refused(url, response.status().as_u16()));
            }
            let data: Value = response.json().await.unwrap_or(Value::Null);
            data.get("data").and_then(Value::as_array).map(|a| a.iter().filter_map(|m| m.get("id").and_then(Value::as_str)).map(|id| json!({"id": id})).collect()).unwrap_or_default()
        }
        "google" => {
            let url = "https://generativelanguage.googleapis.com/v1beta/models?pageSize=200";
            let response = client.get(url).header("x-goog-api-key", &cfg.api_key).timeout(std::time::Duration::from_secs(15)).send().await.map_err(|e| unreachable(url, e))?;
            if !response.status().is_success() {
                return Err(refused(url, response.status().as_u16()));
            }
            let data: Value = response.json().await.unwrap_or(Value::Null);
            data.get("models")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter(|m| m.get("supportedGenerationMethods").and_then(Value::as_array).is_some_and(|methods| methods.iter().any(|x| x == "generateContent")))
                        .filter_map(|m| m.get("name").and_then(Value::as_str))
                        .map(|name| json!({"id": name.strip_prefix("models/").unwrap_or(name)}))
                        .collect()
                })
                .unwrap_or_default()
        }
        _ => {
            note = "Type the model name.";
            Vec::new()
        }
    };
    Ok(json!({"runner": cfg.runner, "models": models, "mmproj": mmproj, "note": note}))
}

/// `GET /llm/models`: the models of the saved runner, or of `?provider=`.
pub async fn models(State(state): Shared, Query(q): Query<HashMap<String, String>>) -> ApiResult {
    let mut cfg = current(&state);
    if let Some(runner) = q.get("provider").filter(|p| !p.is_empty()) {
        cfg.runner = runner.clone();
    }
    Ok(success(list_models(&cfg).await?, None))
}

/// `POST /llm/models`: the models for settings that are not saved yet. The body holds the fields typed so far
/// (`runner`, `lm_studio_base_url`, `own_server_url`, `own_server_api_key`, `api_key`, ...). A key left out or empty is
/// taken from the saved settings, so a saved key is never sent to the screen and back.
pub async fn models_for(State(state): Shared, body: Option<Json<Value>>) -> ApiResult {
    let sent = body_object(body)?;
    let mut cfg = current(&state);
    for (key, value) in &sent {
        let text = value.as_str().map(|s| s.trim().to_string());
        match (key.as_str(), text) {
            ("runner", Some(t)) if !t.is_empty() => cfg.runner = t,
            ("lm_studio_base_url", Some(t)) if !t.is_empty() => cfg.lm_studio_base_url = t,
            ("lm_studio_api_key", Some(t)) if !t.is_empty() => cfg.lm_studio_api_key = t,
            ("own_server_url", Some(t)) if !t.is_empty() => cfg.own_server_url = t,
            ("own_server_api_key", Some(t)) if !t.is_empty() => cfg.own_server_api_key = t,
            ("api_key", Some(t)) if !t.is_empty() => cfg.api_key = t,
            ("comfy_url", Some(t)) if !t.is_empty() => cfg.comfy_url = t,
            ("runner" | "comfy_url" | "lm_studio_base_url" | "lm_studio_api_key" | "own_server_url" | "own_server_api_key" | "api_key", _) => {}
            (other, _) => return Err(ApiError::validation(format!("Unknown field '{other}'. Send runner, lm_studio_base_url, lm_studio_api_key, own_server_url, own_server_api_key or api_key."))),
        }
    }
    Ok(success(list_models(&cfg).await?, None))
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

/// `POST /projects/{pid}/scenes/{sid}/prompts/image`: write the scene's image prompt and save it as `t2i_prompt`, as a job.
pub async fn scene_image_prompt(State(state): Shared, axum::extract::Path((pid, sid)): axum::extract::Path<(String, String)>, body: Option<Json<Value>>) -> ApiResult {
    let mut params = body_object(body)?;
    let loaded = crate::app::load(&state, &pid).await?;
    crate::views::find_scene(&loaded, &sid)?;
    params.insert("scene_id".into(), json!(sid));
    let job = state.jobs.submit(crate::llm_jobs::SCENE_IMAGE_PROMPT, Some(&pid), Value::Object(params), false)?;
    Ok(crate::app::job_response(&job))
}

/// `POST /projects/{pid}/scenes/{sid}/prompts/video`: write the scene's MiniMax H3 prompt again, as a job.
pub async fn scene_video_prompt(State(state): Shared, axum::extract::Path((pid, sid)): axum::extract::Path<(String, String)>, body: Option<Json<Value>>) -> ApiResult {
    let mut params = body_object(body)?;
    let loaded = crate::app::load(&state, &pid).await?;
    crate::views::find_scene(&loaded, &sid)?;
    params.insert("scene_ids".into(), json!([sid]));
    let job = state.jobs.submit(crate::llm_jobs::MINIMAX_PROMPTS, Some(&pid), Value::Object(params), false)?;
    Ok(crate::app::job_response(&job))
}

/// `POST /projects/{pid}/prompts/batch`: write prompts for many scenes. `kind: image` writes `t2i_prompt`, `kind: video`
/// (the default) writes the MiniMax H3 prompt.
pub async fn batch_prompts(State(state): Shared, axum::extract::Path(pid): axum::extract::Path<String>, body: Option<Json<Value>>) -> ApiResult {
    let mut params = body_object(body)?;
    crate::app::load(&state, &pid).await?;
    let kind = h3_core::pyval::text_or_empty(params.get("kind")).trim().to_lowercase();
    if kind == "image" {
        let job = state.jobs.submit(crate::llm_jobs::BATCH_PROMPTS, Some(&pid), Value::Object(params), false)?;
        return Ok(crate::app::job_response(&job));
    }
    if !kind.is_empty() && kind != "video" {
        return Err(ApiError::validation("kind must be 'image' or 'video'."));
    }
    // Video prompts are the MiniMax H3 prompts. Without `resume_missing` every scene is written again.
    let resume = h3_core::pyval::text_or_empty(params.get("run_mode")).trim().to_lowercase() == "resume_missing";
    params.insert("replace_existing".into(), json!(!resume));
    let job = state.jobs.submit(crate::llm_jobs::MINIMAX_PROMPTS, Some(&pid), Value::Object(params), false)?;
    Ok(crate::app::job_response(&job))
}

/// `POST /projects/{pid}/references/{kind}/{rid}/prompt`: write the prompt for a character reference sheet or a location
/// image, as a job. The prompt comes back in the job result.
pub async fn reference_image_prompt(State(state): Shared, axum::extract::Path((pid, kind, rid)): axum::extract::Path<(String, String, String)>, body: Option<Json<Value>>) -> ApiResult {
    let mut params = body_object(body)?;
    if kind != "subjects" && kind != "locations" {
        return Err(ApiError::validation("kind must be 'subjects' or 'locations'."));
    }
    let loaded = crate::app::load(&state, &pid).await?;
    if h3_project::images::reference_card(&loaded.session, &kind, &rid).is_none() {
        return Err(ApiError::validation(format!("Reference '{rid}' was not found in the Reference Builder.")));
    }
    params.insert("kind".into(), json!(kind));
    params.insert("ref_id".into(), json!(rid));
    let job = state.jobs.submit(crate::llm_jobs::REFERENCE_IMAGE_PROMPT, Some(&pid), Value::Object(params), false)?;
    Ok(crate::app::job_response(&job))
}
