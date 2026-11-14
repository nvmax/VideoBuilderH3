//! ComfyUI starts after the app: its output folder is found later, and the projects in it appear, with no restart.

use std::time::{Duration, Instant};

use axum::routing::get;
use axum::{Json, Router};
use h3_project::AppConfig;
use serde_json::{json, Value};

fn write_project(folder: &std::path::Path, name: &str) {
    let project = folder.join(name);
    std::fs::create_dir_all(&project).unwrap();
    let session = json!({"project_name": name, "revision": 1, "builder_save_revision": 1, "segments": []});
    std::fs::write(project.join("vrgdg_builder_session.json"), serde_json::to_string_pretty(&session).unwrap()).unwrap();
}

async fn project_ids(base: &str) -> Vec<String> {
    let body: Value = reqwest::get(format!("{base}/projects")).await.unwrap().json().await.unwrap();
    body["data"].as_array().unwrap().iter().filter_map(|p| p["id"].as_str().map(str::to_string)).collect()
}

#[tokio::test]
async fn comfyuis_output_folder_is_found_when_comfyui_starts_after_the_app() {
    let dir = tempfile::tempdir().unwrap();
    let projects = dir.path().join("projects");
    let output = dir.path().join("comfy_output");
    write_project(&projects, "Own");
    write_project(&output, "FromComfyOutput");

    // The address ComfyUI will have, with nothing listening on it yet.
    let reserved = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let comfy_port = reserved.local_addr().unwrap().port();
    drop(reserved);
    let cfg = AppConfig { comfy_url: format!("http://127.0.0.1:{comfy_port}"), project_root: projects, ..AppConfig::default() };

    let started = Instant::now();
    let state = h3_api::build_state(cfg, &dir.path().join("jobs.db")).unwrap();
    assert!(started.elapsed() < Duration::from_secs(3), "the app does not wait long for a ComfyUI that is not there: {:?}", started.elapsed());
    assert!(state.comfy_output().is_none());
    assert_eq!(state.roots().len(), 1);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/api/v1", listener.local_addr().unwrap());
    let served = std::sync::Arc::clone(&state);
    tokio::spawn(async move { h3_api::serve_on(listener, served).await.unwrap() });
    assert_eq!(project_ids(&base).await, ["Own"]);

    // ComfyUI starts.
    let main_py = dir.path().join("ComfyUI").join("main.py").display().to_string();
    let output_arg = output.display().to_string();
    let comfy = Router::new().route("/system_stats", get(move || async move { Json(json!({"system": {"argv": [main_py, "--output-directory", output_arg]}, "devices": []})) }));
    let comfy_listener = tokio::net::TcpListener::bind(("127.0.0.1", comfy_port)).await.unwrap();
    tokio::spawn(async move { axum::serve(comfy_listener, comfy).await.unwrap() });

    let waited = Instant::now();
    while state.comfy_output().is_none() && waited.elapsed() < Duration::from_secs(20) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(state.comfy_output().as_deref(), Some(output.as_path()), "found without a restart");
    assert_eq!(state.builder_env().output, output, "the Builder keeps its files there now");
    let mut ids = project_ids(&base).await;
    ids.sort();
    assert_eq!(ids, ["FromComfyOutput", "Own"], "the projects in ComfyUI's output folder are listed");
}
