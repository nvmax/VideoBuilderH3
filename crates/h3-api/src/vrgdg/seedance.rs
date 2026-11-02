//! Seedance routes of the Builder (API Nodes video engine): the prompt backup and conversion, the scene's audio excerpt and the
//! ComfyUI graph. The Builder queues the graph itself, through ComfyUI's own `/prompt`, which this server passes on with the ComfyUI
//! API key saved in the app (see `setup_routes`). They answer like the pack's: 400 for anything that cannot be done.

use std::path::Path;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::post as axum_post;
use axum::Router;
use h3_builder::prompt_writers::seedance::convert_prompt;
use h3_builder::prompt_writers::RunnerLlm;
use h3_builder::seedance;
use serde_json::Value;

use super::llm::base_config;
use super::speaking::{done, redacted};
use super::{body_object, into_response, post, Fail, Out};
use crate::app::AppState;

fn fields(result: Result<Value, String>) -> Result<Out, Fail> {
    result.map(Out::from).map_err(Fail::from)
}

pub fn routes(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route("/vrgdg/music_builder/backup_seedance_prompts", post(|_, payload| fields(seedance::backup_prompts(&payload))))
        .route("/vrgdg/music_builder/convert_seedance_prompt", axum_post(convert))
        .route(
            "/vrgdg/workflow_runner/prepare_seedance_scene_audio",
            post(|env, payload| {
                let tools = env.tools().map_err(Fail::from)?;
                fields(seedance::prepare_scene_audio(&tools, &payload))
            }),
        )
        .route("/vrgdg/workflow_runner/build_seedance25_reference_prompt", axum_post(build_reference_prompt))
}

/// `POST /vrgdg/music_builder/convert_seedance_prompt`: lay one scene's saved prompt out for Seedance with the selected text LLM.
async fn convert(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let payload = match body_object(&body) {
        Ok(payload) => payload,
        Err(fail) => return into_response(Err(fail)),
    };
    let base = base_config(&state);
    let llm = RunnerLlm { payload: &payload, base: &base };
    done(convert_prompt(&llm, &payload).await.map_err(|message| redacted(message, &payload)))
}

/// `POST /vrgdg/workflow_runner/build_seedance25_reference_prompt`: the graph for ComfyUI's Seedance 2.5 node. It needs ComfyUI's node
/// list and its input folder (the references are copied there) and output folder.
async fn build_reference_prompt(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let payload = match body_object(&body) {
        Ok(payload) => payload,
        Err(fail) => return into_response(Err(fail)),
    };
    let info = match state.object_info(false).await {
        Ok(info) => info,
        Err(message) => return super::failure(StatusCode::BAD_REQUEST, message),
    };
    let dirs = match h3_pipeline::context::comfy_dirs(&state.cfg).await {
        Ok(dirs) => dirs,
        Err(message) => return super::failure(StatusCode::BAD_REQUEST, message),
    };
    let built = tokio::task::spawn_blocking(move || {
        let has_node = |class: &str| info.get(class).is_some();
        seedance::reference_prompt(&has_node, Path::new(&dirs.input), &dirs.output, &payload)
    })
    .await;
    match built {
        Ok(result) => done(result),
        Err(e) => super::failure(StatusCode::INTERNAL_SERVER_ERROR, format!("The request failed: {e}")),
    }
}
