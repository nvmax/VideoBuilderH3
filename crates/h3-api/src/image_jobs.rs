//! Image jobs: a scene image, a batch of scene images and a reference card image.

#![allow(clippy::result_large_err)]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use h3_core::pyval::{text_or_empty, truthy, Obj};
use h3_image::{generate, ImageEvent, ImagePreset};
use h3_jobs::{Handler, JobCtx, JobError, ProgressUpdate};
use h3_pipeline::RunContext;
use h3_project::{images, load_session, persist_session, Stamp};
use serde_json::{json, Value};

use crate::app::AppState;
use crate::error::ApiError;
use crate::job_handlers::{context_for, fail};
use crate::views;

pub const IMAGE_GENERATE: &str = "image.generate";
pub const IMAGES_GENERATE_BATCH: &str = "images.generate_batch";
pub const REFERENCE_IMAGE_GENERATE: &str = "reference.image.generate";

/// Params that steer the job. Every other key is a value for a preset binding.
const CONTROL_KEYS: [&str; 12] =
    ["scene_id", "mode", "preset_id", "timeout_seconds", "from_previous_final_frame", "scope", "scene_ids", "run_mode", "resume", "kind", "ref_id", "image_ingredients"];

type Run = fn(Arc<AppState>, JobCtx) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, JobError>> + Send>>;

pub fn register(state: &Arc<AppState>) {
    let table: [(&str, Run); 3] = [
        (IMAGE_GENERATE, |s, c| Box::pin(scene_image(s, c))),
        (IMAGES_GENERATE_BATCH, |s, c| Box::pin(batch_images(s, c))),
        (REFERENCE_IMAGE_GENERATE, |s, c| Box::pin(reference_image(s, c))),
    ];
    for (kind, run) in table {
        let owned = Arc::clone(state);
        let handler: Handler = Arc::new(move |ctx| run(Arc::clone(&owned), ctx));
        state.jobs.register(kind, handler);
    }
}

fn reporter(ctx: &JobCtx, scene_id: Option<String>, from: f64, span: f64) -> impl FnMut(ImageEvent) + Send + use<> {
    let ctx = ctx.clone();
    move |event: ImageEvent| {
        ctx.log("info", &event.message);
        ctx.progress(ProgressUpdate {
            percent: from + span * event.percent / 100.0,
            stage: event.stage.to_string(),
            message: event.message,
            scene_id: scene_id.clone(),
            stage_count: 1,
            ..Default::default()
        });
    }
}

/// The preset a job uses: `preset_id`, else `mode`, else the project's image mode when it names one, else Z-Image.
fn resolve_preset(state: &AppState, params: &Value, session: &Obj) -> Result<ImagePreset, JobError> {
    let named = ["preset_id", "mode"].iter().filter_map(|k| params.get(*k).and_then(Value::as_str)).map(str::trim).find(|s| !s.is_empty()).map(str::to_string);
    let project_mode = text_or_empty(session.get("image_model_mode")).trim().to_string();
    let candidates: Vec<String> = match named {
        Some(id) => vec![id],
        None => vec![project_mode, "zimage".into()],
    };
    for id in &candidates {
        if let Some(preset) = state.presets.get(id) {
            return Ok(preset);
        }
    }
    Err(fail(h3_image::ImageError::NotFound(candidates.first().cloned().unwrap_or_default())))
}

/// Binding values from the job params: everything that is not a control key. Pack style names for reference images are
/// accepted (`image_ingredients`, `images`).
fn binding_values(params: &Value) -> Obj {
    let mut values = Obj::new();
    if let Some(map) = params.as_object() {
        for (key, value) in map {
            if !CONTROL_KEYS.contains(&key.as_str()) && !value.is_null() {
                values.insert(key.clone(), value.clone());
            }
        }
        for alias in ["image_ingredients", "images"] {
            if let Some(v) = map.get(alias).filter(|v| !v.is_null()) {
                values.entry("reference_images").or_insert_with(|| v.clone());
            }
        }
    }
    values
}

fn timeout(params: &Value) -> Duration {
    Duration::from_secs_f64(params.get("timeout_seconds").and_then(Value::as_f64).unwrap_or(600.0).max(30.0))
}

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Result<T, JobError> {
    tokio::task::spawn_blocking(work).await.map_err(|e| fail(ApiError::internal(e.to_string())))
}

fn io_fail(e: std::io::Error) -> JobError {
    fail(ApiError::internal(e.to_string()))
}

/// Generate and record one scene image.
#[allow(clippy::too_many_arguments)]
async fn generate_for_scene(state: &AppState, ctx: &JobCtx, run: &RunContext, folder: &Path, scene_ref: &str, params: &Value, from: f64, span: f64) -> Result<Value, JobError> {
    let project = folder.to_path_buf();
    let loaded = blocking({
        let project = project.clone();
        move || load_session(&project)
    })
    .await?
    .map_err(io_fail)?;
    let index = views::find_scene(&loaded, scene_ref).map_err(fail)?;
    let segments = views::segments(&loaded.session);
    let segment = &segments[index];
    let preset = resolve_preset(state, params, &loaded.session)?;

    let mut values = binding_values(params);
    if values.get("prompt").is_none_or(|v| !truthy(v)) {
        let prompt = text_or_empty(segment.get("t2i_prompt")).trim().to_string();
        if prompt.is_empty() {
            return Err(fail(ApiError::validation(format!("Scene '{scene_ref}' has no prompt specified and t2i_prompt is empty."))));
        }
        values.insert("prompt".into(), json!(prompt));
    }
    // Image to image from the previous scene's picture.
    if params.get("from_previous_final_frame").is_some_and(truthy) && index > 0 {
        let previous = &segments[index - 1];
        let history: Vec<String> = previous.get("image_history").and_then(Value::as_array).map(|a| a.iter().map(|v| text_or_empty(Some(v))).collect()).unwrap_or_default();
        let source = Some(text_or_empty(previous.get("approved_image_path"))).filter(|s| !s.is_empty()).or_else(|| history.last().cloned());
        if let Some(source) = source.filter(|s| Path::new(s).is_file()) {
            values.insert("image_to_image".into(), json!(source));
        }
    }

    let made = generate(&run.client, &run.env.dirs, &run.env.object_info, &preset, &values, timeout(params), &ctx.cancel, reporter(ctx, Some(scene_ref.to_string()), from, span * 0.8))
        .await
        .map_err(fail)?;
    let image = made.images.last().cloned().ok_or_else(|| fail(h3_image::ImageError::NoImage(made.prompt_id.clone())))?;
    ctx.progress(ProgressUpdate { percent: from + span * 0.85, stage: "archiving".into(), message: "Archiving the scene preview...".into(), scene_id: Some(scene_ref.to_string()), stage_count: 1, ..Default::default() });

    let scene_number = index as u32 + 1;
    let info = json!({"filename": image.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), "path": image.display().to_string()});
    let outcome = blocking(move || -> std::io::Result<(String, usize, usize, i64)> {
        let now = Stamp::now();
        let saved = images::archive_scene_image(&project, scene_number, &image, &now)?;
        // The scene may have changed while the image rendered, so the session is read again before the record.
        let fresh = load_session(&project)?;
        let mut session = fresh.session;
        let (shown, count) = images::record_scene_image(&mut session, index, &info, &saved)?;
        let saved_session = persist_session(&project, session, now)?;
        Ok((saved.display().to_string(), shown, count, saved_session.revision))
    })
    .await?
    .map_err(io_fail)?;
    Ok(json!({
        "scene_id": scene_ref,
        "preset_id": preset.id,
        "saved_path": outcome.0,
        "history_index": outcome.1,
        "history_count": outcome.2,
        "revision": outcome.3,
        "seed": made.seed,
    }))
}

async fn scene_image(state: Arc<AppState>, ctx: JobCtx) -> Result<Value, JobError> {
    let scene = match ctx.params.get("scene_id") {
        Some(Value::String(s)) if !s.trim().is_empty() => s.trim().to_string(),
        Some(Value::Number(n)) => n.to_string(),
        _ => return Err(fail(ApiError::validation("scene_id is required."))),
    };
    let (run, folder) = context_for(&state, &ctx).await?;
    generate_for_scene(&state, &ctx, &run, &folder, &scene, &ctx.params, 0.0, 100.0).await
}

async fn batch_images(state: Arc<AppState>, ctx: JobCtx) -> Result<Value, JobError> {
    let (run, folder) = context_for(&state, &ctx).await?;
    let loaded = blocking({
        let folder = folder.clone();
        move || load_session(&folder)
    })
    .await?
    .map_err(io_fail)?;
    let params = ctx.params.clone();
    let scope = params.get("scope").and_then(Value::as_str).unwrap_or("all").trim().to_lowercase();
    let resume = params.get("resume").and_then(Value::as_bool).unwrap_or(false) || matches!(params.get("run_mode").and_then(Value::as_str), Some("resume_missing" | "missing"));
    let wanted: Vec<String> = params.get("scene_ids").and_then(Value::as_array).map(|a| a.iter().map(|v| text_or_empty(Some(v))).collect()).unwrap_or_default();
    let segments = views::segments(&loaded.session);
    let mut targets: Vec<(String, usize)> = Vec::new();
    for (i, segment) in segments.iter().enumerate() {
        let id = text_or_empty(segment.get("id"));
        let id = if id.is_empty() { (i + 1).to_string() } else { id };
        if scope == "selected" && !wanted.is_empty() && !wanted.contains(&id) && !wanted.contains(&(i + 1).to_string()) {
            continue;
        }
        let has_image = ["approved_image_path", "custom_image_path"].iter().any(|k| segment.get(*k).is_some_and(truthy)) || segment.get("image_history").and_then(Value::as_array).is_some_and(|a| !a.is_empty());
        if resume && has_image {
            continue;
        }
        targets.push((id, i + 1));
    }
    let total = targets.len();
    if total == 0 {
        return Ok(json!({"processed": 0, "message": "No scenes matched batch image criteria."}));
    }
    let mut results = Vec::new();
    for (n, (id, number)) in targets.iter().enumerate() {
        if ctx.is_cancelled() {
            return Err(JobError::cancelled());
        }
        let from = n as f64 / total as f64 * 100.0;
        ctx.progress(ProgressUpdate {
            percent: from,
            stage: "batch_images".into(),
            message: format!("Generating image for scene {} of {total} ({id})...", n + 1),
            scene_id: Some(id.clone()),
            scene_index: Some(n as i64 + 1),
            scene_count: Some(total as i64),
            stage_index: n as i64 + 1,
            stage_count: total as i64,
        });
        match generate_for_scene(&state, &ctx, &run, &folder, &number.to_string(), &params, from, 100.0 / total as f64).await {
            Ok(done) => results.push(json!({"scene_id": id, "status": "success", "saved_path": done["saved_path"]})),
            Err(e) if e.code == h3_jobs::JOB_CANCELLED => return Err(e),
            Err(e) => results.push(json!({"scene_id": id, "status": "error", "error": e.message})),
        }
    }
    let failed = results.iter().filter(|r| r["status"] == "error").count();
    if failed == results.len() {
        let first = results.iter().find_map(|r| r["error"].as_str()).unwrap_or("").to_string();
        let mut error = JobError::new("BATCH_FAILED", format!("Every scene failed. First error: {first}"));
        error.details = json!({"results": results});
        error.retryable = true;
        return Err(error);
    }
    Ok(json!({"processed": results.len(), "total_targets": total, "failed": failed, "results": results}))
}

/// Generate a picture for a character or location card and set it as the card's image.
async fn reference_image(state: Arc<AppState>, ctx: JobCtx) -> Result<Value, JobError> {
    let kind = ctx.params.get("kind").and_then(Value::as_str).unwrap_or("").to_string();
    let card_id = ctx.params.get("ref_id").and_then(Value::as_str).unwrap_or("").to_string();
    if kind != "subjects" && kind != "locations" {
        return Err(fail(ApiError::validation("kind must be 'subjects' or 'locations'.")));
    }
    let (run, folder) = context_for(&state, &ctx).await?;
    let loaded = blocking({
        let folder = folder.clone();
        move || load_session(&folder)
    })
    .await?
    .map_err(io_fail)?;
    let (name, _) = images::reference_card(&loaded.session, &kind, &card_id).ok_or_else(|| fail(ApiError::validation(format!("Reference '{card_id}' was not found in the Reference Builder."))))?;
    let preset = resolve_preset(&state, &ctx.params, &loaded.session)?;
    let mut values = binding_values(&ctx.params);
    if values.get("prompt").is_none_or(|v| !truthy(v)) {
        let card = loaded.session["flux_reference_builder"][&kind].as_array().and_then(|a| a.iter().find(|c| c["id"] == card_id.as_str())).cloned().unwrap_or(Value::Null);
        let description = text_or_empty(card.get("description")).trim().to_string();
        if description.is_empty() {
            return Err(fail(ApiError::validation("Send a prompt, or give the reference a description to use as the prompt.")));
        }
        values.insert("prompt".into(), json!(description));
    }
    let made = generate(&run.client, &run.env.dirs, &run.env.object_info, &preset, &values, timeout(&ctx.params), &ctx.cancel, reporter(&ctx, None, 0.0, 85.0)).await.map_err(fail)?;
    let image = made.images.last().cloned().ok_or_else(|| fail(h3_image::ImageError::NoImage(made.prompt_id.clone())))?;
    let revision = blocking(move || -> std::io::Result<i64> {
        let card_image = images::save_reference_image(&folder, &kind, &name, &image)?;
        let fresh = load_session(&folder)?;
        let mut session = fresh.session;
        images::set_reference_image(&mut session, &kind, &card_id, card_image)?;
        Ok(persist_session(&folder, session, Stamp::now())?.revision)
    })
    .await?
    .map_err(io_fail)?;
    Ok(json!({"preset_id": preset.id, "revision": revision, "seed": made.seed}))
}
