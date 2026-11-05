//! Clear memory: the app asks the pack inside ComfyUI to clean up, because the Whisper model the Builder keeps loaded between scenes (pack PR 319) lives
//! in that process and only the pack's own route frees it. A ComfyUI whose pack is older or missing gets the plain `/free` call.

use std::sync::{Arc, Mutex};

use axum::routing::post;
use axum::{Json, Router};
use h3_project::AppConfig;
use reqwest::Client;
use serde_json::{json, Value};

type Calls = Arc<Mutex<Vec<String>>>;

/// A ComfyUI stand-in that writes down each route it is asked on. With `pack` it also has the pack's cleanup route.
async fn comfy(pack: bool, calls: Calls) -> String {
    let free_calls = Arc::clone(&calls);
    let mut app = Router::new().route(
        "/free",
        post(move |Json(body): Json<Value>| {
            let calls = Arc::clone(&free_calls);
            async move {
                calls.lock().unwrap().push(format!("free {}", body["unload_models"]));
                Json(json!({}))
            }
        }),
    );
    if pack {
        let pack_calls = Arc::clone(&calls);
        app = app.route(
            "/vrgdg/music_builder/clear_memory_direct",
            post(move || {
                let calls = Arc::clone(&pack_calls);
                async move {
                    calls.lock().unwrap().push("pack".into());
                    Json(json!({"ok": true, "message": "Memory cleanup finished.\nWhisper models unloaded: 1", "gguf_models_unloaded": 0}))
                }
            }),
        );
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

async fn clear(pack: bool) -> (Value, Vec<String>) {
    let calls: Calls = Arc::default();
    let dir = tempfile::tempdir().unwrap();
    let cfg = AppConfig { comfy_url: comfy(pack, Arc::clone(&calls)).await, project_root: dir.path().join("projects"), ..AppConfig::default() };
    let state = h3_api::build_state(cfg, &dir.path().join("jobs.db")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, h3_api::router(state)).await.unwrap() });
    let response = Client::new().post(format!("http://{addr}/vrgdg/music_builder/clear_memory_direct")).json(&json!({})).send().await.unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let body: Value = response.json().await.unwrap();
    let seen = calls.lock().unwrap().clone();
    (body, seen)
}

#[tokio::test]
async fn the_pack_in_comfyui_does_the_cleanup_when_it_has_the_route() {
    let (body, calls) = clear(true).await;
    assert_eq!(calls, ["pack"], "the pack's route already frees the ComfyUI models");
    assert_eq!(body["ok"], true, "{body}");
    assert!(body["message"].as_str().unwrap().contains("Whisper models unloaded: 1"), "{body}");
}

#[tokio::test]
async fn an_older_pack_gets_the_plain_free_call() {
    let (body, calls) = clear(false).await;
    assert_eq!(calls, ["free true"]);
    assert_eq!(body["ok"], true, "{body}");
    assert!(body["message"].as_str().unwrap().contains("Memory cleanup finished"), "{body}");
}
