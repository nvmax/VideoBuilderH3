//! Lyric timing and scenes from timed lines. Port of `agent_api/orchestrator/lyrics_orchestrator.py`.
//!
//! `lyrics.align` runs the pack's Stable-ts timestamp workflow in ComfyUI on the song and saves the timed lines.
//! `timeline.from_lines` turns timed lines into scenes, then applies the minimum and maximum scene length.

#![allow(clippy::result_large_err)]

use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::Json;
use h3_core::lyric_scenes::DEFAULT_INSTRUMENTAL_TEXT;
use h3_core::pyval::{round_to, text_or_empty, truthy, Obj};
use h3_core::timed_lines::{build_scenes_from_payload, parse_timestamped_lyrics_output, BuildParams};
use h3_jobs::{Handler, JobCtx, JobError, ProgressUpdate};
use h3_project::{atomic_write_json, load_session, persist_session, Stamp};
use serde_json::{json, Value};

use crate::app::{body_object, AppState, Shared};
use crate::edit_routes::new_scene_id;
use crate::error::{ApiError, ApiResult};
use crate::job_handlers::{context_for, fail};
use crate::views::resolve_project;

pub const LYRICS_ALIGN: &str = "lyrics.align";
pub const TIMELINE_FROM_LINES: &str = "timeline.from_lines";

const PAYLOAD_FILE: &str = "timestamped_lyrics.json";
const SEGMENT_MODES: [&str; 4] = ["whisper_chunks", "reference_lines", "exact_reference_lines", "reference_stanzas"];
const REFERENCE_UNIT_MODES: [&str; 3] = ["reference_lines", "exact_reference_lines", "reference_stanzas"];

type Run = fn(Arc<AppState>, JobCtx) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, JobError>> + Send>>;

pub fn register(state: &Arc<AppState>) {
    let table: [(&str, Run); 2] = [(LYRICS_ALIGN, |s, c| Box::pin(align_job(s, c))), (TIMELINE_FROM_LINES, |s, c| Box::pin(from_lines_job(s, c)))];
    for (kind, run) in table {
        let owned = Arc::clone(state);
        let handler: Handler = Arc::new(move |ctx| run(Arc::clone(&owned), ctx));
        state.jobs.register(kind, handler);
    }
}

/// Time the project lyrics against the song, as a job.
pub async fn align_route(State(state): Shared, Path(pid): Path<String>, body: Option<Json<Value>>) -> ApiResult {
    submit(&state, LYRICS_ALIGN, &pid, body).await
}

/// Create the timeline scenes from the lyrics, as a job.
pub async fn from_lines_route(State(state): Shared, Path(pid): Path<String>, body: Option<Json<Value>>) -> ApiResult {
    submit(&state, TIMELINE_FROM_LINES, &pid, body).await
}

async fn submit(state: &AppState, kind: &str, pid: &str, body: Option<Json<Value>>) -> ApiResult {
    let params = body_object(body)?;
    crate::app::load(state, pid).await?;
    let job = state.jobs.submit(kind, Some(pid), Value::Object(params), true)?;
    Ok(crate::app::job_response(&job))
}

fn validation(message: impl Into<String>) -> JobError {
    fail(ApiError::validation(message))
}

fn internal(e: impl std::fmt::Display) -> JobError {
    fail(ApiError::internal(e.to_string()))
}

fn payload_path(folder: &FsPath) -> PathBuf {
    folder.join("project_context").join(PAYLOAD_FILE)
}

fn flag(params: &Value, key: &str, default: bool) -> bool {
    params.get(key).map_or(default, truthy)
}

fn number(params: &Value, key: &str, default: f64) -> f64 {
    params.get(key).and_then(Value::as_f64).unwrap_or(default)
}

fn session_audio_path(session: &Obj) -> String {
    let candidates: Vec<String> = ["audio_path", "audio_file"].iter().map(|k| text_or_empty(session.get(*k)).trim().to_string()).filter(|p| !p.is_empty()).collect();
    candidates.iter().find(|p| FsPath::new(p).is_file()).or(candidates.first()).cloned().unwrap_or_default()
}

/// The pasted lyrics: the Line Mapping text, else the project's saved lyrics file.
fn project_reference_lyrics(session: &Obj, folder: &FsPath) -> String {
    let mapper = session.get("lyric_mapper").and_then(Value::as_object);
    let from_mapper = mapper.map(|m| text_or_empty(m.get("source_text"))).filter(|t| truthy(&json!(t))).unwrap_or_default();
    let first = if from_mapper.is_empty() { text_or_empty(session.get("canonical_lyrics")) } else { from_mapper };
    if !first.trim().is_empty() {
        return first.trim().to_string();
    }
    for candidate in [folder.join("project_context").join("full_lyrics.txt"), folder.join("full_lyrics.txt")] {
        if let Ok(text) = std::fs::read_to_string(&candidate) {
            if !text.trim().is_empty() {
                return text.trim().to_string();
            }
        }
    }
    String::new()
}

/// What the timestamp workflow and the scene builder both need, checked.
fn alignment_request(session: &Obj, folder: &FsPath, params: &Value) -> Result<Obj, JobError> {
    let given = text_or_empty(params.get("audio_path")).trim().trim_matches('"').to_string();
    let audio = if given.is_empty() { session_audio_path(session) } else { given };
    if audio.is_empty() || !FsPath::new(&audio).is_file() {
        return Err(validation("The project has no audio file to time the lyrics against. Attach audio first."));
    }
    let given_lyrics = text_or_empty(params.get("reference_lyrics")).trim().to_string();
    let lyrics = if given_lyrics.is_empty() { project_reference_lyrics(session, folder) } else { given_lyrics };
    let mode = Some(text_or_empty(params.get("segment_mode")).trim().to_lowercase()).filter(|m| !m.is_empty()).unwrap_or_else(|| "reference_lines".into());
    if !SEGMENT_MODES.contains(&mode.as_str()) {
        return Err(validation(format!("segment_mode must be one of: {}.", SEGMENT_MODES.join(", "))));
    }
    if mode != "whisper_chunks" && lyrics.is_empty() {
        return Err(validation("Reference lyrics are needed for this segment mode. Set the project lyrics first."));
    }
    let text = |key: &str, default: &str| Some(text_or_empty(params.get(key))).filter(|t| !t.is_empty()).unwrap_or_else(|| default.to_string());
    let mut request = Obj::new();
    request.insert("audio_path".into(), json!(audio));
    request.insert("reference_lyrics".into(), json!(lyrics));
    request.insert("language".into(), json!(text("language", "english")));
    request.insert("segment_mode".into(), json!(mode));
    request.insert("include_instrumental_gaps".into(), json!(flag(params, "include_instrumental_gaps", true)));
    request.insert("instrumental_text".into(), json!(text("instrumental_text", DEFAULT_INSTRUMENTAL_TEXT)));
    request.insert("min_gap_seconds".into(), json!(number(params, "min_gap_seconds", 2.0)));
    request.insert("min_scene_seconds".into(), json!(number(params, "min_scene_seconds", 1.0)));
    request.insert("max_scene_seconds".into(), json!(number(params, "max_scene_seconds", 8.0)));
    request.insert("vocal_tail_padding_seconds".into(), json!(number(params, "vocal_tail_padding_seconds", 0.6)));
    request.insert("model_name".into(), json!(text("model_name", "large-v3")));
    Ok(request)
}

async fn read_session(folder: &FsPath) -> Result<h3_project::LoadedSession, JobError> {
    let folder = folder.to_path_buf();
    tokio::task::spawn_blocking(move || load_session(&folder)).await.map_err(internal)?.map_err(internal)
}

/// Time the lyrics against the song with the ComfyUI workflow and save the result. Returns the saved payload too.
async fn align(state: &AppState, ctx: &JobCtx, params: &Value) -> Result<(Value, Obj), JobError> {
    let (run, folder) = context_for(state, ctx).await?;
    let loaded = read_session(&folder).await?;
    let request = alignment_request(&loaded.session, &folder, params)?;
    ctx.progress(ProgressUpdate { percent: 5.0, stage: "building_workflow".into(), message: "Building the timestamp workflow...".into(), stage_count: 1, ..Default::default() });
    let graph = h3_graph::timestamp::timestamp_graph(&request).map_err(|e| validation(e.to_string()))?;
    if let Some(known) = run.env.object_info.as_object().filter(|m| !m.is_empty()) {
        let missing: Vec<&str> = ["VRGDG_TimestampedLyricsExtractor", "VRGDG_GetStems", "VRGDG_ShowText"].into_iter().filter(|c| !known.contains_key(*c)).collect();
        if !missing.is_empty() {
            return Err(fail(ApiError::new(axum::http::StatusCode::SERVICE_UNAVAILABLE, "COMFY_NODES_MISSING", format!("ComfyUI is missing these nodes: {}. Install or update the VRGDG node pack.", missing.join(", ")))));
        }
    }
    let graph = Value::Object(graph);
    // The progress socket is opened first, under the id the prompt is queued with, so no step is missed. Without it only the clock moves.
    let client_id = h3_comfy::unique_client_id();
    let feed = run.client.progress_feed(&client_id).await;
    let queued = run.client.queue_prompt_as(&graph, &client_id).await.map_err(|e| fail(ApiError::from(e)))?;
    ctx.set_prompt_id(&queued.prompt_id);
    ctx.progress(ProgressUpdate { percent: 15.0, stage: "aligning_lyrics".into(), message: "Timing the lyrics against the song...".into(), stage_count: 1, ..Default::default() });
    // 15% is the graph handed to ComfyUI and 70% the timing read back, so the work itself fills what lies between.
    let mut last_message = String::new();
    let history = run
        .client
        .wait_for_prompt_live(&queued.prompt_id, &graph, feed.as_ref(), Duration::from_secs(45 * 60), Duration::from_millis(500), &ctx.cancel, |live, elapsed| {
            let message = live.describe("Timing the lyrics against the song", elapsed);
            if message != last_message {
                ctx.progress(ProgressUpdate { percent: 15.0 + 55.0 * live.fraction, stage: "aligning_lyrics".into(), message: message.clone(), stage_count: 1, ..Default::default() });
                last_message = message;
            }
        })
        .await
        .map_err(|e| fail(ApiError::from(e)))?;
    drop(feed);
    let texts = h3_comfy::extract_texts(&history, &queued.prompt_id);
    let mut payload = parse_timestamped_lyrics_output(&texts.join("\n")).map_err(validation)?;
    payload.entry("segment_mode").or_insert_with(|| request["segment_mode"].clone());

    let path = payload_path(&folder);
    {
        let (path, value) = (path.clone(), Value::Object(payload.clone()));
        tokio::task::spawn_blocking(move || {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            atomic_write_json(&path, &value)
        })
        .await
        .map_err(internal)?
        .map_err(internal)?;
    }
    let mut shown = request.clone();
    shown.shift_remove("reference_lyrics");
    shown.shift_remove("audio_path");
    let result = json!({
        "payload_path": path.display().to_string(),
        "lines": payload.get("segments").and_then(Value::as_array).map_or(0, Vec::len),
        "duration": payload.get("duration").cloned().unwrap_or(Value::Null),
        "segment_mode": payload.get("segment_mode").cloned().unwrap_or(Value::Null),
        "request": shown,
    });
    Ok((result, payload))
}

async fn align_job(state: Arc<AppState>, ctx: JobCtx) -> Result<Value, JobError> {
    Ok(align(&state, &ctx, &ctx.params.clone()).await?.0)
}

fn has_scene_media(segment: &Value) -> bool {
    ["approved_image_path", "custom_image_path", "video_path", "rendered_video_path"].iter().any(|k| !text_or_empty(segment.get(*k)).trim().is_empty())
        || segment.get("image_history").is_some_and(truthy)
        || segment.get("video_history").is_some_and(truthy)
}

/// Create the project's scenes from timed lyric lines, timing the song first unless a saved result is reused.
async fn from_lines_job(state: Arc<AppState>, ctx: JobCtx) -> Result<Value, JobError> {
    let params = ctx.params.clone();
    let pid = ctx.project_id.clone().ok_or_else(|| validation("project_id is required."))?;
    let folder = resolve_project(&state.roots, &pid).map_err(fail)?;
    let loaded = read_session(&folder).await?;
    let existing: Vec<&Value> = loaded.session.get("segments").and_then(Value::as_array).map(|a| a.iter().filter(|s| s.is_object()).collect()).unwrap_or_default();
    if !existing.is_empty() && !flag(&params, "replace_existing", false) {
        return Err(validation(format!("The project already has {} scenes. Pass replace_existing=true to rebuild them from the lyrics.", existing.len())));
    }
    if existing.iter().any(|s| has_scene_media(s)) {
        return Err(validation("Some scenes already have images or videos. Delete those first; rebuilding would orphan them."));
    }
    let request = alignment_request(&loaded.session, &folder, &params)?;
    let saved = payload_path(&folder);
    let payload: Obj = if flag(&params, "use_saved_alignment", false) && saved.is_file() {
        ctx.progress(ProgressUpdate { percent: 50.0, stage: "using_saved_alignment".into(), message: "Using the saved lyric timing...".into(), stage_count: 1, ..Default::default() });
        let text = std::fs::read_to_string(&saved).map_err(internal)?;
        serde_json::from_str::<Obj>(&text).map_err(internal)?
    } else {
        align(&state, &ctx, &params).await?.1
    };
    if ctx.is_cancelled() {
        return Err(JobError::cancelled());
    }
    ctx.progress(ProgressUpdate { percent: 80.0, stage: "creating_scenes".into(), message: "Creating timeline scenes...".into(), stage_count: 1, ..Default::default() });

    let mode = text_or_empty(request.get("segment_mode"));
    let enforce = flag(&params, "enforce_lengths", true);
    let build = BuildParams {
        segment_mode: mode.clone(),
        instrumental_text: text_or_empty(request.get("instrumental_text")),
        min_scene_seconds: request["min_scene_seconds"].as_f64().unwrap_or(1.0),
        max_scene_seconds: request["max_scene_seconds"].as_f64().unwrap_or(8.0),
        min_gap_seconds: request["min_gap_seconds"].as_f64().unwrap_or(2.0),
        include_instrumental_gaps: request["include_instrumental_gaps"].as_bool().unwrap_or(true),
        enforce_lengths: enforce,
    };
    let lyrics = text_or_empty(request.get("reference_lyrics"));
    let scenes = {
        let lyrics = lyrics.clone();
        tokio::task::spawn_blocking(move || build_scenes_from_payload(&payload.clone(), &build, &lyrics, &mut new_scene_id).map(|s| (s, payload)))
            .await
            .map_err(internal)?
            .map_err(validation)?
    };
    let (scenes, payload) = scenes;

    let revision = {
        let _guard = state.edit_lock.lock().await;
        let folder = folder.clone();
        let scenes = scenes.clone();
        let duration = payload.get("duration").and_then(Value::as_f64).unwrap_or(0.0);
        tokio::task::spawn_blocking(move || -> std::io::Result<i64> {
            let mut session = load_session(&folder)?.session;
            session.insert("segments".into(), Value::Array(scenes));
            session.insert("overlay_segments".into(), json!([]));
            session.insert("lyric_mapper".into(), json!({"source_text": lyrics, "lines": []}));
            // The Video Builder turns the lyric lane on once lyrics are placed on the timeline.
            session.insert("show_timeline_lyric_notes".into(), json!(true));
            if duration > 0.0 {
                let current = session.get("audio_duration").filter(|v| truthy(v)).and_then(Value::as_f64).unwrap_or(0.0);
                session.insert("audio_duration".into(), json!(current.max(duration)));
            }
            Ok(persist_session(&folder, session, Stamp::now())?.revision)
        })
        .await
        .map_err(internal)?
        .map_err(internal)?
    };

    let lengths: Vec<f64> = scenes.iter().map(|s| round_to(s["end"].as_f64().unwrap_or(0.0) - s["start"].as_f64().unwrap_or(0.0), 2)).collect();
    let shortest = lengths.iter().copied().fold(f64::INFINITY, f64::min);
    let longest = lengths.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    ctx.progress(ProgressUpdate { percent: 100.0, stage: "completed".into(), message: format!("Created {} scenes.", scenes.len()), stage_count: 1, ..Default::default() });
    Ok(json!({
        "scenes": scenes.len(),
        "shortest_seconds": shortest,
        "longest_seconds": longest,
        "segment_mode": mode,
        "preserved_reference_units": REFERENCE_UNIT_MODES.contains(&mode.as_str()) && !enforce,
        "revision": revision,
    }))
}
