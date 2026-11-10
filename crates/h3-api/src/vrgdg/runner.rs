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
use h3_builder::{thumbnail, thumbnail_api_image, thumbnail_browser_ai};
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
        .route("/vrgdg/music_builder/thumbnail/save", axum_post(thumbnail_save))
        .route("/vrgdg/music_builder/thumbnail/api_image", axum_post(thumbnail_api_image))
        .route("/vrgdg/music_builder/thumbnail/browser_ai", axum_post(thumbnail_browser_ai_make))
        .route("/vrgdg/music_builder/thumbnail/browser_ai/open", axum_post(thumbnail_browser_ai_open))
        .route(&format!("{base}/build_minimax_h3_prompt"), axum_post(|state, body| build(state, body, Builder::H3("minimax_h3"), false)))
        .route(&format!("{base}/build_minimax_h3_2pass_prompt"), axum_post(|state, body| build(state, body, Builder::H3("minimax_h3_2pass"), false)))
        .route(&format!("{base}/build_minimax_h3_advanced_2pass_prompt"), axum_post(|state, body| build(state, body, Builder::H3("minimax_h3_advanced_2pass"), true)))
        .route(&format!("{base}/build_minimax_h3_draft_prompt"), axum_post(|state, body| build(state, body, Builder::Draft, false)))
        .route(&format!("{base}/build_minimax_h3_finalize_prompt"), axum_post(finalize))
        .route(&format!("{base}/build_clear_memory_prompt"), post(|_, _| fields(clear_memory())))
        .route(&format!("{base}/build_timestamped_transcribe_prompt"), post(|_, p| fields(timestamp_prompt(&p))))
        .route("/vrgdg/music_prompt_creator/build_whisper_prompt", axum_post(build_whisper))
        .route(&format!("{base}/build_zimage_prompt"), axum_post(|state, body| build(state, body, Builder::Image("zimage"), false)))
        .route(&format!("{base}/build_krea2_prompt"), axum_post(|state, body| build(state, body, Builder::Image("krea2"), false)))
        .route(&format!("{base}/build_krea2_2pass_prompt"), axum_post(|state, body| build(state, body, Builder::Image("krea2_2pass"), false)))
        .route(&format!("{base}/build_moss_image_prompt"), axum_post(|state, body| build(state, body, Builder::Image("moss_image"), false)))
        .route(&format!("{base}/build_qwen_image_edit_prompt"), axum_post(|state, body| build(state, body, Builder::Image("qwen_image_edit"), false)))
        .route(&format!("{base}/build_ernie_image_prompt"), axum_post(|state, body| build(state, body, Builder::Image("ernie_image"), false)))
        .route(&format!("{base}/build_flux_klein_prompt"), axum_post(|state, body| build(state, body, Builder::Image("flux_klein"), false)))
        .route(&format!("{base}/build_nb_image_prompt"), axum_post(|state, body| build(state, body, Builder::Image("nb_image"), false)))
        .route(&format!("{base}/build_z_upscale_enhance_prompt"), axum_post(|state, body| build(state, body, Builder::Image("z_upscale_enhance"), false)))
}

/// Which family of graphs a route builds.
#[derive(Clone, Copy)]
enum Builder {
    /// A MiniMax H3 video pass, by workflow key.
    H3(&'static str),
    /// The first pass of a two pass render alone, saved as a draft.
    Draft,
    /// A still image model, by key.
    Image(&'static str),
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

/// Build the ComfyUI graph for one pass type or image model. `debug_copy` also saves the graph beside the scene's outputs.
async fn build(State(state): State<Arc<AppState>>, body: Bytes, builder: Builder, debug_copy: bool) -> Response {
    let payload = match payload_of(&body) {
        Ok(payload) => payload,
        Err(response) => return *response,
    };
    let env = match live_env(&state).await {
        Ok(env) => env,
        Err(message) => return super::failure(StatusCode::BAD_REQUEST, message),
    };
    let built = tokio::task::spawn_blocking(move || -> Result<Obj, String> {
        let mut result = match builder {
            Builder::H3(key) => h3_graph::build(&env, key, &payload),
            Builder::Draft => {
                let key = if payload.get("draft_advanced").is_some_and(h3_core::pyval::truthy) { "minimax_h3_advanced_2pass" } else { "minimax_h3_2pass" };
                let full = h3_graph::build(&env, key, &payload).map_err(|e| e.to_string())?;
                return h3_builder::drafts::build_draft(&full, &payload).map(|v| v.as_object().cloned().unwrap_or_default());
            }
            Builder::Image(key) => h3_graph::image::build_image(&env, key, &payload),
        }
        .map_err(|e| e.to_string())?;
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

/// `POST /vrgdg/workflow_runner/build_minimax_h3_finalize_prompt`. A draft keeps the Turbo LoRA names it was made with, so
/// one whose LoRA was replaced by a newer release finishes with the installed one.
async fn finalize(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let payload = match super::body_object(&body) {
        Ok(payload) => payload,
        Err(fail) => return super::into_response(Err(fail)),
    };
    // Without ComfyUI's list the draft is finished as saved, and ComfyUI reports a missing file itself.
    let loras = state.object_info(false).await.map(|info| h3_comfy::models::model_choices(&info, "loras")).unwrap_or_default();
    let reply = tokio::task::spawn_blocking(move || {
        let mut result = h3_builder::drafts::build_finalize(&payload)?;
        if let Some(result) = result.as_object_mut() {
            h3_graph::drafts::replace_missing_turbo_loras(result, &loras);
        }
        Ok(result)
    })
    .await
    .unwrap_or_else(|e| Err(format!("The request failed: {e}")));
    super::into_response(fields(reply))
}

/// `POST /vrgdg/workflow_runner/build_clear_memory_prompt`.
fn clear_memory() -> Result<Value, String> {
    let (name, prompt) = h3_graph::utility::clear_memory_graph().map_err(|e| e.to_string())?;
    Ok(serde_json::json!({"workflow_path": name, "prompt": Value::Object(prompt)}))
}

/// `POST /vrgdg/workflow_runner/build_timestamped_transcribe_prompt`.
fn timestamp_prompt(payload: &Obj) -> Result<Value, String> {
    let prompt = h3_graph::timestamp::timestamp_graph(payload).map_err(|e| e.to_string())?;
    Ok(serde_json::json!({"workflow_path": "lyrics_timestamp_api.json", "prompt": Value::Object(prompt)}))
}

/// `POST /vrgdg/music_prompt_creator/build_whisper_prompt`: needs ComfyUI's input folder to put the song in.
async fn build_whisper(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let payload = match payload_of(&body) {
        Ok(payload) => payload,
        Err(response) => return *response,
    };
    let input = match h3_pipeline::context::comfy_dirs(&state.cfg).await {
        Ok(dirs) => std::path::PathBuf::from(dirs.input),
        Err(message) => return super::failure(StatusCode::BAD_REQUEST, message),
    };
    let env = state.builder_env();
    match tokio::task::spawn_blocking(move || h3_builder::transcribe::build_whisper_prompt(&env, &input, &payload, &Stamp::now())).await {
        Ok(result) => super::into_response(result.map(Out::from).map_err(Fail::from)),
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

/// `POST /vrgdg/music_builder/thumbnail/save`: keep a picture ComfyUI made as the next `thumbnail_NNNN.png` of the project. It needs ComfyUI's
/// folders, like `save_image`, so it is served here.
async fn thumbnail_save(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let payload = match payload_of(&body) {
        Ok(payload) => payload,
        Err(response) => return *response,
    };
    let dirs = match h3_pipeline::context::comfy_dirs(&state.cfg).await {
        Ok(dirs) => dirs,
        Err(message) => return super::failure(StatusCode::BAD_REQUEST, message),
    };
    let folders = ComfyFolders { output: dirs.output.clone(), input: dirs.input.clone(), temp: dirs.temp.clone() };
    match tokio::task::spawn_blocking(move || thumbnail::save_thumbnail_image(&folders, &payload)).await {
        Ok(result) => super::into_response(result.map(Out::from).map_err(Fail::from)),
        Err(e) => super::failure(StatusCode::INTERNAL_SERVER_ERROR, format!("The request failed: {e}")),
    }
}

/// `POST /vrgdg/music_builder/thumbnail/api_image`: the chosen frame edited by Gemini, Grok or OpenAI, with the provider and the key from the
/// LLM Runner. It keeps the picture as the next `thumbnail_NNNN.png`. The call to the provider can take minutes, so it is awaited here and
/// holds no lock.
async fn thumbnail_api_image(body: Bytes) -> Response {
    let payload = match payload_of(&body) {
        Ok(payload) => payload,
        Err(response) => return *response,
    };
    super::into_response(thumbnail_api_image::make_thumbnail_api_image(&payload).await.map(Out::from).map_err(Fail::from))
}

/// `POST /vrgdg/music_builder/thumbnail/browser_ai`: the chosen frame edited by Flow, GPT Image or Meta AI in the person's Chrome, driven by this
/// app (no ComfyUI, no Node.js, no Playwright). It keeps the picture as the next `thumbnail_NNNN.png`. The site can take many minutes, so the
/// call is awaited here and holds no lock.
async fn thumbnail_browser_ai_make(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let payload = match payload_of(&body) {
        Ok(payload) => payload,
        Err(response) => return *response,
    };
    let work = state.browser_ai_dir().join("downloads");
    super::into_response(thumbnail_browser_ai::make_thumbnail_browser_ai(&payload, &work).await.map(Out::from).map_err(Fail::from))
}

/// `POST /vrgdg/music_builder/thumbnail/browser_ai/open`: open the provider's site in its own Chrome window (its own profile, so the sign-in
/// lasts) for the person to sign in. Returns once the window is up.
async fn thumbnail_browser_ai_open(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let payload = match payload_of(&body) {
        Ok(payload) => payload,
        Err(response) => return *response,
    };
    super::into_response(thumbnail_browser_ai::open_thumbnail_browser_ai(&payload, &state.browser_ai_dir()).await.map(Out::from).map_err(Fail::from))
}
