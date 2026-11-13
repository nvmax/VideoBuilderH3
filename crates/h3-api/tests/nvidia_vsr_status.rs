//! How the app learns whether `nvidia-vfx` (RTX Video Super Resolution) is installed in ComfyUI: from the custom node check of the VRGDG node pack
//! inside ComfyUI. Any other answer means "not known", and a render is then left as it is instead of being changed on a guess.

use axum::routing::get;
use axum::{Json, Router};
use h3_comfy::ComfyClient;
use h3_pipeline::context::nvidia_vsr_installed;
use serde_json::{json, Value};

/// A stand-in ComfyUI that answers the pack's custom node check with `status`, or has no such route (404) without one.
async fn comfy(status: Option<Value>) -> ComfyClient {
    let mut app: Router = Router::new();
    if let Some(status) = status {
        app = app.route("/vrgdg/video_builder/custom_nodes/status", get(move || {
            let status = status.clone();
            async move { Json(status) }
        }));
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    ComfyClient::new(&url)
}

fn status(vfx: Option<bool>) -> Value {
    let mut nodes = vec![json!({"id": "gguf", "installed": true, "source": "registry"})];
    if let Some(installed) = vfx {
        nodes.push(json!({"id": "nvidia_vfx", "installed": installed, "source": "pip", "install": "nvidia-vfx"}));
    }
    json!({"ok": true, "manager_available": true, "nodes": nodes})
}

#[tokio::test]
async fn the_pack_says_the_library_is_installed() {
    let client = comfy(Some(status(Some(true)))).await;
    assert_eq!(client.nvidia_vsr_installed().await, Some(true));
    assert!(nvidia_vsr_installed(&client).await);
}

#[tokio::test]
async fn the_pack_says_the_library_is_missing() {
    let client = comfy(Some(status(Some(false)))).await;
    assert_eq!(client.nvidia_vsr_installed().await, Some(false));
    assert!(!nvidia_vsr_installed(&client).await);
}

#[tokio::test]
async fn a_pack_without_the_entry_does_not_say_so_the_render_is_left_alone() {
    let client = comfy(Some(status(None))).await;
    assert_eq!(client.nvidia_vsr_installed().await, None);
    assert!(nvidia_vsr_installed(&client).await);
}

#[tokio::test]
async fn no_pack_or_no_comfyui_leaves_the_render_alone() {
    let without_pack = comfy(None).await;
    assert_eq!(without_pack.nvidia_vsr_installed().await, None);
    assert!(nvidia_vsr_installed(&without_pack).await);
    assert!(nvidia_vsr_installed(&ComfyClient::new("http://127.0.0.1:9")).await);
}
