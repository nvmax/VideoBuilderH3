//! Model lists the Builder offers in its dropdowns. ComfyUI knows the files, so they come from its loader nodes.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use h3_comfy::models::{combo_choices, model_choices};
use h3_comfy::ComfyClient;
use serde_json::{json, Value};

use crate::app::AppState;

/// The entry the Builder shows for "no LoRA".
const NONE_LORA: &str = "[none]";

pub fn routes(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route("/vrgdg/workflow_runner/lora_list", get(lora_list))
        .route("/vrgdg/workflow_runner/i2v_choices", get(i2v_choices))
        .route("/vrgdg/music_builder/gemma_choices", get(gemma_choices))
}

/// The loader nodes' inputs, as one `object_info` map. A node ComfyUI does not have is left out.
async fn loader_info(state: &AppState, classes: &[&str]) -> Result<Value, Box<Response>> {
    let client = ComfyClient::new(state.cfg.comfy_base());
    let mut info = serde_json::Map::new();
    for class in classes {
        match client.object_info_for(class).await {
            Ok(Value::Object(found)) => info.extend(found),
            Ok(_) => {}
            Err(e) => return Err(Box::new(super::failure(StatusCode::BAD_REQUEST, e.to_string()))),
        }
    }
    Ok(Value::Object(info))
}

fn unique(lists: &[Vec<String>]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in lists.iter().flatten().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        if !out.contains(&item) {
            out.push(item);
        }
    }
    out
}

async fn lora_list(State(state): State<Arc<AppState>>) -> Response {
    match loader_info(&state, &["LoraLoaderModelOnly"]).await {
        Ok(info) => {
            let loras: Vec<String> = std::iter::once(NONE_LORA.to_string()).chain(model_choices(&info, "loras").into_iter().filter(|n| n.trim() != NONE_LORA)).collect();
            Json(json!({"ok": true, "loras": loras})).into_response()
        }
        Err(response) => *response,
    }
}

/// The files ComfyUI lists for a model folder (`GET /models/{folder}`). A folder ComfyUI does not know is empty.
async fn model_files(state: &AppState, folder: &str) -> Vec<String> {
    let url = format!("{}/models/{folder}", state.cfg.comfy_base());
    let Ok(response) = reqwest::get(&url).await else { return Vec::new() };
    response.json::<Vec<String>>().await.unwrap_or_default()
}

async fn i2v_choices(State(state): State<Arc<AppState>>) -> Response {
    // The pack lists the diffusion models first and the GGUF ones after them, sorted without regard to case.
    let mut gguf = model_files(&state, "unet_gguf").await;
    gguf.sort_by_key(|name| name.to_lowercase());
    let unets = unique(&[model_files(&state, "diffusion_models").await, gguf]);
    let (gguf, diffusion): (Vec<String>, Vec<String>) = unets.iter().cloned().partition(|n| n.to_lowercase().ends_with(".gguf"));
    Json(json!({
        "ok": true,
        "unets": unets,
        "video_gguf_unets": gguf,
        "video_diffusion_models": diffusion,
        "vae": unique(&[model_files(&state, "vae").await]),
        "clip": unique(&[model_files(&state, "text_encoders").await]),
        "upscale_models": unique(&[model_files(&state, "latent_upscale_models").await]),
    }))
    .into_response()
}

/// GGUF files for the local Gemma and Qwen runners. The nodes list them from `models/LLM`, so ComfyUI's lists are the
/// pack's lists.
async fn gemma_choices(State(state): State<Arc<AppState>>) -> Response {
    match loader_info(&state, &["VRGDG_SuperGemmaGGUFChat", "VRGDG_QwenGGUF"]).await {
        Ok(info) => Json(json!({
            "ok": true,
            "models": combo_choices(&info, "VRGDG_SuperGemmaGGUFChat", "model_file"),
            "mmproj": combo_choices(&info, "VRGDG_SuperGemmaGGUFChat", "mmproj_file"),
            "qwen_models": combo_choices(&info, "VRGDG_QwenGGUF", "model_file"),
            "qwen_mmproj": combo_choices(&info, "VRGDG_QwenGGUF", "mmproj_file"),
        }))
        .into_response(),
        Err(response) => *response,
    }
}
