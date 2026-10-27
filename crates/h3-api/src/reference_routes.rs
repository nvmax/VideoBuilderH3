//! `GET/PUT /projects/{pid}/scenes/{sid}/minimax-references`: which references a scene's render sends. Port of
//! `get_scene_minimax_references` and `set_scene_minimax_references` in `agent_api/mutations.py`.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use h3_core::pyval::Obj;
use h3_core::refmod::{assign_labels, refmod_items_for_scene, token_report};
use h3_core::scene::MAX_SCENE_REFERENCES;
use h3_core::settings::settings_for_scene;
use h3_pipeline::refmod_library::find_refmod;
use h3_project::scene_ops::OpError;
use h3_project::{session_ops, Stamp};
use serde_json::{json, Value};

use crate::app::{blocking, body_object, if_match, load, Shared};
use crate::error::{respond, success, ApiError, ApiResult};
use crate::views;

/// `refmod` when the project renders with RefMods, else `standard`.
fn scene_pipeline(session: &Value, segment: &Value) -> &'static str {
    let engine = h3_core::pyval::text_or_empty(session.get("video_engine").filter(|v| h3_core::pyval::truthy(v))).trim().to_lowercase();
    if !engine.is_empty() && !matches!(engine.as_str(), "minimax_h3" | "minimax") {
        return "standard";
    }
    if settings_for_scene(session, Some(segment)).get("pipeline").and_then(Value::as_str) == Some("refmod") {
        "refmod"
    } else {
        "standard"
    }
}

fn text(item: &Obj, key: &str) -> String {
    h3_core::pyval::text_or_empty(item.get(key))
}

fn refmod_view(session: &Value, segment: &Value, index: usize, dirs: &[std::path::PathBuf]) -> Value {
    let settings = settings_for_scene(session, Some(segment));
    let with_audio = settings.get("audio_mode").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or("input_audio") != "built_in_audio";
    let segment_obj = segment.as_object().cloned().unwrap_or_default();
    let (mut items, mut sources, mut warnings): (Vec<Obj>, Vec<Value>, Vec<String>) = (Vec::new(), Vec::new(), Vec::new());
    for mut item in refmod_items_for_scene(session, &segment_obj, index) {
        if item.get("strength").and_then(h3_core::pyval::to_float).unwrap_or(0.0) <= 0.0 {
            continue;
        }
        let mod_name = text(&item, "mod_name");
        let entry = find_refmod(dirs, &mod_name);
        let mut source = json!({"type": "refmod", "refmod": mod_name, "found": entry.is_some(), "kind": item.get("kind").cloned().unwrap_or(Value::Null), "frames": null, "canvas": null});
        if let Some(entry) = entry {
            item.insert("kind".into(), json!(entry.kind));
            item.insert("tokens".into(), json!(entry.tokens));
            source["kind"] = json!(entry.kind);
            source["frames"] = json!(entry.frames);
            source["canvas"] = json!(entry.canvas);
        } else {
            warnings.push(format!("RefMod '{mod_name}' ({}) is not in models/refmods; the render will stop on it.", text(&item, "name")));
        }
        items.push(item);
        sources.push(source);
    }
    let labelled = assign_labels(&items, with_audio);
    let audio_source = json!({"type": "scene_audio", "refmod": "", "found": true, "kind": "audio", "frames": null, "canvas": null});
    let out: Vec<Value> = labelled
        .iter()
        .enumerate()
        .map(|(position, entry)| {
            json!({
                "label": entry["label"], "key": entry["key"], "card_id": entry["card_id"], "name": entry["name"],
                "category": entry["category"], "reference_type": entry["reference_type"], "kind": entry["kind"],
                "strength": entry["strength"], "tokens": entry.get("tokens").and_then(h3_core::pyval::to_int).unwrap_or(0),
                "wears": entry.get("wears").cloned().unwrap_or_else(|| json!("")),
                "source": sources.get(position).cloned().unwrap_or_else(|| audio_source.clone()),
            })
        })
        .collect();
    if items.is_empty() {
        warnings.push("No RefMod is mapped to this scene; the render needs at least one.".into());
    }
    if items.len() > MAX_SCENE_REFERENCES {
        warnings.push(format!(
            "This scene maps {} RefMods and a scene can use at most {MAX_SCENE_REFERENCES}; the render will stop. Remove some from the scene mapping.",
            items.len()
        ));
    }
    let report = token_report(&labelled);
    if report["over_limit"].as_bool() == Some(true) {
        warnings.push(format!(
            "These RefMods use {} tokens, above {}; the scene may not render or may crash. The render is not stopped.",
            report["total"], report["limit"]
        ));
    }
    json!({
        "pipeline": "refmod",
        "scene_id": h3_core::pyval::text_or_empty(segment.get("id")),
        "scene_number": index + 1,
        "video_mode": settings.get("video_mode").and_then(Value::as_str).unwrap_or(""),
        "audio_mode": if with_audio { "input_audio" } else { "built_in_audio" },
        "labels": out.iter().map(|o| o["label"].clone()).collect::<Vec<_>>(),
        "items": out,
        "tokens": report,
        "warnings": warnings,
        "editable": false,
    })
}

pub async fn get(State(state): Shared, Path((pid, sid)): Path<(String, String)>) -> ApiResult {
    let loaded = load(&state, &pid).await?;
    let index = views::find_scene(&loaded, &sid)?;
    let session = Value::Object(loaded.session.clone());
    let segments: Vec<Value> = views::segments(&loaded.session).into_iter().map(Value::Object).collect();
    let segment = segments[index].clone();
    if scene_pipeline(&session, &segment) == "refmod" {
        let dirs = h3_pipeline::context::refmod_dirs_of(&state.cfg).await;
        let view = blocking(move || refmod_view(&session, &segment, index, &dirs)).await?;
        return Ok(success(view, None));
    }
    let mut choices = session_ops::scene_reference_choices(&session, &segments, index);
    choices["pipeline"] = json!("standard");
    Ok(success(choices, None))
}

pub async fn put(State(state): Shared, Path((pid, sid)): Path<(String, String)>, headers: HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    let body = body_object(body)?;
    let automatic = body.get("automatic").is_some_and(h3_core::pyval::truthy);
    let keys = body.get("keys").filter(|v| !v.is_null()).cloned();
    if automatic && keys.is_some() {
        return Err(ApiError::validation("Send either `keys` (a chosen order) or `automatic: true`, not both."));
    }
    if !automatic && keys.is_none() {
        return Err(ApiError::validation("Send `keys` (a list of reference keys) or `automatic: true`."));
    }
    let expected = if_match(&headers)?;
    let folder = views::resolve_project(&state.roots, &pid)?;
    let _guard = state.edit_lock.lock().await;
    let loaded = load(&state, &pid).await?;
    let index = views::find_scene(&loaded, &sid)?;
    let segment = Value::Object(views::segments(&loaded.session)[index].clone());
    if scene_pipeline(&Value::Object(loaded.session.clone()), &segment) == "refmod" {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "NOT_SUPPORTED_FOR_MODE",
            "This project renders with RefMods, which take the scene's references from the Reference Builder scene mappings, not from a chosen MiniMax reference order. Change them with PUT /references/scene-mapping (or PATCH the scene's subject_ids / location_id). Nothing was saved.",
        )
        .details(json!({"mode": "refmod", "feature": "minimax_h3_reference_keys"})));
    }
    let scene_id = h3_core::pyval::text_or_empty(segment.get("id"));
    let result = blocking(move || session_ops::set_scene_reference_keys(&folder, &scene_id, keys.as_ref(), expected, &Stamp::now()))
        .await?
        .map_err(|e| match e {
            OpError::SceneNotFound(id) => ApiError::scene_not_found(&id, &pid),
            OpError::Validation(m) => ApiError::validation(m),
            OpError::Conflict { current, incoming } => ApiError::revision_conflict(current, incoming),
            OpError::Io(e) => ApiError::internal(e.to_string()),
        })?;
    let revision = result.get("revision").and_then(Value::as_i64);
    Ok(respond(StatusCode::OK, result, revision))
}
