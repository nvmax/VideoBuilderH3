//! The Builder's Custom Nodes window through the real router, against a small stand-in for ComfyUI. The VRGDG node pack inside ComfyUI knows
//! whether ComfyUI-Manager is in its Python, which ComfyUI version this is and what that version has built in, and it installs the nodes. So
//! the app asks the pack, and only reads the folder itself (and says why installing is not possible) when the pack cannot answer.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use h3_project::AppConfig;
use reqwest::Client;
use serde_json::{json, Value};

const STATUS: &str = "/vrgdg/video_builder/custom_nodes/status";
const INSTALL: &str = "/vrgdg/video_builder/custom_nodes/install";

/// The bodies the stand-in ComfyUI got at the pack's install route.
type Seen = Arc<Mutex<Vec<Value>>>;

struct Server {
    base: String,
    client: Client,
    seen: Seen,
    /// Keeps the temporary folders alive.
    _dir: tempfile::TempDir,
}

/// What the pack answers on a ComfyUI 0.45.0 that has ComfyUI-Manager: one pack to install and one that ComfyUI has built in.
fn pack_status() -> Value {
    json!({
        "ok": true, "custom_nodes_dir": "C:/ComfyUI/custom_nodes", "manager_available": true, "comfyui_version": "0.45.0",
        "nodes": [
            {"id": "gguf", "folder": "ComfyUI-GGUF", "installed": false, "source": "registry", "install": "ComfyUI-GGUF"},
            {"id": "minimax_h3_latent_upscaler", "folder": "Comfyui_Minimax_h3_latent_Upscaler", "installed": false, "source": "github",
             "install": "https://github.com/LBH-123-AI/Comfyui_Minimax_h3_latent_Upscaler.git", "built_in": true, "built_in_since": "0.44.0"}
        ]
    })
}

/// The stand-in ComfyUI. It reports `custom_nodes` as its node folder, and with `pack` it has the pack's status and install routes (an install
/// of the id `fails` is refused the way the pack refuses one), without it ComfyUI has no such routes and answers 404.
async fn comfy(pack: bool, custom_nodes: &Path, seen: Seen) -> String {
    let folders = json!({"custom_nodes": [custom_nodes.display().to_string()]});
    let app: Router<Seen> = Router::new().route("/internal/folder_paths", get(move || {
        let folders = folders.clone();
        async move { Json(folders) }
    }));
    let app = if pack {
        app.route(STATUS, get(|| async { Json(pack_status()) })).route(
            INSTALL,
            post(|State(seen): State<Seen>, body: Bytes| async move {
                let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                seen.lock().unwrap().push(body.clone());
                if body["ids"] == json!(["fails"]) {
                    (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": "cm_cli install failed"})))
                } else {
                    (StatusCode::OK, Json(json!({"ok": true, "results": [{"id": "gguf", "action": "installed"}], "restart_required": true})))
                }
            }),
        )
    } else {
        app
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app.with_state(seen)).await.unwrap() });
    url
}

async fn start_with(comfy_url: String, seen: Seen, dir: tempfile::TempDir) -> Server {
    let projects = dir.path().join("projects");
    std::fs::create_dir_all(&projects).unwrap();
    let cfg = AppConfig { comfy_url, project_root: projects, ..AppConfig::default() };
    let state = h3_api::build_state(cfg, &dir.path().join("jobs.db")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, h3_api::router(state)).await.unwrap() });
    Server { base: format!("http://{addr}"), client: Client::builder().timeout(Duration::from_secs(30)).build().unwrap(), seen, _dir: dir }
}

/// The app with a ComfyUI that has the pack's routes (`pack`) or not, and a `comfyui-kjnodes` folder in its custom nodes.
async fn start(pack: bool) -> Server {
    let dir = tempfile::tempdir().unwrap();
    let custom_nodes = dir.path().join("ComfyUI").join("custom_nodes");
    std::fs::create_dir_all(custom_nodes.join("comfyui-kjnodes")).unwrap();
    let seen = Seen::default();
    let url = comfy(pack, &custom_nodes, Arc::clone(&seen)).await;
    start_with(url, seen, dir).await
}

impl Server {
    async fn status(&self) -> Value {
        let response = self.client.get(format!("{}{STATUS}", self.base)).send().await.unwrap();
        assert_eq!(response.status().as_u16(), 200);
        response.json().await.unwrap()
    }

    async fn install(&self, ids: Value) -> (u16, Value) {
        let response = self.client.post(format!("{}{INSTALL}", self.base)).json(&json!({"ids": ids})).send().await.unwrap();
        (response.status().as_u16(), response.json().await.unwrap())
    }
}

fn node<'a>(status: &'a Value, id: &str) -> &'a Value {
    status["nodes"].as_array().unwrap().iter().find(|n| n["id"] == id).unwrap_or_else(|| panic!("no {id} in {status}"))
}

#[tokio::test]
async fn with_the_pack_in_comfyui_the_answer_is_the_packs_own() {
    let status = start(true).await.status().await;
    assert_eq!(status, pack_status(), "the pack's answer is passed on as it is");
    assert_eq!((status["manager_available"].clone(), status["comfyui_version"].clone()), (json!(true), json!("0.45.0")));
    assert!(status.get("manager_note").is_none());
    assert_eq!((node(&status, "minimax_h3_latent_upscaler")["built_in"].clone(), node(&status, "gguf")["installed"].clone()), (json!(true), json!(false)));
}

#[tokio::test]
async fn without_the_pack_the_folder_is_read_here_and_the_note_says_why_nothing_installs() {
    let status = start(false).await.status().await;
    assert_eq!(status["manager_available"], false);
    let note = status["manager_note"].as_str().unwrap();
    assert!(note.contains("VRGDG node pack is not installed in the ComfyUI this app uses"), "{note}");
    // The folder scan still tells what is installed.
    assert_eq!((node(&status, "kjnodes")["installed"].clone(), node(&status, "gguf")["installed"].clone()), (json!(true), json!(false)));
    assert_eq!(status["nodes"].as_array().unwrap().len(), 9);
}

#[tokio::test]
async fn with_comfyui_down_the_note_says_it_is_not_reachable() {
    let dir = tempfile::tempdir().unwrap();
    let status = start_with("http://127.0.0.1:9".into(), Seen::default(), dir).await.status().await;
    assert_eq!(status["manager_available"], false);
    let note = status["manager_note"].as_str().unwrap();
    assert!(note.contains("ComfyUI is not reachable") && note.contains("127.0.0.1:9"), "{note}");
    assert!(status["nodes"].as_array().unwrap().iter().all(|n| n["installed"] == false), "{status}");
}

#[tokio::test]
async fn an_install_goes_to_the_pack_with_its_ids_and_comes_back_with_its_answer() {
    let server = start(true).await;
    let (status, answer) = server.install(json!(["gguf"])).await;
    assert_eq!(status, 200);
    assert_eq!((answer["ok"].clone(), answer["restart_required"].clone(), answer["results"][0]["action"].clone()), (json!(true), json!(true), json!("installed")));
    assert_eq!(server.seen.lock().unwrap().clone(), vec![json!({"ids": ["gguf"]})]);

    // A refusal of the pack reaches the window with the pack's own words.
    let (status, answer) = server.install(json!(["fails"])).await;
    assert_eq!((status, answer["error"].clone()), (400, json!("cm_cli install failed")));
}

#[tokio::test]
async fn an_install_without_the_pack_says_what_to_install() {
    let (status, answer) = start(false).await.install(json!(["gguf"])).await;
    assert_eq!(status, 501);
    assert!(answer["error"].as_str().unwrap().contains("needs the VRGDG node pack in the ComfyUI this app uses"), "{answer}");
}

#[tokio::test]
async fn an_install_with_comfyui_down_says_comfyui_is_not_reachable() {
    let server = start_with("http://127.0.0.1:9".into(), Seen::default(), tempfile::tempdir().unwrap()).await;
    let (status, answer) = server.install(json!(["gguf"])).await;
    assert_eq!(status, 502);
    assert!(answer["error"].as_str().unwrap().contains("ComfyUI is not reachable"), "{answer}");
}
