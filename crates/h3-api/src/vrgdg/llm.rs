//! LLM Runner routes of the Builder: the runner choices and their checks, the instruction editor, and clearing memory.

use std::future::Future;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post as axum_post, MethodRouter};
use axum::Router;
use h3_builder::instructions;
use h3_builder::prompt_writers::{concepts, enhance, gemma4, image_prompts, motion_notes, references, RunnerLlm};
use h3_core::pyval::{text_or_empty, to_int, truthy, Obj};
use h3_llm::runner::runner_config;
use h3_llm::{own_server, providers, ChatRequest, LlmConfig};
use serde_json::{json, Value};

use super::{post, Fail, Out};
use crate::app::AppState;

fn fields(result: Result<Value, String>) -> Result<Out, Fail> {
    result.map(Out::from).map_err(Fail::from)
}

/// A POST route that does async work with the app's settings and the JSON body.
fn work<F, Fut>(f: F) -> MethodRouter<Arc<AppState>>
where
    F: Fn(Arc<AppState>, Obj) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Result<Value, String>> + Send + 'static,
{
    axum_post(move |State(state): State<Arc<AppState>>, body: Bytes| {
        let f = f.clone();
        async move {
            let reply = match super::body_object(&body) {
                Ok(payload) => f(state, payload).await.map(Out::from).map_err(Fail::from),
                Err(fail) => Err(fail),
            };
            super::into_response(reply)
        }
    })
}

/// A POST route of an LLM writer: it runs on the runner the request's own settings select and fails with 500 like the pack's.
macro_rules! writer {
    ($function:path) => {
        axum_post(|State(state): State<Arc<AppState>>, body: Bytes| async move {
            let payload = match super::body_object(&body) {
                Ok(payload) => payload,
                Err(fail) => return super::into_response(Err(fail)),
            };
            let base = base_config(&state);
            let llm = RunnerLlm { payload: &payload, base: &base };
            let reply = $function(&llm, &payload).await.map(Out::from).map_err(|message| Fail::new(StatusCode::INTERNAL_SERVER_ERROR, message));
            super::into_response(reply)
        })
    };
}

/// `POST /vrgdg/gemma4/generate`: a writer that, unlike the others, answers 400 when the body is not a JSON object.
fn gemma4_generate() -> MethodRouter<Arc<AppState>> {
    axum_post(|State(state): State<Arc<AppState>>, body: Bytes| async move {
        let payload = match serde_json::from_slice::<Value>(&body) {
            Ok(Value::Object(map)) => map,
            Ok(_) => return super::into_response(Err(Fail::new(StatusCode::BAD_REQUEST, "JSON body must be an object."))),
            Err(_) => return super::into_response(Err(Fail::new(StatusCode::BAD_REQUEST, "Invalid JSON body."))),
        };
        let base = base_config(&state);
        let llm = RunnerLlm { payload: &payload, base: &base };
        let reply = gemma4::generate(&llm, &payload).await.map(Out::from).map_err(|message| Fail::new(StatusCode::INTERNAL_SERVER_ERROR, message));
        super::into_response(reply)
    })
}

pub fn routes(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    let base = "/vrgdg/music_builder";
    router
        .route(&format!("{base}/llm_api_choices"), get(|| async { super::into_response(Ok(Out::from(providers::choices()))) }))
        .route(&format!("{base}/test_llm_api"), work(|_, p| async move { test_llm_api(&p).await }))
        .route(&format!("{base}/test_own_server"), work(|state, p| async move { test_own_server(&state, &p).await }))
        .route(&format!("{base}/own_server_models"), work(|state, p| async move { own_server_models(&state, &p).await }))
        .route(&format!("{base}/lm_studio_models"), work(|state, p| async move { lm_studio_models(&state, &p).await }))
        .route(&format!("{base}/own_server_reference_image"), work(|state, p| async move { own_server_image(&state, &p).await }))
        .route(&format!("{base}/generate_t2i"), writer!(image_prompts::generate_t2i))
        .route(&format!("{base}/edit_image_prompt"), writer!(image_prompts::edit_image_prompt))
        .route(&format!("{base}/generate_flux_klein_prompt"), writer!(image_prompts::generate_flux_klein_prompt))
        .route(&format!("{base}/generate_nb_image_prompt"), writer!(image_prompts::generate_nb_image_prompt))
        .route(&format!("{base}/analyze_story_references"), writer!(image_prompts::analyze_story_references))
        .route(&format!("{base}/generate_concept_prompts"), writer!(concepts::generate_concept_prompts))
        .route(&format!("{base}/generate_motion_notes"), writer!(motion_notes::generate_motion_notes))
        .route(&format!("{base}/enhance_minimax_h3_prompt"), writer!(enhance::enhance_minimax_h3_prompt))
        .route(&format!("{base}/describe_reference_image"), writer!(references::describe_reference_image))
        .route(&format!("{base}/flux_reference_location_map"), writer!(references::flux_reference_location_map))
        .route(&format!("{base}/flux_reference_extract_locations"), writer!(references::flux_reference_extract_locations))
        .route(&format!("{base}/flux_reference_extract_subjects"), writer!(references::flux_reference_extract_subjects))
        .route(&format!("{base}/flux_reference_zimage_prompt"), writer!(references::flux_reference_zimage_prompt))
        .route(&format!("{base}/lm_scout_locations"), writer!(references::lm_scout_locations))
        .route(&format!("{base}/wizard_locations_from_lyrics"), writer!(references::wizard_locations_from_lyrics))
        .route("/vrgdg/gemma4/generate", gemma4_generate())
        .route(&format!("{base}/clear_memory_direct"), work(|state, _| async move { clear_memory_direct(&state).await }))
        .route(&format!("{base}/get_instruction"), post(|_, p| fields(instructions::get_instruction(&p))))
        .route(&format!("{base}/save_instruction"), post(|_, p| fields(instructions::save_instruction(&p))))
        .route(&format!("{base}/reset_instruction"), post(|_, p| fields(instructions::reset_instruction(&p))))
        .route(&format!("{base}/list_instruction_presets"), post(|env, p| fields(instructions::list_presets(&env, &p))))
        .route(&format!("{base}/save_instruction_preset"), post(|env, p| fields(instructions::save_preset(&env, &p))))
        .route(&format!("{base}/load_instruction_preset"), post(|env, p| fields(instructions::load_preset(&env, &p))))
}

/// The runner settings of the app as the base every request starts from. Only the ComfyUI address matters to it.
pub fn base_config(state: &AppState) -> LlmConfig {
    let mut cfg = crate::llm_routes::current(state);
    cfg.comfy_url = state.cfg.comfy_url.clone();
    cfg
}

/// The settings of one runner out of a request, whatever `text_runner` says.
fn forced_config(state: &AppState, payload: &Obj, runner: &str, requested: i64) -> LlmConfig {
    let mut forced = payload.clone();
    forced.insert("text_runner".into(), json!(runner));
    runner_config(&forced, &base_config(state), requested)
}

fn first_text(payload: &Obj, keys: &[&str]) -> String {
    keys.iter().map(|k| text_or_empty(payload.get(*k)).trim().to_string()).find(|t| !t.is_empty()).unwrap_or_default()
}

/// `POST test_llm_api`: one short question to the chosen hosted API.
async fn test_llm_api(payload: &Obj) -> Result<Value, String> {
    let provider = providers::canonical(&{
        let found = first_text(payload, &["provider", "llm_api_provider"]);
        if found.is_empty() { "openai".to_string() } else { found }
    });
    let model = first_text(payload, &["model", "llm_api_model"]);
    let key = first_text(payload, &["api_key", "llm_api_key"]);
    let custom = first_text(payload, &["custom_model"]);
    if key.is_empty() {
        return Err("API key is missing.".into());
    }
    if provider.is_empty() {
        return Err("Provider is missing.".into());
    }
    let prompt = Some(first_text(payload, &["prompt"])).filter(|p| !p.is_empty()).unwrap_or_else(|| "Reply with OK only.".into());
    let reply = providers::generate(&provider, &model, &custom, &key, &prompt, &[]).await.map_err(|status| status.trim().to_string())?;
    if reply.text.trim().is_empty() {
        return Err("LLM API returned empty text.".into());
    }
    Ok(json!({"text": reply.text.chars().take(1000).collect::<String>(), "used_provider": reply.provider, "used_model": reply.model, "status": "ok"}))
}

/// `POST test_own_server`.
async fn test_own_server(state: &AppState, payload: &Obj) -> Result<Value, String> {
    let cfg = forced_config(state, payload, "own_server", 64);
    let prompt = Some(first_text(payload, &["prompt"])).filter(|p| !p.is_empty()).unwrap_or_else(|| "Reply with OK only.".into());
    let reply = own_server::chat(&cfg, &ChatRequest::text(&prompt, 0.1, 0.9, 64)).await.map_err(|e| e.to_string())?;
    let base_url = own_server::v1_root(&cfg.own_server_url).map_err(|e| e.to_string())?;
    Ok(json!({"text": reply.text, "used_provider": "own_server", "used_model": reply.model, "base_url": base_url}))
}

/// `POST own_server_models`.
async fn own_server_models(state: &AppState, payload: &Obj) -> Result<Value, String> {
    let cfg = forced_config(state, payload, "own_server", 1200);
    let (models, base_url) = own_server::list_models(&cfg).await.map_err(|e| e.to_string())?;
    Ok(json!({"models": models, "base_url": base_url}))
}

/// `POST lm_studio_models`: the models LM Studio lists. Nothing is loaded or switched.
async fn lm_studio_models(state: &AppState, payload: &Obj) -> Result<Value, String> {
    let cfg = forced_config(state, payload, "lm_studio", 1200);
    let (models, raw) = h3_llm::list_lm_studio_models(&cfg).await.map_err(|e| e.to_string())?;
    Ok(json!({"models": models, "raw": raw}))
}

/// `POST own_server_reference_image`: one picture from the custom server's image endpoint.
async fn own_server_image(state: &AppState, payload: &Obj) -> Result<Value, String> {
    let cfg = forced_config(state, payload, "own_server", 1200);
    let number = |key: &str| payload.get(key).filter(|v| !v.is_null() && v.as_str() != Some("")).and_then(h3_core::pyval::to_float);
    let seed = payload.get("seed").filter(|v| !v.is_null() && v.as_str() != Some("")).and_then(to_int);
    let steps = payload.get("steps").filter(|v| !v.is_null() && v.as_str() != Some("") && truthy(v)).and_then(to_int);
    let prompt = text_or_empty(payload.get("prompt"));
    let model = text_or_empty(payload.get("image_model"));
    let (image_data, size) = own_server::generate_image(&cfg, &prompt, number("width"), number("height"), &model, seed, steps).await.map_err(|e| e.to_string())?;
    Ok(json!({"image_data": image_data, "size": size}))
}

/// `POST clear_memory_direct`: ask ComfyUI to unload its models and free memory.
async fn clear_memory_direct(state: &AppState) -> Result<Value, String> {
    let base = state.cfg.comfy_base().to_string();
    let response = reqwest::Client::new().post(format!("{base}/free")).json(&json!({"unload_models": true, "free_memory": true})).send().await.map_err(|e| format!("ComfyUI is not reachable at {base}: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("ComfyUI refused the cleanup request ({}).", response.status()));
    }
    eprintln!("[VRGDG-RS Clear Memory] Asked ComfyUI to unload its models and free memory.");
    Ok(json!({
        "message": "Memory cleanup finished.\nComfy cleanup calls: free (unload models, free memory)\nGGUF models unloaded: 0\nHF pipelines unloaded: 0",
        "comfy_loaded_before": Value::Null,
        "comfy_loaded_after": Value::Null,
        "comfy_cleanup_calls": ["free"],
        "comfy_cleanup_errors": [],
        "torch_cuda_cache_cleared": false,
        "gguf_models_unloaded": 0,
        "hf_pipelines_unloaded": 0,
        "cuda_cache_cleared": false,
    }))
}
