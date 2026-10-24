//! LLM jobs: writing MiniMax prompts for scenes.

#![allow(clippy::result_large_err)]

use std::sync::Arc;

use h3_core::pyval::{text_or_empty, Obj};
use h3_core::timeline::sort_segments;
use h3_jobs::{Handler, JobCtx, JobError, ProgressUpdate};
use h3_pipeline::minimax_prompts::{generate_scene_prompt, PromptError, PromptParams};
use h3_project::{load_session, session_ops, Stamp};
use serde_json::{json, Value};

use crate::app::AppState;
use crate::error::ApiError;
use crate::llm_routes::current;
use crate::views::resolve_project;

pub const MINIMAX_PROMPTS: &str = "minimax.prompts";
pub const REFERENCE_DESCRIBE: &str = "reference.describe";
pub const EXTRACT_LOCATIONS: &str = "reference.extract_locations";
pub const SCENE_IMAGE_PROMPT: &str = "llm.scene_image_prompt";
pub const REFERENCE_IMAGE_PROMPT: &str = "reference.image_prompt";
pub const BATCH_PROMPTS: &str = "llm.batch_prompts";

type Run = fn(Arc<AppState>, JobCtx) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, JobError>> + Send>>;

pub fn register(state: &Arc<AppState>) {
    let table: [(&str, Run); 6] = [
        (SCENE_IMAGE_PROMPT, |s, c| Box::pin(scene_image_prompt(s, c))),
        (REFERENCE_IMAGE_PROMPT, |s, c| Box::pin(reference_image_prompt(s, c))),
        (BATCH_PROMPTS, |s, c| Box::pin(batch_prompts(s, c))),
        (MINIMAX_PROMPTS, |s, c| Box::pin(minimax_prompts(s, c))),
        (REFERENCE_DESCRIBE, |s, c| Box::pin(describe_reference(s, c))),
        (EXTRACT_LOCATIONS, |s, c| Box::pin(extract_locations(s, c))),
    ];
    for (kind, run) in table {
        let owned = Arc::clone(state);
        let handler: Handler = Arc::new(move |ctx| run(Arc::clone(&owned), ctx));
        state.jobs.register(kind, handler);
    }
}

fn fail(error: impl Into<ApiError>) -> JobError {
    error.into().into_job_error()
}

fn internal(e: impl std::fmt::Display) -> JobError {
    fail(ApiError::internal(e.to_string()))
}

fn num(params: &Value, key: &str, default: f64) -> f64 {
    params.get(key).and_then(Value::as_f64).unwrap_or(default)
}

async fn read_session(folder: &std::path::Path) -> Result<h3_project::LoadedSession, JobError> {
    let folder = folder.to_path_buf();
    tokio::task::spawn_blocking(move || load_session(&folder)).await.map_err(internal)?.map_err(internal)
}

/// Write a prompt for each scene that has none (or the scenes named), saving after every scene. A scene that fails is
/// listed in `failures` and the rest go on.
async fn minimax_prompts(state: Arc<AppState>, ctx: JobCtx) -> Result<Value, JobError> {
    let pid = ctx.project_id.clone().ok_or_else(|| fail(ApiError::validation("project_id is required.")))?;
    let folder = resolve_project(&state.roots, &pid).map_err(fail)?;
    let loaded = read_session(&folder).await?;
    let mut segments: Vec<Value> = loaded.session.get("segments").and_then(Value::as_array).cloned().unwrap_or_default();
    if segments.is_empty() {
        return Err(fail(ApiError::validation("The project has no scenes. Create the timeline first.")));
    }
    sort_segments(&mut segments);
    let wanted: Vec<String> = ctx.params.get("scene_ids").and_then(Value::as_array).map(|a| a.iter().map(|v| text_or_empty(Some(v))).collect()).unwrap_or_default();
    let replace = ctx.params.get("replace_existing").and_then(Value::as_bool).unwrap_or(false);
    let limit = ctx.params.get("limit").and_then(Value::as_u64).unwrap_or(0) as usize;
    let mut targets: Vec<usize> = (0..segments.len())
        .filter(|i| {
            let id = text_or_empty(segments[*i].get("id"));
            let named = wanted.is_empty() || wanted.contains(&id) || wanted.contains(&(i + 1).to_string());
            named && (replace || !wanted.is_empty() || text_or_empty(segments[*i].get("minimax_h3_prompt")).trim().is_empty())
        })
        .collect();
    if limit > 0 {
        targets.truncate(limit);
    }
    let params = PromptParams { temperature: num(&ctx.params, "temperature", 0.45), top_p: num(&ctx.params, "top_p", 0.92), max_new_tokens: num(&ctx.params, "max_new_tokens", 4000.0) as u32, previous_frame: None };
    let total = targets.len();
    let (mut created, mut failures) = (Vec::new(), Vec::new());
    let mut revision = loaded.revision;
    for (done, index) in targets.iter().enumerate() {
        if ctx.is_cancelled() {
            return Err(JobError::cancelled());
        }
        let id = text_or_empty(segments[*index].get("id"));
        ctx.progress(ProgressUpdate {
            percent: 5.0 + 90.0 * done as f64 / total.max(1) as f64,
            stage: "minimax_prompts".into(),
            message: format!("MiniMax prompts {done}/{total} (scene {})", index + 1),
            scene_id: Some(id.clone()),
            scene_index: Some(done as i64 + 1),
            scene_count: Some(total as i64),
            stage_index: done as i64 + 1,
            stage_count: total as i64,
        });
        // The project is read again for each scene, because the scenes written before it changed it.
        let session = read_session(&folder).await?.session;
        match generate_scene_prompt(&current(&state), &session, *index, &params).await {
            Ok(written) => {
                let mut fields = Obj::new();
                fields.insert("minimax_h3_prompt".into(), json!(written.prompt));
                fields.insert("minimax_h3_prompt_origin".into(), json!("gemma"));
                fields.insert("minimax_h3_mode".into(), json!("reference_to_video"));
                fields.insert("video_prompt_type".into(), json!("rtv"));
                let saved = {
                    let _guard = state.edit_lock.lock().await;
                    let (folder, id) = (folder.clone(), id.clone());
                    tokio::task::spawn_blocking(move || session_ops::set_scene_fields(&folder, &id, &fields, &Stamp::now())).await
                };
                let saved = saved.map_err(internal)?.map_err(internal)?;
                revision = saved["revision"].as_i64().unwrap_or(revision);
                created.push(json!({"scene_id": id, "scene_number": index + 1, "characters": written.characters, "shots": written.shots, "attempts": written.attempts, "model": written.model, "warnings": written.warnings}));
            }
            // The LLM being down fails the whole job. A scene's own problem fails only that scene.
            Err(PromptError::Llm(e @ h3_llm::LlmError::Unavailable(_))) => return Err(fail(ApiError::from(e))),
            Err(e) => failures.push(json!({"scene_id": id, "scene_number": index + 1, "error": e.to_string()})),
        }
    }
    if total > 0 && created.is_empty() {
        let first = failures.first().and_then(|f| f["error"].as_str()).unwrap_or("").to_string();
        let mut error = JobError::new("BATCH_FAILED", format!("No prompt could be written. First error: {first}"));
        error.details = json!({"failures": failures});
        return Err(error);
    }
    Ok(json!({"created": created.len(), "failed": failures.len(), "skipped": segments.len() - total, "prompts": created, "failures": failures, "revision": revision}))
}

/// Describe a character or location picture with the LLM and save the text on the card.
async fn describe_reference(state: Arc<AppState>, ctx: JobCtx) -> Result<Value, JobError> {
    let pid = ctx.project_id.clone().ok_or_else(|| fail(ApiError::validation("project_id is required.")))?;
    let folder = resolve_project(&state.roots, &pid).map_err(fail)?;
    let kind = ctx.params.get("kind").and_then(Value::as_str).unwrap_or("").to_string();
    let card_id = ctx.params.get("ref_id").and_then(Value::as_str).unwrap_or("").to_string();
    let loaded = read_session(&folder).await?;
    let (name, image_path) = h3_project::images::reference_card(&loaded.session, &kind, &card_id)
        .ok_or_else(|| fail(ApiError::validation(format!("Reference '{card_id}' was not found in the Reference Builder."))))?;
    let image = std::path::PathBuf::from(&image_path);
    if image_path.is_empty() || !image.is_file() {
        return Err(fail(ApiError::validation(format!("{} has no image file to describe. Add a picture first.", if name.is_empty() { &card_id } else { &name }))));
    }
    ctx.progress(ProgressUpdate { percent: 20.0, stage: "describing".into(), message: format!("Describing {name}..."), stage_count: 1, ..Default::default() });
    let data_url = tokio::task::spawn_blocking(move || h3_project::images::preview_data_url(&image, 512, 88)).await.map_err(internal)?.map_err(internal)?;
    let reference_type = if kind == "locations" { "location".to_string() } else { Some(field_text(&loaded.session, &kind, &card_id, "reference_type")).filter(|t| !t.is_empty()).unwrap_or_else(|| "character".into()) };
    let (description, model) = h3_llm::describe::describe_reference(&current(&state), &reference_type, &name, &data_url, 1, "").await.map_err(|e| fail(ApiError::from(e)))?;
    let payload: Obj = [("description".to_string(), json!(description))].into_iter().collect();
    let saved = {
        let _guard = state.edit_lock.lock().await;
        let (folder, kind, card_id) = (folder.clone(), kind.clone(), card_id.clone());
        tokio::task::spawn_blocking(move || session_ops::upsert_reference(&folder, &kind, &card_id, &payload, None, &Stamp::now())).await
    };
    let saved = saved.map_err(internal)?.map_err(internal)?;
    Ok(json!({"id": card_id, "kind": kind, "name": name, "description": description, "model": model, "revision": saved["revision"]}))
}

/// First of the keys that has text, like Python's `a or b or c`, stripped.
fn first_text(segment: &Value, keys: &[&str]) -> String {
    keys.iter().filter_map(|k| segment.get(*k)).find(|v| h3_core::pyval::truthy(v)).map(|v| text_or_empty(Some(v)).trim().to_string()).unwrap_or_default()
}

/// The project's theme file and the location notes, as the scout gets them.
fn location_style_theme_text(folder: &std::path::Path, session: &Obj, local_notes: &str) -> String {
    let use_context = session.get("use_vrgdg_text_context").is_none_or(h3_core::pyval::truthy);
    let theme = if use_context { std::fs::read_to_string(folder.join("project_context").join("themestyle.txt")).map(|t| t.trim().to_string()).unwrap_or_default() } else { String::new() };
    let mut parts = Vec::new();
    if !theme.is_empty() {
        parts.push(format!("Global theme/style:\n{theme}"));
    }
    if !local_notes.trim().is_empty() {
        parts.push(format!("Location extraction notes:\n{}", local_notes.trim()));
    }
    parts.join("\n\n").trim().to_string()
}

fn new_location_id(taken: &[String]) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let mut salt = now.subsec_nanos() as u64;
    loop {
        salt = salt.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let id = format!("loc_{}_{}", now.as_millis(), (salt >> 33) % 10000);
        if !taken.contains(&id) {
            return id;
        }
    }
}

/// Ask the LLM for filming locations from the lyrics and scene notes and add them to the Reference Builder.
async fn extract_locations(state: Arc<AppState>, ctx: JobCtx) -> Result<Value, JobError> {
    let pid = ctx.project_id.clone().ok_or_else(|| fail(ApiError::validation("project_id is required.")))?;
    let folder = resolve_project(&state.roots, &pid).map_err(fail)?;
    let loaded = read_session(&folder).await?;
    let session = &loaded.session;
    let segments: Vec<&Value> = session.get("segments").and_then(Value::as_array).map(|a| a.iter().filter(|s| s.is_object()).collect()).unwrap_or_default();
    let (mut lyric_lines, mut planning) = (Vec::new(), Vec::new());
    for segment in &segments {
        let lyric = text_or_empty(segment.get("lyric_text")).trim().to_string();
        if !lyric.is_empty() {
            lyric_lines.push(format!("Scene {}: {lyric}", lyric_lines.len() + 1));
        }
        let concept = first_text(segment, &["t2i_prompt", "flux_prompt", "notes", "flux_notes"]);
        let notes = ["notes", "timeline_note", "i2v_notes"].iter().map(|k| text_or_empty(segment.get(*k)).trim().to_string()).filter(|p| !p.is_empty()).collect::<Vec<_>>().join("\n");
        if !concept.is_empty() || !notes.is_empty() {
            planning.push((concept, notes));
        }
    }
    if lyric_lines.is_empty() && planning.is_empty() {
        return Err(fail(ApiError::validation("LM Extract needs lyrics, scene notes, or concept prompts first. Create the scenes from the lyrics first.")));
    }
    let refs = session.get("flux_reference_builder").cloned().unwrap_or_else(|| json!({}));
    let given = text_or_empty(ctx.params.get("style_theme")).trim().to_string();
    let local_notes = if given.is_empty() { text_or_empty(refs.get("location_style_theme")).trim().to_string() } else { given };
    let input = h3_llm::locations::ScoutInput {
        lyrics_text: lyric_lines.join("\n"),
        scenes: planning,
        style_theme: location_style_theme_text(&folder, session, &local_notes),
        subject_context: h3_llm::locations::subject_context_for_locations(&refs),
        existing_locations: refs.get("locations").and_then(Value::as_array).map(|a| a.iter().map(|l| text_or_empty(l.get("name"))).collect()).unwrap_or_default(),
        max_new_tokens: ctx.params.get("max_new_tokens").and_then(Value::as_u64).map(|n| n as u32),
        ..Default::default()
    };
    ctx.progress(ProgressUpdate { percent: 10.0, stage: "extracting_locations".into(), message: "Asking the LLM location scout...".into(), stage_count: 1, ..Default::default() });
    let found = h3_llm::locations::scout_locations(&current(&state), &input).await.map_err(|e| fail(ApiError::from(e)))?;
    let pairs: Vec<(String, String)> = found.locations.iter().map(|l| (l.name.clone(), l.description.clone())).collect();
    let saved = {
        let _guard = state.edit_lock.lock().await;
        let folder = folder.clone();
        tokio::task::spawn_blocking(move || session_ops::merge_scouted_locations(&folder, &pairs, &local_notes, &mut |taken| new_location_id(taken), None, &Stamp::now())).await
    };
    let mut saved = saved.map_err(internal)?.map_err(internal)?;
    saved["model"] = json!(found.model);
    Ok(saved)
}

fn field_text(session: &Obj, kind: &str, card_id: &str, key: &str) -> String {
    session.get("flux_reference_builder").and_then(|r| r.get(kind)).and_then(Value::as_array).and_then(|a| a.iter().find(|c| c["id"] == card_id)).map(|c| text_or_empty(c.get(key))).unwrap_or_default()
}

// -- image prompts ----------------------------------------------------------------------------------------------------------

/// The writer settings from job params. `mode` or `preset_id` names the image model, `user_notes` adds direction.
pub(crate) fn image_prompt_params(params: &Value) -> h3_pipeline::image_prompts::ImagePromptParams {
    let text = |key: &str| text_or_empty(params.get(key)).trim().to_string();
    let model = [text("mode"), text("preset_id")].into_iter().find(|m| !m.is_empty()).unwrap_or_default();
    h3_pipeline::image_prompts::ImagePromptParams {
        temperature: num(params, "temperature", 0.6),
        top_p: num(params, "top_p", 0.95),
        max_new_tokens: num(params, "max_new_tokens", 1200.0) as u32,
        model,
        extra_notes: text("user_notes"),
    }
}

/// Failures of the writer as job errors. The LLM being down is the usual one.
pub(crate) fn prompt_failure(e: PromptError) -> JobError {
    match e {
        PromptError::Llm(llm) => fail(ApiError::from(llm)),
        other => fail(ApiError::validation(other.to_string())),
    }
}

/// Write a scene's still image prompt and save it as the scene's `t2i_prompt`.
async fn scene_image_prompt(state: Arc<AppState>, ctx: JobCtx) -> Result<Value, JobError> {
    let pid = ctx.project_id.clone().ok_or_else(|| fail(ApiError::validation("project_id is required.")))?;
    let folder = resolve_project(&state.roots, &pid).map_err(fail)?;
    let scene = text_or_empty(ctx.params.get("scene_id")).trim().to_string();
    if scene.is_empty() {
        return Err(fail(ApiError::validation("scene_id is required.")));
    }
    let session = read_session(&folder).await?.session;
    let index = h3_pipeline::image_prompts::sorted_index_of(&session, &scene).ok_or_else(|| fail(ApiError::scene_not_found(&scene, &pid)))?;
    ctx.progress(ProgressUpdate { percent: 20.0, stage: "generating".into(), message: "Writing the image prompt...".into(), scene_id: Some(scene.clone()), stage_count: 1, ..Default::default() });
    let written = h3_pipeline::image_prompts::write_scene_image_prompt(&current(&state), &session, index, &image_prompt_params(&ctx.params)).await.map_err(prompt_failure)?;
    let saved = save_scene_image_prompt(&state, &folder, &scene, &written.prompt).await?;
    Ok(json!({"scene_id": scene, "prompt": written.prompt, "model": written.model, "attempts": written.attempts, "revision": saved["revision"]}))
}

pub(crate) async fn save_scene_image_prompt(state: &AppState, folder: &std::path::Path, scene: &str, prompt: &str) -> Result<Value, JobError> {
    let _guard = state.edit_lock.lock().await;
    let (folder, scene, prompt) = (folder.to_path_buf(), scene.to_string(), prompt.to_string());
    tokio::task::spawn_blocking(move || session_ops::set_scene_prompt(&folder, &scene, "t2i_prompt", &prompt, "llm", None, &Stamp::now())).await.map_err(internal)?.map_err(internal)
}

/// Write the prompt for a character reference sheet or a location image. The prompt is returned and not saved.
async fn reference_image_prompt(state: Arc<AppState>, ctx: JobCtx) -> Result<Value, JobError> {
    let pid = ctx.project_id.clone().ok_or_else(|| fail(ApiError::validation("project_id is required.")))?;
    let folder = resolve_project(&state.roots, &pid).map_err(fail)?;
    let kind = text_or_empty(ctx.params.get("kind")).trim().to_string();
    let card = text_or_empty(ctx.params.get("ref_id")).trim().to_string();
    let session = read_session(&folder).await?.session;
    ctx.progress(ProgressUpdate { percent: 20.0, stage: "generating".into(), message: "Writing the reference image prompt...".into(), stage_count: 1, ..Default::default() });
    let written = h3_pipeline::image_prompts::write_reference_image_prompt(&current(&state), &session, &kind, &card, &image_prompt_params(&ctx.params)).await.map_err(prompt_failure)?;
    Ok(json!({"kind": kind, "ref_id": card, "prompt": written.prompt, "model": written.model, "attempts": written.attempts}))
}

/// Write image prompts for many scenes. Scenes that already have one are skipped with `run_mode: resume_missing`.
async fn batch_prompts(state: Arc<AppState>, ctx: JobCtx) -> Result<Value, JobError> {
    let pid = ctx.project_id.clone().ok_or_else(|| fail(ApiError::validation("project_id is required.")))?;
    let folder = resolve_project(&state.roots, &pid).map_err(fail)?;
    let session = read_session(&folder).await?.session;
    let mut segments: Vec<Value> = session.get("segments").and_then(Value::as_array).cloned().unwrap_or_default();
    sort_segments(&mut segments);
    let wanted: Vec<String> = ctx.params.get("scene_ids").and_then(Value::as_array).map(|a| a.iter().map(|v| text_or_empty(Some(v))).collect()).unwrap_or_default();
    let skip_existing = text_or_empty(ctx.params.get("run_mode")).trim().to_lowercase() == "resume_missing";
    let params = image_prompt_params(&ctx.params);
    let targets: Vec<(usize, String)> = segments
        .iter()
        .enumerate()
        .filter_map(|(i, seg)| {
            let id = text_or_empty(seg.get("id"));
            let id = if id.is_empty() { (i + 1).to_string() } else { id };
            let named = wanted.is_empty() || wanted.contains(&id) || wanted.contains(&(i + 1).to_string());
            (named && !(skip_existing && !text_or_empty(seg.get("t2i_prompt")).trim().is_empty())).then_some((i, id))
        })
        .collect();
    let total = targets.len();
    if total == 0 {
        return Ok(json!({"processed": 0, "message": "No scenes matched batch prompt criteria."}));
    }
    let mut results: Vec<Value> = Vec::new();
    for (n, (index, id)) in targets.iter().enumerate() {
        if ctx.is_cancelled() {
            return Err(JobError::cancelled());
        }
        ctx.progress(ProgressUpdate {
            percent: n as f64 / total as f64 * 100.0,
            stage: "batch_prompts".into(),
            message: format!("Generating image prompt for scene {} of {total} ({id})...", n + 1),
            scene_id: Some(id.clone()),
            scene_index: Some(n as i64 + 1),
            scene_count: Some(total as i64),
            stage_index: n as i64 + 1,
            stage_count: total as i64,
        });
        // The project is read again for each scene, because the scenes written before it changed it.
        let fresh = read_session(&folder).await?.session;
        match h3_pipeline::image_prompts::write_scene_image_prompt(&current(&state), &fresh, *index, &params).await {
            Ok(written) => {
                save_scene_image_prompt(&state, &folder, id, &written.prompt).await?;
                results.push(json!({"scene_id": id, "status": "success", "field": "t2i_prompt"}));
            }
            Err(PromptError::Llm(e @ h3_llm::LlmError::Unavailable(_))) => return Err(fail(ApiError::from(e))),
            Err(e) => results.push(json!({"scene_id": id, "status": "error", "error": e.to_string()})),
        }
    }
    let failed = results.iter().filter(|r| r["status"] == "error").count();
    if failed == results.len() {
        let first = results.iter().find_map(|r| r["error"].as_str()).unwrap_or("").to_string();
        let mut error = JobError::new("BATCH_FAILED", format!("No prompt could be written. First error: {first}"));
        error.details = json!({"results": results});
        return Err(error);
    }
    Ok(json!({"processed": results.len(), "total_targets": total, "failed": failed, "results": results}))
}
