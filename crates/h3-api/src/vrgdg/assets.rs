//! Routes that save pictures and audio into a project. Some name a picture ComfyUI made, so they need ComfyUI's folders.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post as axum_post;
use axum::Router;
use h3_builder::assets;
use h3_builder::runner::ComfyFolders;
use h3_builder::Env;
use h3_core::pyval::Obj;
use h3_project::Stamp;
use serde_json::Value;

use super::{post, Fail, Out};
use crate::app::AppState;

fn fields(result: Result<Value, String>) -> Result<Out, Fail> {
    result.map(Out::from).map_err(Fail::from)
}

fn raw(result: Result<Value, String>) -> Result<Out, Fail> {
    result.map(Out::Raw).map_err(Fail::from)
}

pub fn routes(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    let base = "/vrgdg/music_builder";
    router
        .route(&format!("{base}/save_scene_image"), axum_post(|state, body| with_comfy(state, body, |_, payload, comfy| assets::save_scene_image(&payload, comfy.as_ref()))))
        .route(&format!("{base}/archive_scene_image"), axum_post(|state, body| with_comfy(state, body, |_, payload, comfy| assets::archive_scene_image(&payload, comfy.as_ref(), &Stamp::now()))))
        .route(&format!("{base}/save_flux_reference_image"), axum_post(|state, body| with_comfy(state, body, |_, payload, comfy| assets::save_flux_reference_image(&payload, comfy.as_ref()))))
        .route(&format!("{base}/delete_project_media"), post(|_, p| fields(assets::delete_project_media(&p))))
        .route(&format!("{base}/extract_video_final_frame"), post(|env, p| fields(assets::extract_video_final_frame(&env, &p, &Stamp::now()))))
        .route(&format!("{base}/import_reference_subjects"), post(|_, p| fields(assets::import_references(&p, "subject"))))
        .route(&format!("{base}/import_reference_locations"), post(|_, p| fields(assets::import_references(&p, "location"))))
        .route(&format!("{base}/save_scene_audio"), post(|env, p| fields(assets::save_scene_audio(&env, &p))))
        .route(&format!("{base}/save_project_audio"), post(|env, p| fields(assets::save_project_audio(&env, &p))))
        .route(&format!("{base}/save_project_srt"), post(|_, p| fields(assets::save_project_srt(&p))))
        .route(&format!("{base}/save_single_scene_srt"), post(|_, p| fields(assets::save_single_scene_srt(&p))))
        .route(&format!("{base}/trim_scene_audio"), post(|env, p| fields(assets::trim_scene_audio(&env, &p))))
        .route(&format!("{base}/create_silent_audio"), post(|env, p| raw(assets::create_silent_audio(&env, &p))))
        .route(&format!("{base}/prepare_scene_audio_mix"), post(|env, p| fields(assets::prepare_scene_audio_mix(&env, &p))))
        .route(&format!("{base}/prepare_audio_clip_mix"), post(|env, p| fields(assets::prepare_audio_clip_mix(&env, &p))))
}

/// Run a save that may name a ComfyUI picture. ComfyUI's folders are looked up only when the request names one.
async fn with_comfy(State(state): State<Arc<AppState>>, body: Bytes, run: fn(Env, Obj, Option<ComfyFolders>) -> Result<Value, String>) -> Response {
    let payload = match serde_json::from_slice::<Value>(&body) {
        Ok(Value::Object(map)) => map,
        _ => return super::failure(StatusCode::BAD_REQUEST, "Invalid JSON body."),
    };
    let comfy = if payload.get("image").is_some_and(Value::is_object) {
        h3_pipeline::context::comfy_dirs(&state.cfg).await.ok().map(|d| ComfyFolders { output: d.output, input: d.input, temp: d.temp })
    } else {
        None
    };
    let env = state.builder_env();
    match tokio::task::spawn_blocking(move || run(env, payload, comfy)).await {
        Ok(result) => super::into_response(result.map(Out::from).map_err(Fail::from)),
        Err(e) => super::failure(StatusCode::INTERNAL_SERVER_ERROR, format!("The request failed: {e}")).into_response(),
    }
}
