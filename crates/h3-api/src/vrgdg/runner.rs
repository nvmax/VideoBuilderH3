//! Render routes of the Builder (`/vrgdg/workflow_runner/...`): the ComfyUI graphs for a scene and the file work around a
//! render. The Builder runs the graphs itself, through ComfyUI's own routes, which this server passes on.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post as axum_post;
use axum::Router;
use h3_builder::runner::{self, ComfyFolders};
use h3_comfy::ComfyClient;
use h3_core::pyval::Obj;
use h3_media::Tools;
use h3_pipeline::env::LiveEnv;
use h3_project::Stamp;
use serde_json::Value;

use super::{post, Fail, Out};
use crate::app::AppState;

fn fields(result: Result<Value, String>) -> Result<Out, Fail> {
    result.map(Out::from).map_err(Fail::from)
}

pub fn routes(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    let base = "/vrgdg/workflow_runner";
    router
        .route(&format!("{base}/collect_scene_video"), post(|env, p| fields(runner::collect_video(&env, &p, &Stamp::now()))))
        .route(&format!("{base}/trim_scene_video"), post(|env, p| fields(runner::trim_video(&env, &p, &Stamp::now()))))
        .route(&format!("{base}/match_scene_video_start_color"), post(|env, p| fields(runner::match_start_color(&env, &p))))
        .route(&format!("{base}/find_scene_video_output"), post(|_, p| fields(runner::find_video_output(&p))))
        .route(&format!("{base}/find_minimax_h3_stage_outputs"), post(|_, p| fields(runner::find_stage_outputs(&p))))
        .route(&format!("{base}/collect_minimax_h3_stage_backup"), post(|env, p| fields(runner::stage_backup(&env, &p, &Stamp::now()))))
        .route(&format!("{base}/stitch_scene_videos"), post(|env, p| fields(runner::stitch_videos(&env, &p))))
        .route(&format!("{base}/render_image_slideshow"), post(|env, p| fields(runner::render_image_slideshow(&env, &p))))
        .route(&format!("{base}/prepare_scene_audio_clip"), post(|env, p| fields(runner::prepare_scene_audio_clip(&env, &p))))
        .route(&format!("{base}/cleanup_minimax_h3_output"), axum_post(cleanup_output))
        .route(&format!("{base}/save_image"), axum_post(save_image))
        .route(&format!("{base}/build_minimax_h3_prompt"), axum_post(|state, body| build(state, body, "minimax_h3", false)))
        .route(&format!("{base}/build_minimax_h3_2pass_prompt"), axum_post(|state, body| build(state, body, "minimax_h3_2pass", false)))
        .route(&format!("{base}/build_minimax_h3_advanced_2pass_prompt"), axum_post(|state, body| build(state, body, "minimax_h3_advanced_2pass", true)))
}

/// The request body as an object, or the reply to send back.
fn payload_of(body: &Bytes) -> Result<Obj, Box<Response>> {
    match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(map)) => Ok(map),
        _ => Err(Box::new(super::failure(StatusCode::BAD_REQUEST, "Invalid JSON body."))),
    }
}

/// ComfyUI's node list, folders and ffmpeg, as the graph builders ask for them.
async fn live_env(state: &AppState) -> Result<LiveEnv, String> {
    let info = state.object_info(false).await?;
    let dirs = h3_pipeline::context::comfy_dirs(&state.cfg).await?;
    let tools = Tools::find(&state.cfg.ffmpeg_path, &state.cfg.ffprobe_path).map_err(|e| e.to_string())?;
    let folder_paths = ComfyClient::new(state.cfg.comfy_base()).folder_paths().await;
    Ok(LiveEnv::new(info, dirs, tools).with_refmod_dirs(h3_pipeline::context::refmod_dirs(folder_paths.as_ref())))
}

/// Build the ComfyUI graph for one MiniMax H3 pass type. `debug_copy` also saves the graph beside the scene's outputs.
async fn build(State(state): State<Arc<AppState>>, body: Bytes, key: &'static str, debug_copy: bool) -> Response {
    let payload = match payload_of(&body) {
        Ok(payload) => payload,
        Err(response) => return *response,
    };
    let env = match live_env(&state).await {
        Ok(env) => env,
        Err(message) => return super::failure(StatusCode::BAD_REQUEST, message),
    };
    let built = tokio::task::spawn_blocking(move || -> Result<Obj, String> {
        let mut result = h3_graph::build(&env, key, &payload).map_err(|e| e.to_string())?;
        if debug_copy {
            let path = h3_builder::runner::save_advanced_debug_graph(&result, &payload)?;
            result.insert("debug_workflow_path".into(), Value::String(path));
        }
        Ok(result)
    })
    .await;
    match built {
        Ok(Ok(result)) => (StatusCode::OK, axum::Json(super::envelope(Value::Object(result)))).into_response(),
        Ok(Err(message)) => super::failure(StatusCode::BAD_REQUEST, message),
        Err(e) => super::failure(StatusCode::INTERNAL_SERVER_ERROR, format!("The request failed: {e}")),
    }
}

async fn cleanup_output(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let payload = match payload_of(&body) {
        Ok(payload) => payload,
        Err(response) => return *response,
    };
    let output = match h3_pipeline::context::comfy_dirs(&state.cfg).await {
        Ok(dirs) => dirs.output,
        Err(message) => return super::failure(StatusCode::BAD_REQUEST, message),
    };
    match tokio::task::spawn_blocking(move || runner::cleanup_output(&output, &payload)).await {
        Ok(result) => super::into_response(result.map(Out::from).map_err(Fail::from)),
        Err(e) => super::failure(StatusCode::INTERNAL_SERVER_ERROR, format!("The request failed: {e}")),
    }
}

async fn save_image(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let payload = match payload_of(&body) {
        Ok(payload) => payload,
        Err(response) => return *response,
    };
    let dirs = match h3_pipeline::context::comfy_dirs(&state.cfg).await {
        Ok(dirs) => dirs,
        Err(message) => return super::failure(StatusCode::BAD_REQUEST, message),
    };
    let folders = ComfyFolders { output: dirs.output.clone(), input: dirs.input.clone(), temp: dirs.temp.clone() };
    let default = std::path::PathBuf::from(&dirs.output);
    match tokio::task::spawn_blocking(move || runner::save_generated_image(&folders, &default, &payload)).await {
        Ok(result) => super::into_response(result.map(Out::from).map_err(Fail::from)),
        Err(e) => super::failure(StatusCode::INTERNAL_SERVER_ERROR, format!("The request failed: {e}")),
    }
}
