//! Storyboard routes of the Builder: the Storyboard file (`load`, `save`, `export_prompts`, `import_reference_image`). The story steps that
//! need an LLM are in `story.rs`.

use std::sync::Arc;

use axum::http::StatusCode;
use axum::Router;
use h3_project::session::with_save_lock;
use h3_project::storyboard::{export_prompts, import_reference_image, load_storyboard, save_storyboard, StoryboardError};
use h3_project::Stamp;
use serde_json::json;

use super::{post, Fail, Out};
use crate::app::AppState;

fn internal(message: impl Into<String>) -> Fail {
    Fail::new(StatusCode::INTERNAL_SERVER_ERROR, message)
}

/// A save that found another revision on disk answers 409 with the revision the page should refresh from.
fn failed(error: StoryboardError) -> Fail {
    match error {
        StoryboardError::Conflict { current_revision, message } => Fail { status: StatusCode::CONFLICT, body: json!({"ok": false, "conflict": true, "error": message, "current_revision": current_revision}) },
        StoryboardError::Failed(message) => internal(message),
    }
}

pub fn routes(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route("/vrgdg/storyboard/load", post(|_, p| load_storyboard(&p, &Stamp::now()).map(|s| Out::Fields(json!({"storyboard": s}))).map_err(internal)))
        .route("/vrgdg/storyboard/save", post(|_, p| with_save_lock(|| save_storyboard(&p, &Stamp::now())).map(|s| Out::Fields(json!({"storyboard": s}))).map_err(failed)))
        .route("/vrgdg/storyboard/import_reference_image", post(|_, p| import_reference_image(&p, &Stamp::now()).map(Out::Fields).map_err(internal)))
        .route("/vrgdg/storyboard/export_prompts", post(|_, p| with_save_lock(|| export_prompts(&p, &Stamp::now())).map(Out::Fields).map_err(failed)))
}

