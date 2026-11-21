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
        .route("/vrgdg/video_builder/custom_nodes/install/progress", get(custom_nodes_install_progress))
        .route("/vrgdg/video_builder/comfyui/restart", post(comfyui_restart))
}

/// ComfyUI-Manager's reboot routes: the v2 address of its newer versions, then the older one.
const MANAGER_REBOOT_PATHS: [&str; 2] = ["/v2/manager/reboot", "/manager/reboot"];

/// Ask ComfyUI-Manager (at `base`) to restart ComfyUI. Each address is asked with POST and then GET, because the route takes one of them and the
/// other answers 405. A ComfyUI without ComfyUI-Manager answers 404 (a GET) or 405 (a POST) at every address. ComfyUI-Manager restarts the
/// process while it answers, so a connection that closes without an answer, or an answer that does not come in time, means it is restarting.
async fn ask_manager_to_restart(base: &str) -> Result<(), (StatusCode, String)> {
    let http = reqwest::Client::builder().timeout(Duration::from_secs(10)).build().map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    for path in MANAGER_REBOOT_PATHS {
        for method in [reqwest::Method::POST, reqwest::Method::GET] {
            match http.request(method, format!("{base}{path}")).send().await {
                Ok(reply) => match reply.status().as_u16() {
                    200..=299 => return Ok(()),
                    // No such route here: the next address.
                    404 => break,
                    // The route takes the other method.
                    405 => continue,
                    403 => return Err((StatusCode::BAD_GATEWAY, "ComfyUI-Manager refused to restart ComfyUI. Its security level has to be normal or lower for that. Restart ComfyUI by hand.".into())),
                    code => return Err((StatusCode::BAD_GATEWAY, format!("ComfyUI-Manager answered {code} when it was asked to restart ComfyUI."))),
                },
                Err(e) if e.is_connect() => return Err((StatusCode::BAD_GATEWAY, format!("ComfyUI is not reachable at {base}: {e}"))),
                Err(_) => return Ok(()),
            }
        }
    }
    Err((StatusCode::NOT_IMPLEMENTED, "ComfyUI-Manager is not installed in this ComfyUI, so it cannot restart itself. Close ComfyUI and start it again.".into()))
}

/// `POST /vrgdg/video_builder/comfyui/restart` (the Restart ComfyUI button): restart ComfyUI through ComfyUI-Manager. The pack serves the same
/// route inside ComfyUI. This app serves it itself so it works with or without the pack, and its answer is not lost with ComfyUI's connection.
async fn comfyui_restart(State(state): State<Arc<AppState>>) -> Response {
    let base = state.cfg.comfy_base().to_string();
    match ask_manager_to_restart(&base).await {
        Ok(()) => {
            eprintln!("[VRGDG-RS Restart] ComfyUI at {base} is restarting at the Builder's request.");
            (StatusCode::ACCEPTED, Json(json!({"ok": true, "status": "restarting"}))).into_response()
        }
        Err((status, why)) => super::failure(status, why),
    }
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

/// The version ComfyUI reports for itself, when it is running.
async fn comfyui_version(state: &AppState) -> Option<String> {
    ComfyClient::new(state.cfg.comfy_base()).system_stats().await.ok().map(|stats| stats.system.comfyui_version).filter(|v| !v.is_empty())
}

/// `GET /vrgdg/video_builder/custom_nodes/status`: the pack's own answer from ComfyUI. Without it the folder is read here. Installing is then
/// possible when ComfyUI's Python has ComfyUI-Manager, and the answer says why it is not otherwise (`manager_note`).
async fn custom_nodes_status(State(state): State<Arc<AppState>>) -> Response {
    match pack_custom_nodes_status(&state).await {
        Ok(status) => Json(status).into_response(),
        Err(note) => {
            let dir = custom_nodes_dir(&state).await;
            let version = comfyui_version(&state).await;
            // The Python that has ComfyUI-Manager, and whether ComfyUI's Python has the RTX Video Super Resolution library (it needs a Python, not the manager).
            let (manager, vfx) = match dir.clone() {
                Some(dir) => tokio::task::spawn_blocking(move || {
                    use h3_builder::system::{comfy_python, manager_python, nvidia_vfx_installed};
                    let manager = manager_python(Some(&dir));
                    let vfx = manager.clone().or_else(|_| comfy_python(Some(&dir))).is_ok_and(|python| nvidia_vfx_installed(&python));
                    (manager, vfx)
                })
                .await
                .unwrap_or_else(|e| (Err(e.to_string()), false)),
                None => (Err(note), false),
            };
            let mut status = h3_builder::system::custom_nodes_status(dir.as_deref(), version.as_deref(), manager.is_ok(), vfx);
            if let Err(why) = manager {
                status["manager_note"] = json!(why);
            }
            Json(status).into_response()
        }
    }
}

/// `POST /vrgdg/video_builder/custom_nodes/install`: the pack inside ComfyUI installs the nodes with ComfyUI-Manager, so the call goes there as it
/// is. ComfyUI answers 404, or 405 when a catch-all route takes the GET of the same address, when it does not have the pack's route. The app
/// then runs ComfyUI-Manager's `cm_cli` itself in ComfyUI's Python.
async fn custom_nodes_install(State(state): State<Arc<AppState>>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let bytes = match axum::body::to_bytes(body, crate::app::REQUEST_BODY_LIMIT).await {
        Ok(bytes) => bytes,
        Err(e) => return super::failure(StatusCode::BAD_REQUEST, format!("The request body could not be read: {e}")),
    };
    let reply = super::proxy_to_comfy(&state, Request::from_parts(parts, axum::body::Body::from(bytes.clone()))).await;
    if !matches!(reply.status(), StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED) {
        return reply;
    }
    install_without_pack(&state, &bytes).await
}

/// `GET /vrgdg/video_builder/custom_nodes/install/progress`: how far the install the app runs itself has come (the pack inside ComfyUI has no
/// such route, so an install it runs shows no progress). The Custom Nodes window reads it while its install request is open.
async fn custom_nodes_install_progress() -> Response {
    ([(header::CACHE_CONTROL, "no-store")], Json(h3_builder::system::install_progress())).into_response()
}

/// The install without the VRGDG pack in ComfyUI: `cm_cli install` for every id, in the Python that runs ComfyUI.
async fn install_without_pack(state: &AppState, body: &[u8]) -> Response {
    let ids: Vec<String> = match serde_json::from_slice::<Value>(body).ok().as_ref().and_then(|v| v["ids"].as_array()) {
        Some(ids) => ids.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        None => return super::failure(StatusCode::BAD_REQUEST, "ids must be a list."),
    };
    let Some(dir) = custom_nodes_dir(state).await else {
        return super::failure(StatusCode::NOT_IMPLEMENTED, "ComfyUI did not say where it keeps its custom nodes, so nodes cannot be installed from here. Update ComfyUI and try again.");
    };
    let version = comfyui_version(state).await;
    eprintln!("[VRGDG-RS Custom Nodes] Installing {} without the VRGDG pack in ComfyUI: {}", ids.len(), ids.join(", "));
    let installed = tokio::task::spawn_blocking(move || {
        // A pip library needs ComfyUI's Python only. Nodes need the Python that has ComfyUI-Manager.
        let python = if ids.iter().all(|id| id == h3_builder::system::NVIDIA_VFX_ID) { h3_builder::system::comfy_python(Some(&dir)) } else { h3_builder::system::manager_python(Some(&dir)) }
            .map_err(|why| (StatusCode::NOT_IMPLEMENTED, why))?;
        h3_builder::system::install_custom_nodes(&python, &dir, &ids, version.as_deref())
            .map_err(|why| (if why == h3_builder::system::INSTALL_RUNNING { StatusCode::CONFLICT } else { StatusCode::BAD_REQUEST }, why))
    })
    .await;
    match installed {
        Ok(Ok(results)) => Json(json!({"ok": true, "results": results, "restart_required": true})).into_response(),
        Ok(Err((status, why))) => super::failure(status, why),
        Err(e) => super::failure(StatusCode::INTERNAL_SERVER_ERROR, format!("The install stopped unexpectedly: {e}")),
    }
}
