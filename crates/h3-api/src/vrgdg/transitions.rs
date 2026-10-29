//! The transition library for the Builder's timeline: `GET /vrgdg/music_builder/transitions` answers with every transition and its GLSL ES 3.00 source, which
//! the page's WebGL preview compiles. The final video draws the same transitions from their WGSL sources in `h3-transitions`.
//! `GET /vrgdg/music_builder/effects` and `/filters` (each with `/capabilities`) do the same for the effect lane and the filter lane.
//! `GET /vrgdg/music_builder/transitions/capabilities` tells the front end (the pack's `transition_library.mjs`) that this build can draw them, which turns on
//! the lane, the Post Process tab and the live preview. The pack has no such route, so there they stay out of sight.

use std::sync::Arc;

use axum::Router;
use h3_transitions::Kind;
use serde_json::{json, Value};

use super::get_query;
use crate::app::AppState;

fn library() -> Value {
    let list: Vec<Value> = h3_transitions::catalog()
        .into_iter()
        .map(|mut entry| {
            let source = entry.get("id").and_then(Value::as_str).and_then(|id| h3_transitions::glsl3(id).ok()).unwrap_or_default();
            if let Some(map) = entry.as_object_mut() {
                map.insert("glsl3".into(), json!(source));
            }
            entry
        })
        .collect();
    json!({"transitions": list})
}

/// Bumped when the transition fields or the render change in a way the Builder has to know about.
const TRANSITIONS_VERSION: u32 = 1;
/// The same for the effects and the filters.
const LOOKS_VERSION: u32 = 1;

/// Every effect or filter with its GLSL ES 3.00 source, under `effects` or `filters`.
fn look_library(kind: Kind) -> Value {
    let list: Vec<Value> = h3_transitions::look_catalog(kind)
        .into_iter()
        .map(|mut entry| {
            let source = entry.get("id").and_then(Value::as_str).and_then(|id| h3_transitions::look_glsl3(kind, id).ok()).unwrap_or_default();
            if let Some(map) = entry.as_object_mut() {
                map.insert("glsl3".into(), json!(source));
            }
            entry
        })
        .collect();
    json!({format!("{}s", kind.name()): list})
}

fn look_routes(router: Router<Arc<AppState>>, kind: Kind) -> Router<Arc<AppState>> {
    let base = format!("/vrgdg/music_builder/{}s", kind.name());
    router
        .route(&base, get_query(move |_, _| Ok(look_library(kind).into())))
        .route(&format!("{base}/capabilities"), get_query(move |_, _| Ok(json!({"version": LOOKS_VERSION, "count": h3_transitions::look_catalog(kind).len()}).into())))
}

pub fn routes(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    let router = router
        .route("/vrgdg/music_builder/transitions", get_query(|_, _| Ok(library().into())))
        .route("/vrgdg/music_builder/transitions/capabilities", get_query(|_, _| Ok(json!({"version": TRANSITIONS_VERSION, "count": h3_transitions::catalog().len()}).into())));
    look_routes(look_routes(router, Kind::Effect), Kind::Filter)
}
