//! Only this computer may use the server: a web page of another site, a sandboxed page and a rebinding host name are refused on every kind of route, and
//! CORS answers only pages of this computer.

use std::time::Duration;

use h3_project::AppConfig;
use reqwest::{Client, Method, RequestBuilder};
use serde_json::{json, Value};

struct Server {
    root: String,
    client: Client,
    config: std::path::PathBuf,
    _dir: tempfile::TempDir,
}

async fn start() -> Server {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("projects");
    std::fs::create_dir_all(&root).unwrap();
    let cfg = AppConfig { comfy_url: "http://127.0.0.1:9".into(), project_root: root, ..AppConfig::default() };
    let state = h3_api::build_state(cfg, &dir.path().join("jobs.db")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, h3_api::router(state)).await.unwrap() });
    Server { root: format!("http://{addr}"), client: Client::builder().timeout(Duration::from_secs(30)).build().unwrap(), config: dir.path().join("config.json"), _dir: dir }
}

impl Server {
    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        self.client.request(method, format!("{}{path}", self.root))
    }
}

/// The routes of each kind a page could try: the REST API (a read and the dangerous write), a Builder route, the ComfyUI pass-through and MCP.
fn routes() -> Vec<(Method, &'static str)> {
    vec![
        (Method::GET, "/api/v1/health"),
        (Method::GET, "/api/v1/projects"),
        (Method::PUT, "/api/v1/setup/config"),
        (Method::POST, "/api/v1/projects"),
        (Method::POST, "/vrgdg/music_builder/list_instruction_presets"),
        (Method::GET, "/system_stats"),
        (Method::POST, "/mcp"),
    ]
}

#[tokio::test]
async fn a_page_of_another_site_cannot_use_any_route() {
    let s = start().await;
    for origin in ["https://evil.example", "null", "http://192.168.1.20:8190", "http://localhost.evil.example"] {
        for (method, path) in routes() {
            let response = s.request(method.clone(), path).header("origin", origin).json(&json!({"ffmpeg_path": "C:/evil/run.exe"})).send().await.unwrap();
            assert_eq!(response.status(), 403, "{method} {path} with origin {origin}");
            assert!(response.headers().get("access-control-allow-origin").is_none(), "{path}: the answer must not invite the page");
            let body: Value = response.json().await.unwrap();
            assert_eq!(body["error"]["code"], "FOREIGN_ORIGIN", "{path}");
        }
    }
    // The setting a page could have changed was not changed.
    assert!(!s.config.exists(), "the config file was not written");
}

#[tokio::test]
async fn a_rebinding_host_name_cannot_use_any_route() {
    let s = start().await;
    for (method, path) in routes() {
        let response = s.request(method.clone(), path).header("host", "rebind.attacker.example:8190").send().await.unwrap();
        assert_eq!(response.status(), 403, "{method} {path}");
        assert_eq!(response.json::<Value>().await.unwrap()["error"]["code"], "FOREIGN_HOST", "{path}");
    }
}

#[tokio::test]
async fn this_computer_and_programs_without_an_origin_still_work() {
    let s = start().await;
    // No origin: curl, an agent, the MCP bridge, the app's own page reading its own files.
    assert_eq!(s.request(Method::GET, "/api/v1/health").send().await.unwrap().status(), 200);
    assert_eq!(s.request(Method::GET, "/api/v1/projects").header("host", "localhost:8190").send().await.unwrap().status(), 200);
    // The app window and a dev server on this computer.
    for origin in [s.root.as_str(), "http://localhost:1420", "http://tauri.localhost", "http://[::1]:8190"] {
        let response = s.request(Method::GET, "/api/v1/health").header("origin", origin).send().await.unwrap();
        assert_eq!(response.status(), 200, "{origin}");
        assert_eq!(response.headers().get("access-control-allow-origin").and_then(|v| v.to_str().ok()), Some(origin), "{origin} is answered with CORS");
    }
    let posted = s.request(Method::POST, "/api/v1/projects").header("origin", s.root.as_str()).json(&json!({"name": "From the window"})).send().await.unwrap();
    assert_eq!(posted.status(), 200);
    let mcp = s.request(Method::POST, "/mcp").json(&json!({"jsonrpc": "2.0", "id": 1, "method": "ping"})).send().await.unwrap();
    assert_eq!(mcp.status(), 200);
}

#[tokio::test]
async fn a_preflight_is_answered_only_for_pages_of_this_computer() {
    let s = start().await;
    let preflight = |origin: &str| {
        s.request(Method::OPTIONS, "/api/v1/projects").header("origin", origin).header("access-control-request-method", "POST").header("access-control-request-headers", "content-type")
    };
    let foreign = preflight("https://evil.example").send().await.unwrap();
    assert_eq!(foreign.status(), 403);
    assert!(foreign.headers().get("access-control-allow-origin").is_none());
    let local = preflight("http://localhost:1420").send().await.unwrap();
    assert!(local.status().is_success(), "{}", local.status());
    assert_eq!(local.headers().get("access-control-allow-origin").and_then(|v| v.to_str().ok()), Some("http://localhost:1420"));
}
