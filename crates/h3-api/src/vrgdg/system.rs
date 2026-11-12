//! System routes of the Builder: the header meters, memory cleanup, the update banner and the custom node check.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Request, State};
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
        .route(CUSTOM_NODES_STATUS, get(custom_nodes_status))
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

const CUSTOM_NODES_STATUS: &str = "/vrgdg/video_builder/custom_nodes/status";

/// What the VRGDG node pack inside ComfyUI answers for the custom node check, or why there is no answer (the text the dialog shows). The pack
/// knows what this app cannot see from outside: whether ComfyUI-Manager is in ComfyUI's Python, which ComfyUI version this is, and what that
/// version has built in.
async fn pack_custom_nodes_status(state: &AppState) -> Result<Value, String> {
    let base = state.cfg.comfy_base().to_string();
    let http = reqwest::Client::builder().timeout(Duration::from_secs(15)).build().map_err(|e| e.to_string())?;
    let response = http
        .get(format!("{base}{CUSTOM_NODES_STATUS}"))
        .send()
        .await
        .map_err(|_| format!("ComfyUI is not reachable at {base}, so nodes cannot be installed from here. Start ComfyUI and open this window again."))?;
    if response.status().as_u16() == 404 {
        return Err("The VRGDG node pack is not installed in the ComfyUI this app uses, so nodes cannot be installed from here. Install the pack, or install the nodes with ComfyUI-Manager, then restart ComfyUI.".into());
    }
    let status: Value = response.json().await.map_err(|e| format!("The VRGDG node pack in ComfyUI gave an answer this app cannot read: {e}"))?;
    if status["ok"] == true && status["nodes"].is_array() {
        Ok(status)
    } else {
        Err(format!("The VRGDG node pack in ComfyUI could not check the custom nodes: {}", status["error"].as_str().unwrap_or("no reason given")))
    }
}

/// `GET /vrgdg/video_builder/custom_nodes/status`: the pack's own answer from ComfyUI. Without it the folder is read here, and the answer says
/// installing is not possible and why (`manager_note`).
async fn custom_nodes_status(State(state): State<Arc<AppState>>) -> Response {
    match pack_custom_nodes_status(&state).await {
        Ok(status) => Json(status).into_response(),
        Err(note) => {
            let dir = custom_nodes_dir(&state).await;
            let mut status = h3_builder::system::custom_nodes_status(dir.as_deref());
            status["manager_note"] = json!(note);
            Json(status).into_response()
        }
    }
}

/// `POST /vrgdg/video_builder/custom_nodes/install`: the pack inside ComfyUI installs the nodes with ComfyUI-Manager, so the call goes there as it
/// is. A 404 means ComfyUI does not have the pack's route.
async fn custom_nodes_install(State(state): State<Arc<AppState>>, request: Request) -> Response {
    let reply = super::proxy_to_comfy(&state, request).await;
    if reply.status() == StatusCode::NOT_FOUND {
        return super::failure(StatusCode::NOT_IMPLEMENTED, "ComfyUI has no custom node install route. Installing from here needs the VRGDG node pack in the ComfyUI this app uses. Install the pack, or install the nodes with ComfyUI-Manager, then restart ComfyUI.");
    }
    reply
}
