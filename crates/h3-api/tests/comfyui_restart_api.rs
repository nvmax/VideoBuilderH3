//! The Builder's Restart ComfyUI button through the real router, against small stand-ins for ComfyUI. The app asks ComfyUI-Manager's reboot
//! route (POST, then GET, at the v2 address and then the older one) and tells the button whether ComfyUI is restarting or why it cannot.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::State;
use axum::http::{Method, StatusCode, Uri};
use axum::routing::any;
use axum::Router;
use h3_project::AppConfig;
use reqwest::Client;
use serde_json::Value;
use tokio::io::AsyncReadExt;

const RESTART: &str = "/vrgdg/video_builder/comfyui/restart";

/// What the stand-in ComfyUI was asked: "METHOD /path".
type Seen = Arc<Mutex<Vec<String>>>;

/// A ComfyUI that records every call and answers `handler`: what it says for a given method and path.
async fn comfy(handler: fn(&Method, &str) -> StatusCode) -> (String, Seen) {
    let seen = Seen::default();
    let app = Router::new().fallback(any(move |State(seen): State<Seen>, method: Method, uri: Uri| async move {
        seen.lock().unwrap().push(format!("{method} {}", uri.path()));
        handler(&method, uri.path())
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let state = Arc::clone(&seen);
    tokio::spawn(async move { axum::serve(listener, app.with_state(state)).await.unwrap() });
    (url, seen)
}

/// The app with `comfy_url` as its ComfyUI. Gives the address of the app, which lives as long as the temporary folder in the tuple.
async fn app(comfy_url: String) -> (String, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let projects = dir.path().join("projects");
    std::fs::create_dir_all(&projects).unwrap();
    let cfg = AppConfig { comfy_url, project_root: projects, ..AppConfig::default() };
    let state = h3_api::build_state(cfg, &dir.path().join("jobs.db")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, h3_api::router(state)).await.unwrap() });
    (base, dir)
}

async fn restart(base: &str) -> (u16, Value) {
    let client = Client::builder().timeout(Duration::from_secs(30)).build().unwrap();
    let reply = client.post(format!("{base}{RESTART}")).send().await.unwrap();
    (reply.status().as_u16(), reply.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn a_manager_with_a_get_only_v2_route_is_asked_with_get_after_the_post_is_refused() {
    // ComfyUI answers a POST at an address that only has a GET route with 405.
    let (url, seen) = comfy(|method, path| match (method.as_str(), path) {
        ("GET", "/v2/manager/reboot") => StatusCode::OK,
        (_, "/v2/manager/reboot") => StatusCode::METHOD_NOT_ALLOWED,
        _ => StatusCode::NOT_FOUND,
    })
    .await;
    let (base, _dir) = app(url).await;
    let (code, body) = restart(&base).await;
    assert_eq!(code, 202, "{body}");
    assert_eq!((&body["ok"], &body["status"]), (&Value::Bool(true), &Value::from("restarting")));
    assert_eq!(*seen.lock().unwrap(), ["POST /v2/manager/reboot", "GET /v2/manager/reboot"]);
}

#[tokio::test]
async fn an_older_manager_is_asked_at_its_own_address_after_the_v2_one_is_not_found() {
    // The way a real ComfyUI answers an address it has no route for: a POST gets 405 and a GET gets 404 (its catch-all route takes the GET).
    let (url, seen) = comfy(|method, path| match (method.as_str(), path) {
        ("POST", "/manager/reboot") => StatusCode::OK,
        ("POST", _) => StatusCode::METHOD_NOT_ALLOWED,
        _ => StatusCode::NOT_FOUND,
    })
    .await;
    let (base, _dir) = app(url).await;
    let (code, _) = restart(&base).await;
    assert_eq!(code, 202);
    assert_eq!(*seen.lock().unwrap(), ["POST /v2/manager/reboot", "GET /v2/manager/reboot", "POST /manager/reboot"]);
}

#[tokio::test]
async fn without_a_manager_it_says_so_and_nothing_else_is_asked() {
    let (url, seen) = comfy(|method, _| if method == Method::POST { StatusCode::METHOD_NOT_ALLOWED } else { StatusCode::NOT_FOUND }).await;
    let (base, _dir) = app(url).await;
    let (code, body) = restart(&base).await;
    assert_eq!(code, 501, "{body}");
    assert!(body["error"].as_str().unwrap().contains("ComfyUI-Manager is not installed"), "{body}");
    assert_eq!(seen.lock().unwrap().len(), 4, "both addresses, with both methods");
}

#[tokio::test]
async fn a_refusal_by_the_managers_security_level_says_so() {
    let (url, _seen) = comfy(|_, _| StatusCode::FORBIDDEN).await;
    let (base, _dir) = app(url).await;
    let (code, body) = restart(&base).await;
    assert_eq!(code, 502);
    assert!(body["error"].as_str().unwrap().contains("security level"), "{body}");
}

#[tokio::test]
async fn any_other_answer_is_an_error_with_its_code() {
    let (url, _seen) = comfy(|_, _| StatusCode::INTERNAL_SERVER_ERROR).await;
    let (base, _dir) = app(url).await;
    let (code, body) = restart(&base).await;
    assert_eq!(code, 502);
    assert!(body["error"].as_str().unwrap().contains("500"), "{body}");
}

#[tokio::test]
async fn a_connection_that_closes_without_an_answer_means_the_restart_began() {
    // ComfyUI-Manager restarts the process while it answers: the connection is gone with no reply.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = [0u8; 1024];
            let _ = stream.read(&mut buffer).await;
            drop(stream);
        }
    });
    let (base, _dir) = app(url).await;
    let (code, body) = restart(&base).await;
    assert_eq!(code, 202, "{body}");
    assert_eq!(body["status"], "restarting");
}

#[tokio::test]
async fn a_comfyui_that_is_not_running_is_reported_as_unreachable() {
    // A port nothing listens on.
    let free = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", free.local_addr().unwrap());
    drop(free);
    let (base, _dir) = app(url).await;
    let (code, body) = restart(&base).await;
    assert_eq!(code, 502);
    assert!(body["error"].as_str().unwrap().contains("not reachable"), "{body}");
}
