//! Audio Mask routes of the Builder (Hear Stems, Show Stems). Splitting a scene's audio runs in ComfyUI, with the pack's
//! Get Stems node, so the song's own Demucs install does the work.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post as axum_post;
use axum::Router;
use h3_builder::audio_mask::{self, Separated};
use h3_builder::Env;
use h3_comfy::ComfyClient;
use serde_json::{json, Value};

use super::{post, Fail, Out};
use crate::app::AppState;

fn raw(result: Result<Value, String>) -> Result<Out, Fail> {
    result.map(Out::Raw).map_err(Fail::from)
}

pub fn routes(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route("/vrgdg/music_builder/audio_mask/state", post(|_, p| raw(audio_mask::scene_state(&p))))
        .route("/vrgdg/music_builder/audio_mask/states", post(|_, p| raw(audio_mask::scene_states(&p))))
        .route("/vrgdg/music_builder/audio_mask/render", post(|_, p| raw(audio_mask::render_masked_mix(&p))))
        .route("/vrgdg/music_builder/audio_mask/list", post(|_, p| raw(audio_mask::list_scene_stems(&p))))
        .route("/vrgdg/music_builder/audio_mask/delete", post(|_, p| raw(audio_mask::delete_scene_stems(&p))))
        .route("/vrgdg/music_builder/audio_mask/separate", axum_post(separate))
}

async fn separate(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let payload = match serde_json::from_slice::<Value>(&body) {
        Ok(Value::Object(map)) => map,
        _ if body.iter().all(u8::is_ascii_whitespace) => serde_json::Map::new(),
        _ => return super::failure(StatusCode::BAD_REQUEST, "The request body must be a JSON object."),
    };
    let env = state.builder_env();
    let base = state.cfg.comfy_base().to_string();
    let handle = tokio::runtime::Handle::current();
    let reply = tokio::task::spawn_blocking(move || {
        let separator = |context: &Path, model: &str, device: &str| handle.block_on(demucs_in_comfy(&base, &env, context, model, device));
        audio_mask::separate_scene_stems(&env, &payload, &separator)
    })
    .await;
    match reply {
        Ok(Ok(value)) => axum::Json(value).into_response(),
        Ok(Err(message)) => super::failure(StatusCode::BAD_REQUEST, message),
        Err(e) => super::failure(StatusCode::INTERNAL_SERVER_ERROR, format!("The request failed: {e}")),
    }
}

/// The names of the stems the Get Stems node returns, in the order of its outputs.
const NODE_STEMS: [&str; 4] = ["vocals", "drums", "bass", "other"];

/// Split the audio file with ComfyUI's Get Stems node and read the four stems back. The node has no 6 stem model, so that
/// choice is split with `htdemucs`, which gives the same vocals. The result says which model was used.
async fn demucs_in_comfy(base: &str, env: &Env, context: &Path, requested: &str, device: &str) -> Result<Separated, String> {
    let model = if audio_mask::stems_for_model(requested).len() == NODE_STEMS.len() { requested } else { "htdemucs" };
    if model != requested {
        eprintln!("[VRGDG-RS Audio Mask] ComfyUI's Get Stems node has no {requested} model. Splitting with {model}, which gives the same vocals.");
    }
    let client = ComfyClient::new(base);
    for class in ["VRGDG_GetStems", "PreviewAudio"] {
        match client.object_info_for(class).await {
            Ok(info) if info.get(class).is_some() => {}
            Ok(_) => return Err(format!("ComfyUI does not have the {class} node. Install the VRGDG node pack and restart ComfyUI.")),
            Err(e) => return Err(e.to_string()),
        }
    }
    let mut graph = serde_json::Map::new();
    graph.insert("1".into(), json!({"class_type": "VRGDG_GetStems", "inputs": {"model_name": model, "device": device, "audio_file_path": context.display().to_string()}}));
    for (index, _) in NODE_STEMS.iter().enumerate() {
        graph.insert((index + 2).to_string(), json!({"class_type": "PreviewAudio", "inputs": {"audio": ["1", index]}}));
    }
    eprintln!("[VRGDG-RS Audio Mask] Splitting the scene audio with {model} on {device} in ComfyUI.");
    let queued = client.queue_prompt(&Value::Object(graph)).await.map_err(|e| format!("ComfyUI did not accept the split: {e}"))?;
    let cancel = Arc::new(AtomicBool::new(false));
    let history = client.wait_for_prompt(&queued.prompt_id, Duration::from_secs(1800), Duration::from_secs(1), &cancel, |_| {}).await.map_err(|e| format!("The split in ComfyUI failed: {e}"))?;
    let entry = history.get(&queued.prompt_id).unwrap_or(&history);
    let folder = context.parent().unwrap_or(Path::new("."));
    let http = reqwest::Client::new();
    let mut stems = Vec::new();
    for (index, name) in NODE_STEMS.iter().enumerate() {
        let file = entry
            .get("outputs")
            .and_then(|o| o.get((index + 2).to_string()))
            .and_then(|o| o.get("audio"))
            .and_then(|a| a.get(0))
            .ok_or_else(|| format!("ComfyUI did not return the {name} stem."))?;
        let field = |key: &str| file.get(key).and_then(Value::as_str).unwrap_or("").to_string();
        let response = http
            .get(format!("{base}/view"))
            .query(&[("filename", field("filename")), ("subfolder", field("subfolder")), ("type", field("type"))])
            .send()
            .await
            .map_err(|e| format!("Could not read the {name} stem from ComfyUI: {e}"))?;
        if !response.status().is_success() {
            return Err(format!("ComfyUI could not send the {name} stem ({}).", response.status()));
        }
        let bytes = response.bytes().await.map_err(|e| e.to_string())?;
        let temporary = folder.join(format!("_stem_{name}.flac"));
        std::fs::write(&temporary, &bytes).map_err(|e| format!("{}: {e}", temporary.display()))?;
        let decoded = audio_mask::decode_audio(env, &temporary).map_err(|e| format!("Could not read the {name} stem: {e}"));
        let _ = std::fs::remove_file(&temporary);
        stems.push(((*name).to_string(), decoded?));
    }
    Ok(Separated { model: model.to_string(), stems })
}
