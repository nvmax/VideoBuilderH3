//! Story routes of the Builder: the Story Mode steps and the scene story beat. They run on the runner the request's own settings select, and
//! fail with 500 like the pack's.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::post as axum_post;
use axum::Router;
use h3_builder::prompt_writers::RunnerLlm;
use h3_builder::story::arc::{self, ArcError};
use h3_builder::story::{beat, book, places, plan, revise, scene_prompts};
use h3_core::pyval::Obj;
use serde_json::{json, Value};

use super::llm::base_config;
use super::{body_object, into_response, Fail, Out};
use crate::app::AppState;

/// The Story Mode steps behind `/vrgdg/storyboard/story_mode/{action}`, in the order the pack lists them.
const STEPS: [&str; 11] = ["book", "chapter", "split", "convert", "remove_people", "cast", "acts", "plan", "act_summary", "locations", "revise_scene"];

fn finish(result: Result<Value, String>) -> Response {
    into_response(result.map(Out::from).map_err(|message| Fail::new(StatusCode::INTERNAL_SERVER_ERROR, message)))
}

async fn step(llm: &RunnerLlm<'_>, action: &str, payload: &Obj) -> Result<Value, String> {
    match action {
        "book" => book::draft_story_book(llm, payload).await,
        "chapter" => book::rewrite_story_chapter(llm, payload).await,
        "split" => book::split_pasted_story(llm, payload).await,
        "convert" => book::convert_story_arc(llm, payload).await,
        "remove_people" => book::remove_story_people(llm, payload).await,
        "cast" => book::scan_story_cast(llm, payload).await,
        "acts" => plan::build_story_acts(llm, payload).await,
        "plan" => plan::build_story_scene_plan(llm, payload).await,
        "act_summary" => plan::rewrite_act_summary(llm, payload).await,
        "locations" => places::build_story_locations(llm, payload).await,
        _ => revise::revise_story_from_scene(llm, payload).await,
    }
}

async fn story_mode(State(state): State<Arc<AppState>>, Path(action): Path<String>, body: Bytes) -> Response {
    if !STEPS.contains(&action.as_str()) {
        return into_response(Err(Fail::new(StatusCode::BAD_REQUEST, format!("Unknown Story Mode step. Use one of: {}.", STEPS.join(", ")))));
    }
    let payload = match body_object(&body) {
        Ok(payload) => payload,
        Err(fail) => return into_response(Err(fail)),
    };
    let base = base_config(&state);
    let llm = RunnerLlm { payload: &payload, base: &base };
    finish(step(&llm, &action, &payload).await)
}

async fn scene_story_beat(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let payload = match body_object(&body) {
        Ok(payload) => payload,
        Err(fail) => return into_response(Err(fail)),
    };
    let base = base_config(&state);
    let llm = RunnerLlm { payload: &payload, base: &base };
    finish(beat::build_scene_beat(&llm, &payload).await)
}

async fn gemma_image_prompt(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let payload = match body_object(&body) {
        Ok(payload) => payload,
        Err(fail) => return into_response(Err(fail)),
    };
    let base = base_config(&state);
    let llm = RunnerLlm { payload: &payload, base: &base };
    finish(scene_prompts::build_storyboard_image_prompt(&llm, &payload).await)
}

async fn story_brief(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let payload = match body_object(&body) {
        Ok(payload) => payload,
        Err(fail) => return into_response(Err(fail)),
    };
    let base = base_config(&state);
    let llm = RunnerLlm { payload: &payload, base: &base };
    finish(arc::build_story_layer_brief(&llm, &payload).await)
}

/// The story arc fails with the model's own text too when it lost the lyric sections, so the Builder can show what came back.
async fn story_arc(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let payload = match body_object(&body) {
        Ok(payload) => payload,
        Err(fail) => return into_response(Err(fail)),
    };
    let base = base_config(&state);
    let llm = RunnerLlm { payload: &payload, base: &base };
    match arc::build_story_layer_arc(&llm, &payload).await {
        Ok(result) => finish(Ok(result)),
        Err(ArcError::Failed(message)) => finish(Err(message)),
        Err(ArcError::Format(found)) => {
            let diagnostics = json!({
                "kind": "story_arc_format",
                "runner": found.runner,
                "expected_sections": found.expected_sections,
                "raw_output": found.raw_output,
                "cleaned_output": found.cleaned_output,
            });
            into_response(Err(Fail { status: StatusCode::INTERNAL_SERVER_ERROR, body: json!({"ok": false, "error": found.message, "diagnostics": diagnostics}) }))
        }
    }
}

pub fn routes(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route("/vrgdg/storyboard/story_mode/{action}", axum_post(story_mode))
        .route("/vrgdg/storyboard/story_brief", axum_post(story_brief))
        .route("/vrgdg/storyboard/story_arc", axum_post(story_arc))
        .route("/vrgdg/storyboard/scene_story_beat", axum_post(scene_story_beat))
        .route("/vrgdg/storyboard/gemma_image_prompt", axum_post(gemma_image_prompt))
}
