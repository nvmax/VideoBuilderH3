//! The GPU list of the LLM Runner: the app asks the pack inside ComfyUI (only that process knows the GPU numbers a local model can use), adds the answer to the
//! model choices, and says nothing when ComfyUI's pack is too old to know.

use axum::routing::get;
use axum::{Json, Router};
use h3_project::AppConfig;
use reqwest::Client;
use serde_json::{json, Value};

fn node(class: &str) -> Value {
    json!({class: {"input": {"required": {"model_file": [["gemma.gguf"], {}], "mmproj_file": [["mmproj.gguf"], {}]}}}})
}

/// A ComfyUI stand-in with the two GGUF nodes, and the pack's GPU list when `with_gpus`.
async fn comfy(with_gpus: bool) -> String {
    let gemma = node("VRGDG_SuperGemmaGGUFChat");
    let qwen = node("VRGDG_QwenGGUF");
    let mut app = Router::new()
        .route("/object_info/VRGDG_SuperGemmaGGUFChat", get(move || async move { Json(gemma) }))
        .route("/object_info/VRGDG_QwenGGUF", get(move || async move { Json(qwen) }));
    if with_gpus {
        let listing = json!({
            "ok": true, "visible_count": 1, "comfyui_gpu": 0, "cuda_visible_devices": "1",
            "gpus": [
                {"number": 0, "name": "NVIDIA GeForce RTX 5090", "total_gb": 31.8, "free_gb": 25.4, "slot": 0, "visible": true, "comfyui_gpu": true, "label": "GPU 0: NVIDIA GeForce RTX 5090"},
                {"number": null, "name": "NVIDIA GeForce RTX 4090", "total_gb": 24.0, "free_gb": 15.0, "slot": 1, "visible": false, "comfyui_gpu": false, "label": "NVIDIA GeForce RTX 4090 - not visible to ComfyUI"},
            ],
            "note": "ComfyUI can only see 1 of the 2 GPUs.",
        });
        app = app.route("/vrgdg/music_builder/llm_gpus", get(move || async move { Json(listing) }));
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

async fn serve(comfy_url: String) -> (String, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let cfg = AppConfig { comfy_url, project_root: dir.path().join("projects"), ..AppConfig::default() };
    let state = h3_api::build_state(cfg, &dir.path().join("jobs.db")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, h3_api::router(state)).await.unwrap() });
    (format!("http://{addr}"), dir)
}

async fn get_json(base: &str, path: &str) -> (u16, Value) {
    let response = Client::new().get(format!("{base}{path}")).send().await.unwrap();
    let status = response.status().as_u16();
    (status, response.json().await.unwrap())
}

#[tokio::test]
async fn the_model_choices_carry_the_gpu_list_of_comfyui() {
    let (base, _dir) = serve(comfy(true).await).await;
    let (status, choices) = get_json(&base, "/vrgdg/music_builder/gemma_choices").await;
    assert_eq!(status, 200, "{choices}");
    assert_eq!(choices["models"], json!(["gemma.gguf"]));
    assert_eq!(choices["qwen_models"], json!(["gemma.gguf"]));
    assert_eq!(choices["gpus"]["gpus"][0]["name"], "NVIDIA GeForce RTX 5090");
    assert_eq!(choices["gpus"]["gpus"][1]["visible"], false, "a card ComfyUI cannot see is listed, with the reason");
    assert_eq!(choices["gpus"]["note"], "ComfyUI can only see 1 of the 2 GPUs.");
    assert!(choices["gpus"].get("ok").is_none(), "the pack's own ok flag is not repeated inside the list");
}

#[tokio::test]
async fn the_gpu_list_is_also_a_route_of_its_own() {
    let (base, _dir) = serve(comfy(true).await).await;
    let (status, listing) = get_json(&base, "/vrgdg/music_builder/llm_gpus").await;
    assert_eq!(status, 200, "{listing}");
    assert_eq!(listing["ok"], true);
    assert_eq!(listing["gpus"].as_array().map(Vec::len), Some(2));
    assert_eq!(listing["comfyui_gpu"], 0);
}

#[tokio::test]
async fn a_comfyui_whose_pack_cannot_list_gpus_gives_no_list_and_the_models_still_come() {
    let (base, _dir) = serve(comfy(false).await).await;
    let (status, choices) = get_json(&base, "/vrgdg/music_builder/gemma_choices").await;
    assert_eq!(status, 200, "{choices}");
    assert_eq!(choices["models"], json!(["gemma.gguf"]));
    assert!(choices["gpus"].is_null(), "no list is a null, which the Builder reads as 'update the pack': {choices}");
    let (status, refused) = get_json(&base, "/vrgdg/music_builder/llm_gpus").await;
    assert_eq!(status, 400);
    assert_eq!(refused["ok"], false);
    assert!(refused["error"].as_str().unwrap_or_default().contains("GPUs"), "{refused}");
}
