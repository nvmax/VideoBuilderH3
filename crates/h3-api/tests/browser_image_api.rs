//! The Browser AI routes of the Builder through the real router, against a small stand-in for ComfyUI. The Image panel's Browser AI card
//! (setup, sign-in, the workflow builder) works with the VRGDG node pack inside ComfyUI, so this server passes those routes on to it. The
//! thumbnail dialog does not use them: it has its own routes, which drive Chrome from the app (`thumbnail_api.rs`). No browser or
//! provider is involved here.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{Method, StatusCode, Uri};
use axum::routing::get;
use axum::{Json, Router};
use h3_project::AppConfig;
use reqwest::Client;
use serde_json::{json, Value};

/// What the stand-in ComfyUI was asked: method, path and the JSON body.
type Seen = Arc<Mutex<Vec<(String, String, Value)>>>;

struct Server {
    base: String,
    client: Client,
    seen: Seen,
    /// Keeps the temporary project folder alive.
    _dir: tempfile::TempDir,
}

/// The stand-in ComfyUI. `nodes` is its node list. With `routes` it serves the pack's routes (every other path it answers like the pack does: a
/// prompt that is empty is refused with the error envelope, anything else is ok), without it ComfyUI has no such route and answers 404.
async fn comfy(nodes: Value, routes: bool, seen: Seen) -> String {
    let app: Router<Seen> = Router::new().route("/object_info", get(move || {
        let nodes = nodes.clone();
        async move { Json(nodes) }
    }));
    let app = if routes {
        app.fallback(|State(seen): State<Seen>, method: Method, uri: Uri, body: Bytes| async move {
            let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            seen.lock().unwrap().push((method.to_string(), uri.path_and_query().unwrap().to_string(), body.clone()));
            if body["prompt"] == "" {
                (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": "Flow Nano Banana prompt text is empty."})))
            } else {
                (StatusCode::OK, Json(json!({"ok": true, "answered_by": "comfyui"})))
            }
        })
    } else {
        app
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app.with_state(seen)).await.unwrap() });
    url
}

async fn start_with(comfy_url: String, seen: Seen) -> Server {
    let dir = tempfile::tempdir().unwrap();
    let projects = dir.path().join("projects");
    std::fs::create_dir_all(&projects).unwrap();
    let cfg = AppConfig { comfy_url, project_root: projects, ..AppConfig::default() };
    let state = h3_api::build_state(cfg, &dir.path().join("jobs.db")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, h3_api::router(state)).await.unwrap() });
    Server { base: format!("http://{addr}"), client: Client::builder().timeout(Duration::from_secs(30)).build().unwrap(), seen, _dir: dir }
}

/// The app with a ComfyUI that has the node list `nodes`, and the pack's routes when `routes`.
async fn start(nodes: Value, routes: bool) -> Server {
    let seen = Seen::default();
    let url = comfy(nodes, routes, Arc::clone(&seen)).await;
    start_with(url, seen).await
}

fn with_node() -> Value {
    json!({"VRGDG_FlowBrowserImageEdit": {}, "VRGDG_ChatGPTImagesBrowser": {}, "VRGDG_MetaAIBrowserImage": {}, "PreviewImage": {}})
}

impl Server {
    async fn capabilities(&self) -> Value {
        self.client.get(format!("{}/vrgdg/music_builder/thumbnail/capabilities", self.base)).send().await.unwrap().json().await.unwrap()
    }

    async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        let response = self.client.post(format!("{}{path}", self.base)).json(&body).send().await.unwrap();
        (response.status().as_u16(), response.json().await.unwrap())
    }
}

#[tokio::test]
async fn the_dialog_is_offered_browser_ai_that_the_app_runs_itself_whatever_comfyui_has() {
    // The thumbnail dialog's Browser AI engine drives Chrome from the app (`browser_ai_direct`): it needs neither the pack's nodes nor Playwright
    // in ComfyUI, so the answer is the same with the node, without it and with ComfyUI down.
    for server in [start(with_node(), true).await, start(json!({"PreviewImage": {}}), true).await, start_with("http://127.0.0.1:9".into(), Seen::default()).await] {
        let answer = server.capabilities().await;
        assert_eq!(answer["ok"], true, "{answer}");
        assert_eq!((answer["browser_ai"].clone(), answer["browser_ai_direct"].clone()), (json!(true), json!(true)), "{answer}");
        // The engines the app has on its own are still there, and the overlay engine still is not.
        assert_eq!((answer["frame"].clone(), answer["video_intro"].clone()), (json!(true), json!(true)));
        assert!(answer["api_image"]["providers"].is_object() && answer.get("overlay").is_none(), "{answer}");
    }
}

#[tokio::test]
async fn every_browser_ai_route_is_passed_on_to_comfyui_with_its_body_and_answer() {
    let server = start(with_node(), true).await;
    let body = json!({"provider": "flow_nano_banana", "prompt": "Add the title HEAT.", "aspect_ratio": "16:9", "image_ingredients": [{"path": "C:/p/source_0001.png"}], "timeout_seconds": 601});
    for path in [
        "/vrgdg/browser_image/setup",
        "/vrgdg/browser_image/open_login",
        "/vrgdg/browser_image/manual_open",
        "/vrgdg/browser_image/manual_upload",
        "/vrgdg/browser_image/manual_submit",
        "/vrgdg/browser_image/manual_finish",
        "/vrgdg/browser_image/store_reference",
        "/vrgdg/browser_image/manual_wait_download",
        "/vrgdg/browser_image/manual_import_latest",
        "/vrgdg/workflow_runner/build_flow_gpt_image_prompt",
    ] {
        let (status, answer) = server.post(path, body.clone()).await;
        assert_eq!((status, answer["answered_by"].clone()), (200, json!("comfyui")), "{path}: {answer}");
    }
    let status = server.client.get(format!("{}/vrgdg/browser_image/status?provider=flow_nano_banana", server.base)).send().await.unwrap();
    assert_eq!(status.status().as_u16(), 200);

    let seen = server.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 11, "{seen:?}");
    // The builder route got the request the dialog made, untouched.
    let (method, path, forwarded) = &seen[9];
    assert_eq!((method.as_str(), path.as_str()), ("POST", "/vrgdg/workflow_runner/build_flow_gpt_image_prompt"));
    assert_eq!(forwarded, &body);
    // The GET kept its query.
    assert_eq!((seen[10].0.as_str(), seen[10].1.as_str()), ("GET", "/vrgdg/browser_image/status?provider=flow_nano_banana"));
}

#[tokio::test]
async fn a_refusal_of_the_pack_reaches_the_dialog_with_its_status_and_error() {
    let server = start(with_node(), true).await;
    let (status, answer) = server.post("/vrgdg/workflow_runner/build_flow_gpt_image_prompt", json!({"provider": "flow_nano_banana", "prompt": ""})).await;
    assert_eq!(status, 400);
    assert_eq!((answer["ok"].clone(), answer["error"].clone()), (json!(false), json!("Flow Nano Banana prompt text is empty.")));
}

#[tokio::test]
async fn a_comfyui_without_the_pack_routes_is_told_apart_with_what_to_install() {
    let server = start(json!({"PreviewImage": {}}), false).await;
    let (status, answer) = server.post("/vrgdg/workflow_runner/build_flow_gpt_image_prompt", json!({"prompt": "x"})).await;
    assert_eq!(status, 501, "{answer}");
    assert_eq!(answer["ok"], false);
    let error = answer["error"].as_str().unwrap();
    assert!(error.contains("/vrgdg/workflow_runner/build_flow_gpt_image_prompt") && error.contains("VRGDG node pack"), "{error}");
}

#[tokio::test]
async fn browser_ai_routes_answer_with_the_error_envelope_when_comfyui_is_down() {
    let server = start_with("http://127.0.0.1:9".into(), Seen::default()).await;
    let (status, answer) = server.post("/vrgdg/browser_image/open_login", json!({"provider": "flow_nano_banana"})).await;
    assert_eq!(status, 502, "{answer}");
    assert_eq!(answer["ok"], false);
    assert!(answer["error"].as_str().unwrap().contains("ComfyUI is not reachable"), "{answer}");
}
