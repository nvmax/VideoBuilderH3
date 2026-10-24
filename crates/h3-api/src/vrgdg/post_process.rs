//! Post Process routes of the Builder: LUTs, film grain and Adjust.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Query, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use h3_builder::post_process as post_tools;
use serde_json::Value;
use tower::ServiceExt;
use tower_http::services::ServeFile;

use super::{get_query, post, Fail, Out};
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
