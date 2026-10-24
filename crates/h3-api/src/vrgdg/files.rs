//! File, media and project log routes of the Builder.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Query, Request};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use h3_builder::{error_log, files, media, Env};
use h3_core::pyval::Obj;
use h3_project::Stamp;
use serde_json::{json, Value};
use tower::ServiceExt;
use tower_http::services::ServeFile;

use super::{get_query, post, Fail, Out};
use crate::app::AppState;

const AUDIO_TYPES: [&str; 6] = ["wav", "mp3", "flac", "m4a", "ogg", "mp4"];
const VIDEO_TYPES: [&str; 6] = ["mp4", "m4v", "mov", "webm", "mkv", "avi"];
const IMAGE_TYPES: [&str; 4] = ["png", "jpg", "jpeg", "webp"];

fn fields(result: Result<Value, String>) -> Result<Out, Fail> {
    result.map(Out::from).map_err(Fail::from)
}

fn raw(result: Result<Value, String>) -> Result<Out, Fail> {
    result.map(Out::Raw).map_err(Fail::from)
}

pub fn routes(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route("/vrgdg/music_builder/load_srt", post(|_, p| fields(files::load_srt(&p))))
        .route("/vrgdg/music_builder/load_prompt_json", post(|_, p| fields(files::load_prompt_json(&p))))
        .route("/vrgdg/music_builder/load_text_file", post(|_, p| fields(files::load_text_file(&p))))
        .route("/vrgdg/music_builder/save_text_file", post(|_, p| fields(files::save_text_file(&p))))
        .route("/vrgdg/music_builder/open_local_file", post(|_, p| fields(files::open_local_file(&p))))
        .route("/vrgdg/music_builder/pick_path", post(|_, p| fields(files::pick_path(&p))))
        .route("/vrgdg/music_builder/default_context_paths", get_query(|env, _| Ok(files::default_context_paths(&env).into())))
        .route("/vrgdg/music_builder/default_audio_srt_paths", get_query(|env, _| Ok(files::default_audio_srt_paths(&env).into())))
        .route("/vrgdg/music_builder/project_prompt_creator_paths", post(|_, p| fields(files::project_prompt_creator_paths(&p))))
        .route("/vrgdg/music_builder/import_latest_prompt_creator_outputs", post(|_, _| Err(prompt_creator_unavailable())))
        .route("/vrgdg/music_builder/copy_prompt_creator_outputs", post(|_, _| Err(prompt_creator_unavailable())))
        .route("/vrgdg/music_builder/analyze_audio", post(|env, p| fields(media::analyze_audio(&env, &p))))
        .route("/vrgdg/music_builder/latent_status", post(|_, p| raw(media::latent_status(&p))))
        .route("/vrgdg/music_builder/list_scene_latent_takes", post(|_, p| fields(media::list_scene_latent_takes(&p))))
        .route("/vrgdg/music_builder/check_latent_predecessor", post(|_, p| raw(media::check_latent_predecessor(&p))))
        .route("/vrgdg/music_builder/delete_scene_latent", post(|_, p| fields(media::delete_scene_latent(&p))))
        .route("/vrgdg/music_builder/list_dirty_latents", post(|_, p| fields(media::list_dirty_latents(&p))))
        .route("/vrgdg/music_builder/renumber_scenes_after_removal", post(|_, p| fields(media::renumber_after_removal(&p, &Stamp::now()))))
        .route("/vrgdg/music_builder/renumber_scenes_after_insert", post(|_, p| fields(media::renumber_after_insert(&p))))
        .route("/vrgdg/music_builder/scan_scene_videos", post(|env, p| fields(media::scan_scene_videos(&env, &p))))
        .route("/vrgdg/music_builder/restore_scene_video", post(|env, p| fields(media::restore_scene_video_route(&env, &p, &Stamp::now()))))
        .route("/vrgdg/music_builder/error_log", post(|_, p| fields(error_log::project_error_log(&p, false, true))))
        .route("/vrgdg/music_builder/error_log/clear", post(|_, p| fields(error_log::project_error_log(&p, true, true))))
        .route("/vrgdg/music_builder/error_log/append", post(append_error))
        .route("/vrgdg/music_builder/audio", get(audio_file))
        .route("/vrgdg/video_editor/video", get(video_file))
        .route("/vrgdg/video_editor/image", get(image_file))
}

/// The Prompt Creator is a separate set of ComfyUI nodes. This app has no such outputs to import.
fn prompt_creator_unavailable() -> Fail {
    Fail::from("Importing Prompt Creator outputs is not available in H3 Studio. Use Line Mapping and the Story Builder instead.")
}

/// Credentials in the request and in the saved model defaults are removed from the logged text.
fn append_error(env: Env, payload: Obj) -> Result<Out, Fail> {
    let mut secrets = error_log::secret_values(&Value::Object(payload.clone()));
    let defaults = h3_project::builder_store::model_defaults_path(&env.output);
    if let Some(saved) = std::fs::read_to_string(defaults).ok().and_then(|t| h3_core::pyjson::parse(&t).ok()) {
        secrets.extend(error_log::secret_values(&saved));
    }
    fields(error_log::append_project_error(&payload, &secrets))
}

fn not_found(message: &str) -> Response {
    (StatusCode::NOT_FOUND, axum::Json(json!({"ok": false, "error": message}))).into_response()
}

/// Send a file with range support. Only files with one of `allowed` extensions are sent, so these routes cannot be used
/// to read settings or keys.
async fn send(path: PathBuf, allowed: &[&str], headers: &HeaderMap, missing: &str, tune: impl FnOnce(&mut Response)) -> Response {
    let extension = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if !path.is_file() {
        return not_found(missing);
    }
    if !allowed.contains(&extension.as_str()) {
        return (StatusCode::BAD_REQUEST, axum::Json(json!({"ok": false, "error": "Unsupported file type."}))).into_response();
    }
    let mut request = Request::builder().method("GET").uri("/").body(Body::empty()).unwrap_or_default();
    for name in [header::RANGE, header::IF_RANGE, header::IF_MODIFIED_SINCE, header::IF_NONE_MATCH] {
        if let Some(value) = headers.get(&name) {
            request.headers_mut().insert(name, value.clone());
        }
    }
    match ServeFile::new(path).oneshot(request).await {
        Ok(response) => {
            let mut response = response.into_response();
            tune(&mut response);
            response
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, axum::Json(json!({"ok": false, "error": e.to_string()}))).into_response(),
    }
}

fn query_path(query: &HashMap<String, String>) -> PathBuf {
    let raw = query.get("path").map(|p| p.trim().to_string()).unwrap_or_default();
    std::path::absolute(&raw).unwrap_or_else(|_| PathBuf::from(raw)).components().collect()
}

async fn audio_file(Query(query): Query<HashMap<String, String>>, headers: HeaderMap) -> Response {
    send(query_path(&query), &AUDIO_TYPES, &headers, "Audio file was not found.", |_| {}).await
}

/// A clip, with the type a browser expects and a cache lifetime. The client versions the URL with the scene's
/// `video_cache_bust`, which only changes when the scene is rendered again, so one URL always means the same bytes.
async fn video_file(Query(query): Query<HashMap<String, String>>, headers: HeaderMap) -> Response {
    let path = query_path(&query);
    let mp4 = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("mp4") || e.eq_ignore_ascii_case("m4v"));
    let name = path.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let shown = if mp4 { format!("{name}.mp4") } else { path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default() };
    send(path, &VIDEO_TYPES, &headers, "Video file was not found.", move |response| {
        let headers = response.headers_mut();
        if mp4 {
            headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("video/mp4"));
        }
        if let Ok(value) = HeaderValue::from_str(&format!("inline; filename=\"{}\"", shown.replace('"', "_"))) {
            headers.insert(header::CONTENT_DISPOSITION, value);
        }
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=31536000, immutable"));
    })
    .await
}

async fn image_file(Query(query): Query<HashMap<String, String>>, headers: HeaderMap) -> Response {
    let versioned = query.get("thumbv").is_some_and(|v| !v.trim().is_empty());
    send(query_path(&query), &IMAGE_TYPES, &headers, "Image file was not found.", move |response| {
        if versioned {
            response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=31536000, immutable"));
        }
    })
    .await
}
