//! First-run setup through the API: checks of ComfyUI, its nodes, folders and ffmpeg, and saving the settings.

use std::time::Duration;

use axum::routing::get;
use axum::{Json, Router};
use h3_comfy::REQUIRED_NODES;
use h3_project::AppConfig;
use reqwest::{Client, Method};
use serde_json::{json, Map, Value};

struct Server {
    base: String,
    client: Client,
    dir: tempfile::TempDir,
}

/// A ComfyUI stand-in that has every node but the ones named.
async fn comfy_without(missing: &'static [&'static str]) -> String {
    let app = Router::new()
        .route(
            "/system_stats",
            get(|| async { Json(json!({"system": {"os": "nt", "comfyui_version": "0.9.9", "argv": ["main.py"]}, "devices": [{"name": "Test GPU", "vram_total": 25769803776u64, "vram_free": 1}]})) }),
        )
        .route(
            "/object_info",
            get(move || async move {
                let mut nodes = Map::new();
                for r in REQUIRED_NODES.iter().filter(|r| !missing.contains(&r.class_type)) {
                    nodes.insert(r.class_type.to_string(), json!({}));
                }
                Json(Value::Object(nodes))
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

async fn start() -> Server {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("projects");
    let cfg = AppConfig { comfy_url: "http://127.0.0.1:9".into(), project_root: root, ..AppConfig::default() };
    let state = h3_api::build_state(cfg, &dir.path().join("jobs.db")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, h3_api::router(state)).await.unwrap() });
    Server { base: format!("http://{addr}/api/v1"), client: Client::builder().timeout(Duration::from_secs(60)).build().unwrap(), dir }
}

impl Server {
    async fn call(&self, method: Method, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut request = self.client.request(method, format!("{}{}", self.base, path));
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Value::Null))
    }
}

fn level<'a>(report: &'a Value, id: &str) -> &'a str {
    report["data"]["checks"].as_array().unwrap().iter().find(|c| c["id"] == id).unwrap_or_else(|| panic!("no check {id}: {report}"))["level"].as_str().unwrap()
}

#[tokio::test]
async fn a_first_run_with_no_comfyui_says_what_to_fix() {
    let s = start().await;
    let (code, report) = s.call(Method::GET, "/setup/status", None).await;
    assert_eq!(code, 200, "{report}");
    assert_eq!(report["data"]["first_run"], true);
    assert_eq!(report["data"]["ready"], false);
    assert_eq!((level(&report, "comfyui"), level(&report, "nodes"), level(&report, "output_folder")), ("fail", "skip", "skip"));
    assert_eq!(level(&report, "project_folder"), "ok", "a folder that can be created is fine");
    assert_eq!(level(&report, "ffmpeg"), "ok", "{report}");
    let comfy = report["data"]["checks"].as_array().unwrap().iter().find(|c| c["id"] == "comfyui").unwrap();
    assert!(comfy["fix"].as_str().unwrap().contains("Start ComfyUI"), "{comfy}");
}

#[tokio::test]
async fn settings_are_checked_before_they_are_saved() {
    let s = start().await;
    let comfy = comfy_without(&[]).await;
    let output = s.dir.path().join("comfy_output");
    std::fs::create_dir_all(&output).unwrap();
    let (code, report) = s.call(Method::POST, "/setup/check", Some(json!({"comfy_url": comfy, "comfy_output_dir": output.display().to_string()}))).await;
    assert_eq!(code, 200, "{report}");
    assert_eq!((level(&report, "comfyui"), level(&report, "nodes"), level(&report, "output_folder")), ("ok", "ok", "ok"), "{report}");
    assert!(report["data"]["checks"][0]["detail"].as_str().unwrap().contains("Test GPU"), "{report}");
    // Checking saves nothing.
    assert!(!s.dir.path().join("config.json").exists());

    let (code, _) = s.call(Method::POST, "/setup/check", Some(json!({"comfy_url": "localhost:8188"}))).await;
    assert_eq!(code, 400);
    let (code, _) = s.call(Method::POST, "/setup/check", Some(json!({"nonsense": "x"}))).await;
    assert_eq!(code, 400);
    let (code, _) = s.call(Method::POST, "/setup/check", Some(json!({"project_root": ""}))).await;
    assert_eq!(code, 400);
}

#[tokio::test]
async fn missing_node_packs_are_named_by_the_pack_that_provides_them() {
    let s = start().await;
    // A required one fails the check. An optional feature only warns.
    let core = comfy_without(&["VRGDG_MiniMaxH3AudioDrive", "VHS_VideoCombine"]).await;
    let (_, report) = s.call(Method::POST, "/setup/check", Some(json!({"comfy_url": core}))).await;
    assert_eq!(level(&report, "nodes"), "fail");
    let nodes = report["data"]["checks"].as_array().unwrap().iter().find(|c| c["id"] == "nodes").unwrap()["detail"].as_str().unwrap().to_string();
    assert!(nodes.contains("comfyui-vrgamedevgirl") && nodes.contains("VRGDG_MiniMaxH3AudioDrive") && nodes.contains("ComfyUI-VideoHelperSuite"), "{nodes}");
    assert_eq!(report["data"]["ready"], false);

    let optional = comfy_without(&["MMH3UltimateUpscale"]).await;
    let (_, report) = s.call(Method::POST, "/setup/check", Some(json!({"comfy_url": optional}))).await;
    assert_eq!(level(&report, "nodes"), "warn");
    let detail = report["data"]["checks"].as_array().unwrap().iter().find(|c| c["id"] == "nodes").unwrap()["detail"].as_str().unwrap().to_string();
    assert!(detail.contains("2 Pass Advanced"), "{detail}");
}

#[tokio::test]
async fn saved_settings_keep_the_llm_settings_and_end_the_first_run() {
    let s = start().await;
    let config = s.dir.path().join("config.json");
    // The LLM settings were saved by the LLM panel while the app ran.
    s.call(Method::PUT, "/llm/settings", Some(json!({"runner": "own_server", "own_server_url": "http://127.0.0.1:1/v1", "own_server_model": "m"}))).await;
    let (code, saved) = s.call(Method::PUT, "/setup/config", Some(json!({"comfy_url": "http://127.0.0.1:8199", "project_root": s.dir.path().join("elsewhere").display().to_string(), "ffmpeg_path": ""}))).await;
    assert_eq!(code, 200, "{saved}");
    assert_eq!((saved["data"]["saved"].clone(), saved["data"]["restart_required"].clone()), (json!(true), json!(true)));
    let on_disk = AppConfig::load(&config).unwrap();
    assert_eq!(on_disk.comfy_url, "http://127.0.0.1:8199");
    assert_eq!((on_disk.ffmpeg_path.as_str(), on_disk.llm.runner.as_str(), on_disk.llm.own_server_model.as_str()), ("ffmpeg", "own_server", "m"));
    let (_, report) = s.call(Method::GET, "/setup/status", None).await;
    assert_eq!(report["data"]["first_run"], false);

    let (code, _) = s.call(Method::PUT, "/setup/config", Some(json!({}))).await;
    assert_eq!(code, 400);
    let (code, _) = s.call(Method::PUT, "/setup/config", Some(json!({"comfy_url": "nope"}))).await;
    assert_eq!(code, 400);
    assert_eq!(AppConfig::load(&config).unwrap().comfy_url, "http://127.0.0.1:8199", "a refused change leaves the file alone");
}

#[tokio::test]
async fn a_project_folder_that_is_a_file_is_refused() {
    let s = start().await;
    let file = s.dir.path().join("not_a_folder");
    std::fs::write(&file, b"x").unwrap();
    let (_, report) = s.call(Method::POST, "/setup/check", Some(json!({"project_root": file.display().to_string()}))).await;
    assert_eq!(level(&report, "project_folder"), "fail");
}
