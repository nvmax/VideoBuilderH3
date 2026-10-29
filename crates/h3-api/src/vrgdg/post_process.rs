//! Post Process routes of the Builder: LUTs, film grain and Adjust.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Query, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use h3_builder::overlay as overlay_tools;
use h3_builder::overlay_library as overlay_library_tools;
use h3_builder::pack_import as pack_import_tools;
use h3_builder::post_process as post_tools;
use serde_json::Value;
use tower::ServiceExt;
use tower_http::services::ServeFile;

use super::{get_query, get_query_overlay, post, post_overlay, Fail, Out};
use crate::app::AppState;

fn fields(result: Result<Value, String>) -> Result<Out, Fail> {
    result.map(Out::from).map_err(Fail::from)
}

pub fn routes(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    let base = "/vrgdg/music_builder";
    router
        .route(&format!("{base}/luts"), get_query(|env, _| fields(post_tools::list_luts(&env))))
        .route(&format!("{base}/luts/example"), get(example))
        .route(&format!("{base}/luts/preview"), post(|env, p| fields(post_tools::preview_lut(&env, &p))))
        .route(&format!("{base}/luts/apply_video"), post(|env, p| fields(post_tools::apply_lut_video(&env, &p))))
        .route(&format!("{base}/luts/delete_preview"), post(|_, p| fields(post_tools::delete_preview(&p))))
        .route(&format!("{base}/post_process/adjust/preview"), post(|env, p| fields(post_tools::preview_adjust(&env, &p))))
        .route(&format!("{base}/post_process/adjust/apply_video"), post(|env, p| fields(post_tools::apply_adjust_video(&env, &p))))
        .route(&format!("{base}/post_process/film_grain/preview"), post(|env, p| fields(post_tools::preview_film_grain(&env, &p))))
        .route(&format!("{base}/post_process/film_grain/apply_video"), post(|env, p| fields(post_tools::apply_film_grain_video(&env, &p))))
        .route(&format!("{base}/overlay_layers/capabilities"), get_query(|env, _| fields(overlay_tools::capabilities(&env))))
        .route(&format!("{base}/overlay_layers/render"), post(|env, p| fields(overlay_tools::render(&env, &p))))
        .route(&format!("{base}/overlay_layers/preview_frame"), post(|env, p| fields(overlay_tools::preview_frame(&env, &p))))
        .route(&format!("{base}/overlay_layers/fonts"), get_query(|env, _| fields(overlay_tools::fonts(&env))))
        .route(&format!("{base}/overlay_layers/font"), get(overlay_font))
        .route(&format!("{base}/overlay_layers/lyric_captions"), post(|env, p| fields(overlay_tools::lyric_captions(&env, &p))))
        .route(&format!("{base}/overlay_library"), get_query_overlay(|env, _| fields(overlay_library_tools::list(&env))))
        .route(&format!("{base}/overlay_library/import"), post_overlay(|env, p| fields(overlay_library_tools::import(&env, &p))))
        .route(&format!("{base}/overlay_library/delete"), post_overlay(|env, p| fields(overlay_library_tools::delete(&env, &p))))
        .route(&format!("{base}/overlay_library/use"), post_overlay(|env, p| fields(overlay_library_tools::use_in_project(&env, &p))))
        .route(&format!("{base}/overlay_library/thumb"), get(overlay_thumb))
        .route(&format!("{base}/overlay_library/packs/scan"), post_overlay(|env, p| fields(pack_import_tools::scan(&env, &p))))
        .route(&format!("{base}/overlay_library/packs/import"), post_overlay(|env, p| fields(pack_import_tools::import(&env, &p))))
        .route(&format!("{base}/overlay_library/packs/image"), get(overlay_pack_image))
        .route(&format!("{base}/overlay_layers/preview"), get(overlay_preview))
}

/// The picture of a pack in a folder that is being imported from.
async fn overlay_pack_image(Query(query): Query<HashMap<String, String>>) -> Response {
    match pack_import_tools::image_path(query.get("root").map(String::as_str).unwrap_or(""), query.get("file").map(String::as_str).unwrap_or("")) {
        Ok(path) => {
            let request = Request::builder().method("GET").uri("/").body(Body::empty()).unwrap_or_default();
            match ServeFile::new(path).oneshot(request).await {
                Ok(response) => response.into_response(),
                Err(e) => super::failure(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
            }
        }
        Err(message) => super::failure(StatusCode::NOT_FOUND, message),
    }
}

/// The example picture shown beside a LUT.
async fn example(State(state): State<Arc<AppState>>, Query(query): Query<HashMap<String, String>>) -> Response {
    let env = state.builder_env();
    let name = query.get("name").cloned().unwrap_or_default();
    match post_tools::lut_example_path(&env, &name) {
        Ok(path) => {
            let request = Request::builder().method("GET").uri("/").body(Body::empty()).unwrap_or_default();
            match ServeFile::new(path).oneshot(request).await {
                Ok(response) => response.into_response(),
                Err(e) => super::failure(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
            }
        }
        Err(message) => super::failure(StatusCode::NOT_FOUND, message),
    }
}

/// A small playable copy of an overlay (library or project) for the live preview in the player. Range requests work, so the page can seek.
async fn overlay_preview(State(state): State<Arc<AppState>>, Query(query): Query<HashMap<String, String>>) -> Response {
    let env = state.builder_env();
    let (path, folder) = (query.get("path").cloned().unwrap_or_default(), query.get("project_folder").cloned().unwrap_or_default());
    let made = tokio::task::spawn_blocking(move || overlay_library_tools::preview_proxy(&env, &path, &folder)).await;
    match made {
        Ok(Ok(file)) => {
            let request = Request::builder().method("GET").uri("/").body(Body::empty()).unwrap_or_default();
            match ServeFile::new(file).oneshot(request).await {
                Ok(response) => response.into_response(),
                Err(e) => super::failure(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
            }
        }
        Ok(Err(message)) => super::failure(StatusCode::NOT_FOUND, message),
        Err(e) => super::failure(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// The still or the two second loop of an overlay library clip.
async fn overlay_thumb(State(state): State<Arc<AppState>>, Query(query): Query<HashMap<String, String>>) -> Response {
    let env = state.builder_env();
    let (id, kind) = (query.get("id").cloned().unwrap_or_default(), query.get("kind").cloned().unwrap_or_default());
    match overlay_library_tools::thumb_path(&env, &id, &kind) {
        Ok(path) => {
            let request = Request::builder().method("GET").uri("/").body(Body::empty()).unwrap_or_default();
            match ServeFile::new(path).oneshot(request).await {
                Ok(response) => response.into_response(),
                Err(e) => super::failure(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
            }
        }
        Err(message) => super::failure(StatusCode::NOT_FOUND, message),
    }
}

/// A font file a text clip names, for the live preview to load.
async fn overlay_font(State(state): State<Arc<AppState>>, Query(query): Query<HashMap<String, String>>) -> Response {
    let env = state.builder_env();
    match overlay_tools::font_path(&env, query.get("name").map(String::as_str).unwrap_or("")) {
        Ok(path) => {
            let request = Request::builder().method("GET").uri("/").body(Body::empty()).unwrap_or_default();
            match ServeFile::new(path).oneshot(request).await {
                Ok(response) => response.into_response(),
                Err(e) => super::failure(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
            }
        }
        Err(message) => super::failure(StatusCode::NOT_FOUND, message),
    }
}
