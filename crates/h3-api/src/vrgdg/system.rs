//! System routes of the Builder: the header meters, memory cleanup, the update banner and the custom node check.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use h3_comfy::ComfyClient;
use serde_json::{json, Value};

use crate::app::AppState;

static CLEANUPS: AtomicU64 = AtomicU64::new(1);
static CACHED: Mutex<Option<(Instant, Value)>> = Mutex::new(None);

pub fn routes(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route("/vrgdg/resource-monitor", get(resource_monitor))
        .route("/vrgdg/resource-monitor/clear-memory", post(clear_memory))
        .route("/vrgdg/update/v10/status", get(update_status))
        .route("/vrgdg/update/v10", post(update))
        .route("/vrgdg/video_builder/custom_nodes/status", get(custom_nodes_status))
        .route("/vrgdg/video_builder/custom_nodes/install", post(custom_nodes_install))
}

/// RAM and GPU readings, shared for one second like the pack does so several open Builders cost one reading.
async fn resource_monitor() -> Response {
    let cached = CACHED.lock().ok().and_then(|c| c.as_ref().filter(|(at, _)| at.elapsed() < Duration::from_secs(1)).map(|(_, v)| v.clone()));
    let reading = match cached {
        Some(reading) => reading,
        None => {
            let reading = tokio::task::spawn_blocking(h3_builder::system::read_resources).await.unwrap_or_else(|_| json!({"ram": {"used": 0, "total": 0}, "gpus": [], "gpu_status": "unavailable"}));
            if let Ok(mut cache) = CACHED.lock() {
                *cache = Some((Instant::now(), reading.clone()));
            }
            reading
        }
    };
    ([(header::CACHE_CONTROL, "no-store")], Json(reading)).into_response()
}

/// `POST /vrgdg/resource-monitor/clear-memory`: ask ComfyUI to unload its models and free memory. Refused while a job
/// runs or waits.
async fn clear_memory(State(state): State<Arc<AppState>>) -> Response {
    let base = state.cfg.comfy_base().to_string();
    let http = reqwest::Client::new();
    let busy = match http.get(format!("{base}/queue")).send().await {
        Ok(response) => {
            let queue: Value = response.json().await.unwrap_or(Value::Null);
            let count = |key: &str| queue.get(key).and_then(Value::as_array).map_or(0, Vec::len);
            count("queue_running") + count("queue_pending") > 0
        }
        Err(e) => return super::failure(StatusCode::INTERNAL_SERVER_ERROR, format!("ComfyUI is not reachable at {base}: {e}")),
    };
    if busy {
        eprintln!("[VRGDG-RS Clear Memory] Skipped: running or queued jobs; nothing cleared.");
        return (StatusCode::CONFLICT, Json(json!({"ok": false, "error": "Wait for the running and queued jobs to finish."}))).into_response();
    }
    let freed = http.post(format!("{base}/free")).json(&json!({"unload_models": true, "free_memory": true})).send().await;
    match freed {
        Ok(response) if response.status().is_success() => {
            let id = CLEANUPS.fetch_add(1, Ordering::Relaxed);
            eprintln!("[VRGDG-RS Clear Memory #{id}] Requested model unloading and memory cleanup in ComfyUI.");
            (StatusCode::ACCEPTED, Json(json!({"ok": true, "status": "requested", "gguf_models_unloaded": 0, "cleanup_id": id}))).into_response()
        }
        Ok(response) => super::failure(StatusCode::INTERNAL_SERVER_ERROR, format!("ComfyUI refused the cleanup request ({}).", response.status())),
        Err(e) => super::failure(StatusCode::INTERNAL_SERVER_ERROR, format!("Cleanup request failed: {e}")),
    }
}

/// The pack's updater works on its own git checkout. This app is updated by its installer.
async fn update_status() -> Response {
    Json(json!({"ok": false, "error": "H3 Studio is updated with its installer."})).into_response()
}

async fn update() -> Response {
    super::failure(StatusCode::BAD_REQUEST, "H3 Studio is updated with its installer.")
}

/// ComfyUI's custom node folder, from the folders it reports.
async fn custom_nodes_dir(state: &AppState) -> Option<PathBuf> {
    let paths = ComfyClient::new(state.cfg.comfy_base()).folder_paths().await?;
    paths.get("custom_nodes")?.as_array()?.iter().filter_map(Value::as_str).map(PathBuf::from).find(|p| p.is_dir())
}

async fn custom_nodes_status(State(state): State<Arc<AppState>>) -> Response {
    let dir = custom_nodes_dir(&state).await;
    Json(h3_builder::system::custom_nodes_status(dir.as_deref())).into_response()
}

async fn custom_nodes_install() -> Response {
    super::failure(StatusCode::BAD_REQUEST, "Installing custom nodes is not available in H3 Studio. Install them with ComfyUI-Manager and restart ComfyUI.")
}
