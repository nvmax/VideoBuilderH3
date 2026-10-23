//! Routes for image presets, scene images and reference card images.

use std::path::PathBuf;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use h3_comfy::ComfyClient;
use h3_core::pyval::Obj;
use h3_image::{apply_preset, choices_for, ApplyCtx, ImagePreset, PresetOrigin};
use h3_project::{images, persist_session, LoadedSession, Stamp};
use serde_json::{json, Value};

use crate::app::{blocking, body_object, check_revision, job_response, load, Shared};
use crate::error::{success, ApiError, ApiResult};
use crate::image_jobs::{IMAGES_GENERATE_BATCH, IMAGE_GENERATE, REFERENCE_IMAGE_GENERATE};
use crate::views;

fn preset_view(preset: &ImagePreset, with_workflow: bool) -> Value {
    let mut value = json!({
        "id": preset.id,
        "name": preset.name,
        "description": preset.description,
        "origin": preset.origin,
        "bindings": preset.bindings,
        "output_nodes": preset.output_nodes,
    });
    if with_workflow {
        value["workflow"] = preset.workflow.clone();
    }
    value
}

fn get_preset(state: &crate::app::AppState, id: &str) -> Result<ImagePreset, ApiError> {
    state.presets.get(id).ok_or_else(|| ApiError::from(h3_image::ImageError::NotFound(id.to_string())))
}

pub async fn list_presets(State(state): Shared) -> ApiResult {
    Ok(success(Value::Array(state.presets.list().iter().map(|p| preset_view(p, false)).collect()), None))
}

pub async fn get_one_preset(State(state): Shared, Path(id): Path<String>) -> ApiResult {
    Ok(success(preset_view(&get_preset(&state, &id)?, true), None))
}

/// `{name, description, workflow}` where the workflow is the API format JSON, as an object or as text.
pub async fn import_preset(State(state): Shared, body: Option<Json<Value>>) -> ApiResult {
    let body = body_object(body)?;
    let name = body.get("name").and_then(Value::as_str).unwrap_or("").to_string();
    let description = body.get("description").and_then(Value::as_str).unwrap_or("").to_string();
    let text = match body.get("workflow") {
        Some(Value::String(s)) => s.clone(),
        Some(v @ Value::Object(_)) => v.to_string(),
        _ => return Err(ApiError::validation("Send the workflow saved with Export (API) as `workflow`.")),
    };
    let store = state.presets_dir.clone();
    let preset = blocking(move || h3_image::PresetStore::new(&store).import(&name, &description, &text)).await??;
    Ok(crate::error::respond(StatusCode::CREATED, preset_view(&preset, true), None))
}

/// Change a custom preset: its name, description, bindings or output nodes.
pub async fn update_preset(State(state): Shared, Path(id): Path<String>, body: Option<Json<Value>>) -> ApiResult {
    let body = body_object(body)?;
    let mut preset = get_preset(&state, &id)?;
    if preset.origin != PresetOrigin::Custom {
        return Err(ApiError::validation("A built-in preset cannot be changed. Import a copy instead."));
    }
    if let Some(name) = body.get("name").and_then(Value::as_str) {
        preset.name = name.trim().to_string();
    }
    if let Some(description) = body.get("description").and_then(Value::as_str) {
        preset.description = description.to_string();
    }
    if let Some(bindings) = body.get("bindings") {
        preset.bindings = serde_json::from_value(bindings.clone()).map_err(|e| ApiError::validation(format!("The bindings are not valid: {e}")))?;
    }
    if let Some(nodes) = body.get("output_nodes") {
        preset.output_nodes = serde_json::from_value(nodes.clone()).map_err(|e| ApiError::validation(format!("output_nodes must be a list of node ids: {e}")))?;
    }
    for binding in &preset.bindings {
        for target in &binding.targets {
            if preset.workflow.get(&target.node).is_none() {
                return Err(ApiError::validation(format!("Binding '{}' points at node {}, which the workflow does not have.", binding.key, target.node)));
            }
        }
    }
    let store = state.presets_dir.clone();
    let saved = preset.clone();
    blocking(move || h3_image::PresetStore::new(&store).save(&saved)).await??;
    Ok(success(preset_view(&preset, true), None))
}

pub async fn delete_preset(State(state): Shared, Path(id): Path<String>) -> ApiResult {
    let store = state.presets_dir.clone();
    blocking(move || h3_image::PresetStore::new(&store).delete(&id)).await??;
    Ok(success(json!({"deleted": true}), None))
}

/// The names ComfyUI offers for each model binding, for dropdowns.
pub async fn preset_choices(State(state): Shared, Path(id): Path<String>) -> ApiResult {
    let preset = get_preset(&state, &id)?;
    let info = ComfyClient::new(state.cfg.comfy_base()).object_info().await?;
    let mut out = serde_json::Map::new();
    for binding in &preset.bindings {
        let choices = choices_for(&preset.workflow, binding, &info);
        if !choices.is_empty() {
            out.insert(binding.key.clone(), json!(choices));
        }
    }
    Ok(success(Value::Object(out), None))
}

/// The graph a preset would send for these values, without running it. Image files are not copied.
pub async fn preset_graph(State(state): Shared, Path(id): Path<String>, body: Option<Json<Value>>) -> ApiResult {
    let preset = get_preset(&state, &id)?;
    let values: Obj = body_object(body)?.get("values").and_then(Value::as_object).cloned().unwrap_or_default();
    let applied = blocking(move || {
        let scratch = std::env::temp_dir().join("vrgdg_h3_dry_run");
        let mut ctx = ApplyCtx { input_dir: scratch, random: Box::new(|| 1) };
        apply_preset(&preset, &values, &mut ctx)
    })
    .await??;
    Ok(success(json!({"graph": applied.graph, "seed": applied.seed}), None))
}

// -- scene images -----------------------------------------------------------------------------------------------------------

pub async fn generate_scene_image(State(state): Shared, Path((pid, sid)): Path<(String, String)>, body: Option<Json<Value>>) -> ApiResult {
    let mut params = body_object(body)?;
    let loaded = load(&state, &pid).await?;
    views::find_scene(&loaded, &sid)?;
    params.insert("scene_id".into(), json!(sid));
    Ok(job_response(&state.jobs.submit(IMAGE_GENERATE, Some(&pid), Value::Object(params), true)?))
}

pub async fn generate_batch_images(State(state): Shared, Path(pid): Path<String>, body: Option<Json<Value>>) -> ApiResult {
    let params = body_object(body)?;
    load(&state, &pid).await?;
    Ok(job_response(&state.jobs.submit(IMAGES_GENERATE_BATCH, Some(&pid), Value::Object(params), true)?))
}

/// Load, change and save the session in one step. A stale `If-Match` is a 409.
async fn change_session<T: Send + 'static>(
    state: &crate::app::AppState,
    pid: &str,
    headers: &axum::http::HeaderMap,
    change: impl FnOnce(&mut Obj, &LoadedSession) -> std::io::Result<T> + Send + 'static,
) -> Result<(T, i64), ApiError> {
    let loaded = load(state, pid).await?;
    check_revision(headers, loaded.revision)?;
    blocking(move || -> std::io::Result<(T, i64)> {
        let mut session = loaded.session.clone();
        let result = change(&mut session, &loaded)?;
        let saved = persist_session(&loaded.project_folder, session, Stamp::now())?;
        Ok((result, saved.revision))
    })
    .await?
    .map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidInput | std::io::ErrorKind::InvalidData => ApiError::validation(e.to_string()),
        _ => ApiError::internal(e.to_string()),
    })
}

fn scene_index(loaded: &LoadedSession, sid: &str) -> std::io::Result<usize> {
    views::find_scene(loaded, sid).map_err(|e| std::io::Error::new(std::io::ErrorKind::NotFound, e.message))
}

/// A file path or uploaded image data, archived into the scene's previews. Returns the new path.
fn archive_from_body(project: &std::path::Path, scene_number: u32, body: &Obj) -> std::io::Result<PathBuf> {
    let now = Stamp::now();
    if let Some(data) = body.get("image_data").and_then(Value::as_str).filter(|d| !d.trim().is_empty()) {
        let (bytes, ext) = images::decode_image_data(data)?;
        return images::archive_scene_image_bytes(project, scene_number, &bytes, &ext, &now);
    }
    match body.get("source_path").and_then(Value::as_str).map(str::trim).filter(|p| !p.is_empty()) {
        Some(path) => images::archive_scene_image(project, scene_number, std::path::Path::new(path), &now),
        None => Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Send source_path or image_data.")),
    }
}

/// Set the scene's own image from a file or upload (`custom_image_path`).
pub async fn put_scene_image(State(state): Shared, Path((pid, sid)): Path<(String, String)>, headers: axum::http::HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    let body = body_object(body)?;
    let (path, revision) = change_session(&state, &pid, &headers, move |session, loaded| {
        let index = scene_index(loaded, &sid)?;
        let saved = archive_from_body(&loaded.project_folder, index as u32 + 1, &body)?;
        let segment = session["segments"][index].as_object_mut().expect("scene object");
        segment.insert("custom_image_path".into(), json!(saved.display().to_string()));
        segment.insert("preview_mode".into(), json!("image"));
        Ok(saved.display().to_string())
    })
    .await?;
    Ok(success(json!({"custom_image_path": path, "revision": revision}), Some(revision)))
}

pub async fn delete_scene_image(State(state): Shared, Path((pid, sid)): Path<(String, String)>, headers: axum::http::HeaderMap) -> ApiResult {
    let (_, revision) = change_session(&state, &pid, &headers, move |session, loaded| {
        let index = scene_index(loaded, &sid)?;
        images::clear_scene_image(session, index)
    })
    .await?;
    Ok(success(json!({"cleared": true, "revision": revision}), Some(revision)))
}

pub async fn approve_scene_image(State(state): Shared, Path((pid, sid)): Path<(String, String)>, headers: axum::http::HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    let body = body_object(body)?;
    let wanted = body.get("image_path").and_then(Value::as_str).map(str::to_string);
    let (path, revision) = change_session(&state, &pid, &headers, move |session, loaded| {
        let index = scene_index(loaded, &sid)?;
        images::approve_scene_image(session, index, wanted.as_deref())
    })
    .await?;
    Ok(success(json!({"approved_image_path": path, "revision": revision}), Some(revision)))
}

pub async fn revert_scene_image(State(state): Shared, Path((pid, sid)): Path<(String, String)>, headers: axum::http::HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    let body = body_object(body)?;
    let delta = body.get("delta").and_then(Value::as_i64).unwrap_or(-1);
    let to_index = body.get("index").and_then(Value::as_i64);
    let ((index, current, total), revision) = change_session(&state, &pid, &headers, move |session, loaded| {
        let scene = scene_index(loaded, &sid)?;
        images::revert_scene_image(session, scene, delta, to_index)
    })
    .await?;
    Ok(success(json!({"history_index": index, "current_image": current, "total_takes": total, "revision": revision}), Some(revision)))
}

pub async fn scene_image_history(State(state): Shared, Path((pid, sid)): Path<(String, String)>) -> ApiResult {
    let loaded = load(&state, &pid).await?;
    let index = views::find_scene(&loaded, &sid)?;
    let segment = &views::segments(&loaded.session)[index];
    let history: Vec<Value> = segment.get("image_history").and_then(Value::as_array).cloned().unwrap_or_default();
    Ok(success(
        json!({
            "history": history,
            "history_index": segment.get("image_history_index").cloned().unwrap_or(Value::Null),
            "approved_image_path": segment.get("approved_image_path").cloned().unwrap_or(json!("")),
            "custom_image_path": segment.get("custom_image_path").cloned().unwrap_or(json!("")),
        }),
        Some(loaded.revision),
    ))
}

// -- reference cards --------------------------------------------------------------------------------------------------------

/// Characters and locations with their pictures, without the preview data.
pub async fn get_references(State(state): Shared, Path(pid): Path<String>) -> ApiResult {
    let loaded = load(&state, &pid).await?;
    let refs = loaded.session.get("flux_reference_builder").and_then(Value::as_object).cloned().unwrap_or_default();
    let strip = |list: Option<&Value>| -> Vec<Value> {
        list.and_then(Value::as_array)
            .map(|cards| {
                cards
                    .iter()
                    .map(|card| {
                        let mut card = card.clone();
                        if let Some(image) = card.get_mut("image").and_then(Value::as_object_mut) {
                            image.remove("data");
                        }
                        card
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    Ok(success(json!({"subjects": strip(refs.get("subjects")), "locations": strip(refs.get("locations"))}), Some(loaded.revision)))
}

fn check_kind(kind: &str) -> Result<(), ApiError> {
    match kind {
        "subjects" | "locations" => Ok(()),
        _ => Err(ApiError::validation("kind must be 'subjects' or 'locations'.")),
    }
}

pub async fn generate_reference_image(State(state): Shared, Path((pid, kind, rid)): Path<(String, String, String)>, body: Option<Json<Value>>) -> ApiResult {
    check_kind(&kind)?;
    let mut params = body_object(body)?;
    let loaded = load(&state, &pid).await?;
    if images::reference_card(&loaded.session, &kind, &rid).is_none() {
        return Err(ApiError::validation(format!("Reference '{rid}' was not found in the Reference Builder.")));
    }
    params.insert("kind".into(), json!(kind));
    params.insert("ref_id".into(), json!(rid));
    Ok(job_response(&state.jobs.submit(REFERENCE_IMAGE_GENERATE, Some(&pid), Value::Object(params), true)?))
}

/// Set a card's picture from a file or uploaded data.
pub async fn put_reference_image(State(state): Shared, Path((pid, kind, rid)): Path<(String, String, String)>, headers: axum::http::HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    check_kind(&kind)?;
    let body = body_object(body)?;
    let (card, revision) = change_session(&state, &pid, &headers, move |session, loaded| {
        let (name, _) = images::reference_card(session, &kind, &rid).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, format!("Reference '{rid}' was not found.")))?;
        let temp;
        let source: PathBuf = match (body.get("image_data").and_then(Value::as_str).filter(|d| !d.trim().is_empty()), body.get("source_path").and_then(Value::as_str).map(str::trim).filter(|p| !p.is_empty())) {
            (Some(data), _) => {
                let (bytes, ext) = images::decode_image_data(data)?;
                temp = loaded.project_folder.join(format!(".upload_{}.{ext}", Stamp::now().compact));
                std::fs::write(&temp, bytes)?;
                temp.clone()
            }
            (None, Some(path)) => PathBuf::from(path),
            _ => return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Send source_path or image_data.")),
        };
        let saved = images::save_reference_image(&loaded.project_folder, &kind, &name, &source);
        if source.starts_with(&loaded.project_folder) && source.file_name().is_some_and(|n| n.to_string_lossy().starts_with(".upload_")) {
            let _ = std::fs::remove_file(&source);
        }
        let image = saved?;
        let path = image["path"].clone();
        images::set_reference_image(session, &kind, &rid, image)?;
        Ok(path)
    })
    .await?;
    Ok(success(json!({"image_path": card, "revision": revision}), Some(revision)))
}
