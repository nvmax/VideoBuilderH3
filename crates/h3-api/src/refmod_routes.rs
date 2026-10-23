//! `GET /refmods`: the saved RefMods, for picking one on a Reference Builder card. Port of `api_refmods`.

use std::collections::HashMap;

use axum::extract::{Query, State};
use h3_pipeline::refmod_library::list_refmods;
use serde_json::json;

use crate::app::{blocking, Shared};
use crate::error::{success, ApiResult};

pub async fn list(State(state): Shared, Query(query): Query<HashMap<String, String>>) -> ApiResult {
    let dirs = h3_pipeline::context::refmod_dirs_of(&state.cfg).await;
    let folder = query.get("folder").cloned().unwrap_or_default();
    let entries = blocking(move || list_refmods(&dirs, &folder)).await?;
    Ok(success(json!({"refmods": entries.iter().map(|e| e.public()).collect::<Vec<_>>()}), None))
}
