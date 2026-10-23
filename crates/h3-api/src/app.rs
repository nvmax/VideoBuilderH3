//! The router and its handlers.

use std::collections::HashMap;
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use h3_comfy::models::model_choices;
use h3_comfy::ComfyClient;
use h3_core::pyval::Obj;
use h3_jobs::JobManager;
use h3_media::Tools;
use h3_pipeline::{connect, prepare_scene_render, RenderParams};
use h3_project::{latents, load_session, persist_session, AppConfig, LoadedSession, Stamp};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use tower_http::cors::{Any, CorsLayer};

use crate::error::{respond, success, ApiError, ApiResult};
use crate::job_handlers::{self, VIDEOS_RENDER_BATCH, VIDEO_RENDER, VIDEO_STITCH};
use crate::views::{self, SceneFilter};

pub struct AppState {
    pub cfg: AppConfig,
    pub roots: Vec<PathBuf>,
    pub jobs: JobManager,
}

pub type Shared = State<Arc<AppState>>;

/// Build the `/api/v1` router and register the job handlers.
pub fn router(state: Arc<AppState>) -> Router {
    job_handlers::register(&state);
    let v1 = Router::new()
        .route("/health", get(health))
        .route("/meta", get(meta))
        .route("/models", get(models))
        .route("/queue", get(queue))
        .route("/events", get(events))
        .route("/jobs", get(list_jobs))
        .route("/jobs/{id}", get(get_job))
        .route("/jobs/{id}/log", get(job_log))
        .route("/jobs/{id}/cancel", post(cancel_job))
        .route("/jobs/{id}/retry", post(retry_job))
        .route("/projects", get(list_projects))
        .route("/projects/{pid}", get(get_project))
        .route("/projects/{pid}/summary", get(project_summary))
        .route("/projects/{pid}/settings", get(get_settings).patch(patch_settings))
        .route("/projects/{pid}/scenes", get(list_scenes))
        .route("/projects/{pid}/scenes/{sid}", get(get_scene))
        .route("/projects/{pid}/scenes/{sid}/video/render", post(render_scene))
        .route("/projects/{pid}/video/render", post(render_batch))
        .route("/projects/{pid}/video/graph", post(video_graph))
        .route("/projects/{pid}/stitch", post(stitch))
        .route("/projects/{pid}/latents", get(latent_list))
        .route("/projects/{pid}/latents/dirty", get(latent_dirty))
        .route("/projects/{pid}/scenes/{sid}/latent", get(latent_one).delete(latent_delete));
    let cors = CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any);
    Router::new().nest("/api/v1", v1).layer(cors).with_state(state)
}

// -- helpers ----------------------------------------------------------------------------------------------------------------

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(work).await.map_err(|e| ApiError::internal(e.to_string()))
}

async fn load(state: &AppState, pid: &str) -> Result<LoadedSession, ApiError> {
    let folder = views::resolve_project(&state.roots, pid)?;
    blocking(move || load_session(&folder)).await?.map_err(|e| ApiError::internal(e.to_string()))
}

fn if_match(headers: &HeaderMap) -> Result<Option<i64>, ApiError> {
    match headers.get("if-match").and_then(|v| v.to_str().ok()) {
        None => Ok(None),
        Some(text) => text.trim().trim_matches('"').parse().map(Some).map_err(|_| ApiError::validation("If-Match must be the project revision number.")),
    }
}

fn check_revision(headers: &HeaderMap, current: i64) -> Result<(), ApiError> {
    match if_match(headers)? {
        Some(incoming) if incoming != current => Err(ApiError::revision_conflict(current, incoming)),
        _ => Ok(()),
    }
}

fn body_object(body: Option<Json<Value>>) -> Result<Obj, ApiError> {
    match body.map(|b| b.0) {
        None | Some(Value::Null) => Ok(Obj::new()),
        Some(Value::Object(map)) => Ok(map),
        Some(_) => Err(ApiError::validation("The request body must be a JSON object.")),
    }
}

fn job_response(job: &h3_jobs::Job) -> Response {
    respond(StatusCode::ACCEPTED, json!({"job_id": job.id, "status": job.status, "job": job.to_api()}), None)
}

// -- service ----------------------------------------------------------------------------------------------------------------

async fn health(State(state): Shared) -> ApiResult {
    let client = ComfyClient::new(state.cfg.comfy_base());
    let stats = client.system_stats().await.ok();
    let queue_depth = match &stats {
        Some(_) => {
            let q = client.queue_state().await;
            ["queue_running", "queue_pending"].iter().map(|k| q.get(*k).and_then(Value::as_array).map_or(0, Vec::len)).sum::<usize>()
        }
        None => 0,
    };
    let ffmpeg = Tools::find(&state.cfg.ffmpeg_path, &state.cfg.ffprobe_path).is_ok();
    Ok(success(
        json!({
            "status": if stats.is_some() { "healthy" } else { "degraded" },
            "comfy_up": stats.is_some(),
            "queue_depth": queue_depth,
            "ffmpeg_available": ffmpeg,
            "gpu_available": stats.as_ref().is_some_and(|s| !s.devices.is_empty()),
        }),
        None,
    ))
}

async fn meta() -> ApiResult {
    Ok(success(
        json!({
            "api_version": "v1",
            "app": "vrgdg-h3",
            "app_version": env!("CARGO_PKG_VERSION"),
            "capabilities": ["projects", "scenes", "settings", "summary", "videos", "latents", "jobs", "events"],
        }),
        None,
    ))
}

async fn models(State(state): Shared) -> ApiResult {
    let client = ComfyClient::new(state.cfg.comfy_base());
    let info = client.object_info().await?;
    let mut groups = serde_json::Map::new();
    for category in ["diffusion_models", "text_encoders", "vae", "loras", "latent_upscale_models", "upscale_models"] {
        groups.insert(category.to_string(), json!(model_choices(&info, category)));
    }
    Ok(success(Value::Object(groups), None))
}

async fn queue(State(state): Shared) -> ApiResult {
    let mut summary = state.jobs.queue_summary();
    let q = ComfyClient::new(state.cfg.comfy_base()).queue_state().await;
    summary["comfy_queue_depth"] = json!(["queue_running", "queue_pending"].iter().map(|k| q.get(*k).and_then(Value::as_array).map_or(0, Vec::len)).sum::<usize>());
    Ok(success(summary, None))
}

#[derive(Deserialize)]
struct EventsQuery {
    project_id: Option<String>,
}

async fn events(State(state): Shared, Query(query): Query<EventsQuery>) -> Sse<impl tokio_stream::Stream<Item = Result<SseEvent, Infallible>>> {
    let wanted = query.project_id;
    let stream = BroadcastStream::new(state.jobs.subscribe()).filter_map(move |item| match item {
        Ok(event) if wanted.is_none() || event.project_id.is_none() || event.project_id == wanted => {
            Some(Ok(SseEvent::default().event(event.name).data(event.data.to_string())))
        }
        _ => None,
    });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)).text("ping"))
}

// -- jobs -------------------------------------------------------------------------------------------------------------------

async fn list_jobs(State(state): Shared, Query(q): Query<HashMap<String, String>>) -> ApiResult {
    let jobs = state.jobs.list(q.get("project_id").map(String::as_str), q.get("status").map(String::as_str), q.get("type").map(String::as_str))?;
    Ok(success(Value::Array(jobs.iter().map(h3_jobs::Job::to_api).collect()), None))
}

async fn get_job(State(state): Shared, Path(id): Path<String>) -> ApiResult {
    Ok(success(state.jobs.get(&id)?.to_api(), None))
}

async fn job_log(State(state): Shared, Path(id): Path<String>, Query(q): Query<HashMap<String, String>>) -> ApiResult {
    let since = q.get("since").and_then(|s| s.parse().ok()).unwrap_or(0);
    let logs = state.jobs.logs(&id, since)?;
    Ok(success(json!({"job_id": id, "logs": logs}), None))
}

async fn cancel_job(State(state): Shared, Path(id): Path<String>) -> ApiResult {
    Ok(success(state.jobs.cancel(&id)?.to_api(), None))
}

async fn retry_job(State(state): Shared, Path(id): Path<String>, body: Option<Json<Value>>) -> ApiResult {
    let resume = body_object(body)?.get("resume").and_then(Value::as_bool).unwrap_or(false);
    let job = state.jobs.retry(&id, resume)?;
    Ok(respond(StatusCode::ACCEPTED, job.to_api(), None))
}

// -- projects ---------------------------------------------------------------------------------------------------------------

async fn list_projects(State(state): Shared) -> ApiResult {
    let roots = state.roots.clone();
    Ok(success(Value::Array(blocking(move || views::list_projects(&roots)).await?), None))
}

async fn get_project(State(state): Shared, Path(pid): Path<String>) -> Result<Response, ApiError> {
    let loaded = load(&state, &pid).await?;
    let mut response = success(views::project_detail(&loaded), Some(loaded.revision));
    if let Ok(tag) = HeaderValue::from_str(&format!("\"{}\"", loaded.revision)) {
        response.headers_mut().insert(header::ETAG, tag);
    }
    Ok(response)
}

async fn project_summary(State(state): Shared, Path(pid): Path<String>) -> ApiResult {
    let loaded = load(&state, &pid).await?;
    Ok(success(views::project_summary(&loaded), Some(loaded.revision)))
}

async fn get_settings(State(state): Shared, Path(pid): Path<String>) -> ApiResult {
    let loaded = load(&state, &pid).await?;
    Ok(success(views::settings_view(&loaded.session), Some(loaded.revision)))
}

async fn patch_settings(State(state): Shared, Path(pid): Path<String>, headers: HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    let patch = body_object(body)?;
    if patch.is_empty() {
        return Err(ApiError::validation("Send the settings to change, for example {\"minimax_h3\": {\"render_pass\": \"two_pass\"}}."));
    }
    let loaded = load(&state, &pid).await?;
    check_revision(&headers, loaded.revision)?;
    let mut session = loaded.session.clone();
    views::apply_settings_patch(&mut session, &patch)?;
    let folder = loaded.project_folder.clone();
    let saved = blocking(move || persist_session(&folder, session, Stamp::now())).await?.map_err(|e| ApiError::new(StatusCode::CONFLICT, "REVISION_CONFLICT", e.to_string()).retryable())?;
    Ok(success(json!({"settings": views::settings_view(&saved.session), "revision": saved.revision}), Some(saved.revision)))
}

// -- scenes -----------------------------------------------------------------------------------------------------------------

fn flag(q: &HashMap<String, String>, key: &str) -> Option<bool> {
    q.get(key).map(|v| matches!(v.trim().to_lowercase().as_str(), "1" | "true" | "yes"))
}

async fn list_scenes(State(state): Shared, Path(pid): Path<String>, Query(q): Query<HashMap<String, String>>) -> ApiResult {
    let loaded = load(&state, &pid).await?;
    let filter = SceneFilter { has_image: flag(&q, "has_image"), has_video: flag(&q, "has_video"), has_prompt: flag(&q, "has_prompt"), status: q.get("status").cloned() };
    Ok(success(Value::Array(views::scenes_view(&loaded, &filter, false)), Some(loaded.revision)))
}

async fn get_scene(State(state): Shared, Path((pid, sid)): Path<(String, String)>) -> ApiResult {
    let loaded = load(&state, &pid).await?;
    let index = views::find_scene(&loaded, &sid)?;
    let segments = views::segments(&loaded.session);
    Ok(success(views::scene_view(&loaded.project_folder, &segments[index], index, true), Some(loaded.revision)))
}

// -- video ------------------------------------------------------------------------------------------------------------------

async fn render_scene(State(state): Shared, Path((pid, sid)): Path<(String, String)>, body: Option<Json<Value>>) -> ApiResult {
    let mut params = body_object(body)?;
    let loaded = load(&state, &pid).await?;
    views::find_scene(&loaded, &sid)?;
    params.insert("scene_id".into(), json!(sid));
    let job = state.jobs.submit(VIDEO_RENDER, Some(&pid), Value::Object(params), true)?;
    Ok(job_response(&job))
}

async fn render_batch(State(state): Shared, Path(pid): Path<String>, body: Option<Json<Value>>) -> ApiResult {
    let params = body_object(body)?;
    load(&state, &pid).await?;
    let job = state.jobs.submit(VIDEOS_RENDER_BATCH, Some(&pid), Value::Object(params), true)?;
    Ok(job_response(&job))
}

async fn stitch(State(state): Shared, Path(pid): Path<String>, body: Option<Json<Value>>) -> ApiResult {
    const SUPPORTED: [&str; 5] = ["scene_ids", "output_prefix", "audio", "audio_path", "overlays"];
    let payload = body_object(body)?;
    let unknown: Vec<&String> = payload.keys().filter(|k| !SUPPORTED.contains(&k.as_str())).collect();
    if !unknown.is_empty() {
        return Err(ApiError::validation(format!(
            "Unknown stitch field(s): {}. Supported: {}.",
            unknown.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "),
            SUPPORTED.join(", ")
        ))
        .details(json!({"unknown": unknown, "supported": SUPPORTED})));
    }
    let params: Obj = payload.into_iter().filter(|(_, v)| !v.is_null()).collect();
    // Check the scenes, audio and files now so a bad request is a 400 instead of a failed job.
    let loaded = load(&state, &pid).await?;
    let check = params.clone();
    blocking(move || h3_pipeline::stitch_plan::build_stitch_request(&loaded.project_folder, &loaded.session, &check, true).map(|_| ()))
        .await?
        .map_err(ApiError::validation)?;
    let job = state.jobs.submit(VIDEO_STITCH, Some(&pid), Value::Object(params), false)?;
    Ok(job_response(&job))
}

async fn video_graph(State(state): Shared, Path(pid): Path<String>, body: Option<Json<Value>>) -> ApiResult {
    let mut params = body_object(body)?;
    let loaded = load(&state, &pid).await?;
    let scene = match params.remove("scene_id") {
        Some(Value::String(s)) => s,
        Some(Value::Number(n)) => n.to_string(),
        _ => return Err(ApiError::validation("scene_id is required.")),
    };
    views::find_scene(&loaded, &scene)?;
    let run = connect(&state.cfg).await.map_err(|e| ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "COMFY_UNAVAILABLE", e).retryable())?;
    let env = run.env;
    let render = RenderParams { overrides: params.into_iter().filter(|(k, _)| k != "mode").collect(), timeout_hours: None };
    let result = blocking(move || -> Result<Value, h3_pipeline::RenderError> {
        let prepared = prepare_scene_render(&*env, &loaded, &scene, &render)?;
        let built = h3_graph::build(&*env, prepared.workflow_key, &prepared.payload)?;
        Ok(json!({"scene_id": prepared.scene_id, "scene_number": prepared.scene_number, "workflow": prepared.workflow_key, "result": built}))
    })
    .await??;
    Ok(success(result, None))
}

// -- latents ----------------------------------------------------------------------------------------------------------------

async fn latent_list(State(state): Shared, Path(pid): Path<String>) -> ApiResult {
    let loaded = load(&state, &pid).await?;
    let folder = loaded.project_folder.clone();
    let count = views::segments(&loaded.session).len() as u32;
    let items = blocking(move || (1..=count).filter_map(|n| latents::latent_info(&folder, n).ok()).collect::<Vec<_>>()).await?;
    Ok(success(json!({"scenes": items}), Some(loaded.revision)))
}

async fn latent_dirty(State(state): Shared, Path(pid): Path<String>) -> ApiResult {
    let loaded = load(&state, &pid).await?;
    let folder = loaded.project_folder.clone();
    let dirty = blocking(move || latents::list_dirty(&folder)).await?;
    Ok(success(json!({"dirty_scenes": dirty}), Some(loaded.revision)))
}

fn scene_number(loaded: &LoadedSession, sid: &str) -> Result<u32, ApiError> {
    Ok(views::find_scene(loaded, sid)? as u32 + 1)
}

async fn latent_one(State(state): Shared, Path((pid, sid)): Path<(String, String)>) -> ApiResult {
    let loaded = load(&state, &pid).await?;
    let number = scene_number(&loaded, &sid)?;
    let folder = loaded.project_folder.clone();
    let info = blocking(move || latents::latent_info(&folder, number)).await?.map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(success(serde_json::to_value(info).unwrap_or(Value::Null), Some(loaded.revision)))
}

async fn latent_delete(State(state): Shared, Path((pid, sid)): Path<(String, String)>, Query(q): Query<HashMap<String, String>>) -> ApiResult {
    let loaded = load(&state, &pid).await?;
    let number = scene_number(&loaded, &sid)?;
    let folder = loaded.project_folder.clone();
    let all = flag(&q, "all").unwrap_or(false);
    let reindex = flag(&q, "reindex").unwrap_or(false);
    let deleted = blocking(move || if all { latents::delete_all_latents(&folder) } else { usize::from(latents::delete_latent(&folder, number, reindex)) }).await?;
    Ok(success(json!({"deleted": deleted}), Some(loaded.revision)))
}
