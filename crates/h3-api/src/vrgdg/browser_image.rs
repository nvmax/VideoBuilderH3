//! The Image panel's Browser AI card (install, check, sign in, the workflow builder). It works with the nodes of the VRGDG node pack, which run
//! inside ComfyUI with Node.js and Playwright, and the pack's routes for them (`browser/nodes.py`) are served by ComfyUI too. These routes pass
//! the call on to ComfyUI, so they need the pack installed in the ComfyUI this app talks to.
//!
//! The Thumbnail dialog does not use them. It has its own routes (`thumbnail/browser_ai` and `thumbnail/browser_ai/open` in `runner.rs`), which
//! drive Chrome from this app with the `h3-browser` crate and need nothing installed.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;

use crate::app::AppState;

/// The one GET route: which browser, login and dependencies are ready.
const STATUS: &str = "/vrgdg/browser_image/status";

/// The POST routes: set up and sign in, the manual steps of the Image panel's Browser AI card, and the one that builds the ComfyUI graph with
/// the provider's node (`build_flow_gpt_image_prompt`, which the thumbnail dialog uses).
const POSTS: [&str; 10] = [
    "/vrgdg/browser_image/setup",
    "/vrgdg/browser_image/open_login",
    "/vrgdg/browser_image/manual_open",
    "/vrgdg/browser_image/manual_upload",
    "/vrgdg/browser_image/manual_submit",
    "/vrgdg/browser_image/manual_finish",
    "/vrgdg/browser_image/store_reference",
    "/vrgdg/browser_image/manual_wait_download",
    "/vrgdg/browser_image/manual_import_latest",
    "/vrgdg/workflow_runner/build_flow_gpt_image_prompt",
];

pub fn routes(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    POSTS.iter().fold(router.route(STATUS, get(forward)), |router, path| router.route(path, post(forward)))
}

/// Send the call to ComfyUI as it is. A 404 means ComfyUI does not have the pack's route, so the reply says what to install.
async fn forward(State(state): State<Arc<AppState>>, request: Request) -> Response {
    let path = request.uri().path().to_string();
    let reply = super::proxy_to_comfy(&state, request).await;
    if reply.status() == StatusCode::NOT_FOUND {
        return super::failure(StatusCode::NOT_IMPLEMENTED, format!("ComfyUI has no {path} route. Browser AI needs the VRGDG node pack installed in the ComfyUI this app uses. Install it and restart ComfyUI."));
    }
    reply
}
