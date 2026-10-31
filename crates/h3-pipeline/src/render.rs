//! Render one scene. Port of the MiniMax part of `render_scene_video_async` in
//! `agent_api/orchestrator/video_orchestrator.py`: settings, scene inputs, prompt, audio, graph, ComfyUI run and the
//! clip it made. Trimming to the exact timeline length, collecting the clip into the project and recording it on the
//! scene come with the media phase.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use h3_comfy::{extract_videos, ComfyClient, ComfyError};
use h3_core::continuity::{canonical_continuity_mode, MASKED};
use h3_core::pyval::{text_or_empty, to_float, truthy, Obj};
use h3_core::scene::{resolve_scene_inputs, SceneInputsArgs};
use h3_core::settings::{build_render_payload, settings_for_scene, workflow_key};
use h3_graph::{build, GraphEnv};
use h3_project::{latents, LoadedSession};
use serde_json::{json, Value};

use crate::env::LiveEnv;

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("Scene '{0}' was not found in this project.")]
    SceneNotFound(String),
    /// A problem with the scene's inputs or settings that the user can fix.
    #[error("{0}")]
    Invalid(String),
    #[error("Latent continuation for Scene {scene:03} requires Scene {predecessor:03} latent, but it was not found. Render Scene {predecessor:03} first.")]
    PredecessorMissing { scene: u32, predecessor: u32 },
    #[error("Latent for Scene {stale:03} is stale (dirty) and must be re-rendered before Scene {scene:03}.")]
    LatentStale { scene: u32, stale: u32 },
    #[error(transparent)]
    Graph(#[from] h3_graph::GraphError),
    #[error(transparent)]
    Comfy(#[from] ComfyError),
    #[error("{0}")]
    Other(String),
}

/// What the caller can change for one render. Keys in `overrides` replace payload keys, like the API's render params.
#[derive(Debug, Clone, Default)]
pub struct RenderParams {
    pub overrides: Obj,
    pub timeout_hours: Option<f64>,
}

/// Progress the render reports.
#[derive(Debug, Clone)]
pub struct RenderEvent {
    pub percent: f64,
    pub stage: &'static str,
    pub message: String,
    /// The id of the scene being rendered, when the event is about one.
    pub scene_id: Option<String>,
}

impl RenderEvent {
    /// An event that is not about one scene (a batch starting, the stitch).
    pub fn new(percent: f64, stage: &'static str, message: impl Into<String>) -> Self {
        Self { percent, stage, message: message.into(), scene_id: None }
    }

    /// An event about the scene with this id.
    pub fn for_scene(percent: f64, stage: &'static str, message: impl Into<String>, scene_id: &str) -> Self {
        Self { percent, stage, message: message.into(), scene_id: Some(scene_id.to_string()) }
    }
}

/// The id of the scene a reference names: an id, or the scene's number as text.
pub fn scene_id_of(loaded: &LoadedSession, scene_ref: &str) -> Option<String> {
    let segments = loaded.session.get("segments").and_then(Value::as_array)?;
    segments
        .iter()
        .enumerate()
        .find(|(i, s)| s.get("id").and_then(Value::as_str) == Some(scene_ref) || (i + 1).to_string() == scene_ref)
        .and_then(|(_, s)| s.get("id").and_then(Value::as_str).map(str::to_string))
}

/// A scene ready to become a graph.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub scene_index: usize,
    pub scene_number: u32,
    pub scene_id: String,
    pub workflow_key: &'static str,
    pub payload: Obj,
    /// An enabled Audio Mask renders with its masked mix. The finished clip gets this real audio back: file and start second.
    pub restore_audio: Option<(PathBuf, f64)>,
}

/// What a render produced.
#[derive(Debug, Clone)]
pub struct RenderOutcome {
    pub scene_index: usize,
    pub scene_number: u32,
    pub scene_id: String,
    /// The continuity mode the payload asked for (`off` or `latent_continuation_masked`).
    pub continuity_mode: String,
    pub prompt_id: String,
    /// The raw clip ComfyUI saved. It is longer than the scene (warm-up, alignment padding) until it is trimmed.
    pub raw_video_path: String,
    pub output_folder: String,
    /// Timing plan, exact trim, seed and the other summaries the graph builder reported.
    pub graph: Obj,
    pub graph_nodes: Value,
    pub restore_audio: Option<(PathBuf, f64)>,
}

fn segment_text(segment: &Obj, key: &str) -> String {
    text_or_empty(segment.get(key)).trim().to_string()
}

fn aspect_alias(raw: &str) -> String {
    match raw {
        "16:9" => "16:9 (Widescreen)",
        "9:16" => "9:16 (Portrait Widescreen)",
        "1:1" => "1:1 (Square)",
        "4:3" => "4:3 (Standard)",
        "3:4" => "3:4 (Portrait Standard)",
        "3:2" => "3:2 (Photo)",
        "2:3" => "2:3 (Portrait Photo)",
        "21:9" => "21:9 (Ultrawide)",
        other => other,
    }
    .to_string()
}

/// Everything before the graph: the scene's settings, inputs, prompt and audio as one render payload.
pub fn prepare_scene_render(env: &dyn GraphEnv, loaded: &LoadedSession, scene_ref: &str, params: &RenderParams) -> Result<Prepared, RenderError> {
    let session = Value::Object(loaded.session.clone());
    let segments: Vec<Value> = loaded.session.get("segments").and_then(Value::as_array).cloned().unwrap_or_default();
    let index = segments
        .iter()
        .enumerate()
        .position(|(i, s)| s.get("id").and_then(Value::as_str) == Some(scene_ref) || (i + 1).to_string() == scene_ref)
        .ok_or_else(|| RenderError::SceneNotFound(scene_ref.to_string()))?;
    let segment: Obj = segments[index].as_object().cloned().unwrap_or_default();
    let scene_number = index as u32 + 1;
    let folder = loaded.project_folder.display().to_string();
    let p = &params.overrides;

    let start = to_float(segment.get("start").unwrap_or(&Value::Null)).unwrap_or(0.0);
    let end = segment.get("end").filter(|v| truthy(v)).and_then(to_float).unwrap_or(start + 4.0);
    let duration = (end - start).max(0.25);

    let settings = settings_for_scene(&session, Some(&segments[index]));
    let key = match p.get("mode").and_then(Value::as_str).map(|m| m.trim().to_lowercase()) {
        Some(mode) if !matches!(mode.as_str(), "minimax_h3" | "minimax") && mode.starts_with("minimax") => match mode.as_str() {
            "minimax_h3_2pass" => "minimax_h3_2pass",
            "minimax_h3_advanced_2pass" => "minimax_h3_advanced_2pass",
            other => return Err(RenderError::Invalid(format!("Video mode '{other}' is not available in this app."))),
        },
        _ => workflow_key(&settings),
    };
    let mut payload = build_render_payload(&settings, None).map_err(|e| RenderError::Invalid(e.message))?;
    let pass2_prompt = segment_text(&segment, "minimax_h3_pass2_prompt");
    if !pass2_prompt.is_empty() {
        payload.insert("pass2_prompt".into(), json!(pass2_prompt));
    }
    let refmod = settings.get("pipeline").and_then(Value::as_str) == Some("refmod");

    // The scene inputs the UI resolves per render: reference images, continuity, reference videos and the last frame.
    let continuity_mode = match p.get("continuity_mode") {
        Some(v) if truthy(v) => v.clone(),
        _ => settings.get("continuity_mode").filter(|v| truthy(v)).cloned().unwrap_or_else(|| json!("off")),
    };
    let configured_images: Option<Vec<Value>> = p.get("image_paths").and_then(Value::as_array).cloned();
    let configured_videos: Option<Vec<Value>> = p.get("video_references").and_then(Value::as_array).cloned();
    let render_pass = settings.get("render_pass").and_then(Value::as_str).unwrap_or("single").to_string();
    if refmod {
        // The RefMod pipeline renders from saved RefMods in scene order, with no images.
        let items: Vec<Obj> = h3_core::refmod::refmod_items_for_scene(&session, &segment, index).into_iter().filter(|i| i.get("strength").and_then(Value::as_f64).unwrap_or(0.0) > 0.0).collect();
        let items = h3_core::refmod::assign_labels(&items, false);
        if items.is_empty() {
            return Err(RenderError::Invalid(format!("Scene {scene_number} needs at least one RefMod. Pick a RefMod on a Reference Builder card and map it to the scene.")));
        }
        let wanted = text_or_empty(p.get("continuity_mode").filter(|v| truthy(v))).trim().to_string();
        let continuity = if wanted.is_empty() { text_or_empty(settings.get("continuity_mode").filter(|v| truthy(v))).trim().to_string() } else { wanted };
        let continuity = if continuity.is_empty() { "off".to_string() } else { continuity };
        payload.insert("pipeline".into(), json!("refmod"));
        payload.insert("refmod_references".into(), Value::Array(h3_core::refmod::reference_payload(&items)));
        payload.insert("continuity_mode".into(), json!(if canonical_continuity_mode(Some(&json!(continuity))) == Some(MASKED) { MASKED } else { "off" }));
        payload.insert("image_paths".into(), json!([]));
        payload.insert("video_references".into(), json!([]));
    }
    // The RefMod pipeline sends no images, so it has no Reference Builder pictures to look for or check (the pack does the same in
    // `agent_api/orchestrator/video_orchestrator.py`). Its payload was filled above.
    if !refmod {
        let args = SceneInputsArgs {
            continuity_mode: &continuity_mode,
            has_previous_segment: index > 0,
            scene_number: i64::from(scene_number),
            configured_image_paths: configured_images.as_deref(),
            configured_video_references: configured_videos.as_deref(),
            render_pass: &render_pass,
        };
        let video_mode = settings.get("video_mode").and_then(Value::as_str).unwrap_or("reference_to_video");
        let inputs = resolve_scene_inputs(&session, &segment, video_mode, index, &args, &|path| Path::new(path).is_file())
            .map_err(|e| RenderError::Invalid(format!("Scene {scene_number}: {}", e.message)))?;
        let missing: Vec<String> = inputs
            .get("missing_image_paths")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        if !missing.is_empty() {
            return Err(RenderError::Invalid(format!("Scene {scene_number} references image files that do not exist: {}", missing.join(", "))));
        }
        for key in ["continuity_mode", "image_paths", "video_references"] {
            payload.insert(key.into(), inputs.get(key).cloned().unwrap_or(Value::Null));
        }
        if let Some(last) = inputs.get("last_frame_path") {
            payload.insert("last_frame_path".into(), last.clone());
        }
    }
    for (k, v) in p {
        payload.insert(k.clone(), v.clone());
    }
    payload.insert("project_folder".into(), json!(folder));
    payload.insert("scene_number".into(), json!(scene_number));
    payload.insert("start".into(), json!(start));
    payload.insert("end".into(), json!(end));
    payload.insert("duration".into(), json!(duration));

    // A scene continued from its predecessor needs the predecessor's latent, and it must be current.
    let continuity = text_or_empty(payload.get("continuity_mode")).trim().to_lowercase();
    if canonical_continuity_mode(Some(&json!(continuity))) == Some(MASKED) && scene_number > 1 {
        let pred = scene_number - 1;
        let project = loaded.project_folder.as_path();
        if !env.latent_exists(&folder, pred) {
            return Err(RenderError::PredecessorMissing { scene: scene_number, predecessor: pred });
        }
        if latents::is_dirty(project, pred) {
            return Err(RenderError::LatentStale { scene: scene_number, stale: pred });
        }
    }
    if let Some(raw) = payload.get("aspect_ratio").and_then(Value::as_str).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        payload.insert("aspect_ratio".into(), json!(aspect_alias(&raw)));
    }

    // The Video Builder renders the saved MiniMax prompt, falling back to the video prompt.
    let prompt_text = ["prompt"]
        .iter()
        .filter_map(|k| p.get(*k))
        .find(|v| truthy(v))
        .map(|v| text_or_empty(Some(v)).trim().to_string())
        .or_else(|| ["minimax_h3_prompt", "i2v_prompt"].iter().map(|k| segment_text(&segment, k)).find(|t| !t.is_empty()))
        .unwrap_or_default();
    if prompt_text.is_empty() {
        return Err(RenderError::Invalid(format!("Scene {scene_number} needs a MiniMax H3 prompt before it can render.")));
    }
    for key in ["i2v_prompt", "t2v_prompt", "prompt"] {
        payload.insert(key.into(), json!(prompt_text));
    }

    // The song the scene is cut from.
    let audio = match p.get("audio_path") {
        Some(v) if truthy(v) => text_or_empty(Some(v)).trim().to_string(),
        _ => text_or_empty(loaded.session.get("audio_path")).trim().to_string(),
    };
    // An enabled Audio Mask renders with its masked mix (kept vocals plus the music). The finished clip gets the real audio back.
    let mut restore_audio = None;
    let built_in = text_or_empty(payload.get("audio_mode").filter(|v| truthy(v))).trim() == "built_in_audio";
    let mut audio = audio;
    // Built-in audio is generated with the picture: no song goes to the graph.
    if built_in {
        audio.clear();
    }
    if !audio.is_empty() {
        let masked = crate::audio_mask::audio_override_for_scene(&segment, &loaded.project_folder, duration)
            .map_err(|e| RenderError::Invalid(format!("Scene {scene_number}: {e}")))?;
        if let Some(masked) = masked {
            restore_audio = Some((PathBuf::from(&audio), start));
            audio = masked.path.display().to_string();
            payload.insert("source_start_seconds".into(), json!(0.0));
            payload.insert("source_duration_seconds".into(), json!(masked.duration_seconds));
        }
    }
    payload.insert("audio_path".into(), json!(audio));
    Ok(Prepared {
        scene_index: index,
        scene_number,
        scene_id: segment.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
        workflow_key: key,
        payload,
        restore_audio,
    })
}

/// The scene's clip when ComfyUI's history does not name it: the newest final clip in the scene's output folder.
fn find_output_in_folder(folder: &str, scene_number: u32, since: SystemTime) -> Option<String> {
    let needle = format!("MiniMaxH3_scene_{scene_number:04}");
    let mut found: Vec<(SystemTime, String)> = std::fs::read_dir(folder)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let lower = name.to_lowercase();
            if !lower.ends_with(".mp4") || !name.contains(&needle) || lower.contains("_stage1") {
                return None;
            }
            let modified = entry.metadata().ok()?.modified().ok()?;
            (modified + Duration::from_secs(2) >= since).then(|| (modified, entry.path().display().to_string()))
        })
        .collect();
    found.sort();
    found.pop().map(|(_, path)| path)
}

/// Render one scene in ComfyUI and find the clip it made. Blocking work (ffmpeg, ffprobe, files) runs on the blocking
/// thread pool.
pub async fn render_scene(
    client: &ComfyClient,
    env: Arc<LiveEnv>,
    loaded: LoadedSession,
    scene_ref: &str,
    params: RenderParams,
    cancel: Arc<AtomicBool>,
    mut on_event: impl FnMut(RenderEvent),
) -> Result<RenderOutcome, RenderError> {
    // Every event names its scene, so a window can show which one is rendering. A reference that names no scene fails in the preparation below.
    let scene_id = scene_id_of(&loaded, scene_ref);
    let mut emit = |percent: f64, stage: &'static str, message: String| on_event(RenderEvent { percent, stage, message, scene_id: scene_id.clone() });
    emit(5.0, "preparing", format!("Preparing scene {scene_ref}..."));
    let (prepared, graph_result) = {
        let env = Arc::clone(&env);
        let scene_ref = scene_ref.to_string();
        let params = params.clone();
        tokio::task::spawn_blocking(move || -> Result<(Prepared, Obj), RenderError> {
            let prepared = prepare_scene_render(&*env, &loaded, &scene_ref, &params)?;
            let result = build(&*env, prepared.workflow_key, &prepared.payload)?;
            Ok((prepared, result))
        })
        .await
        .map_err(|e| RenderError::Other(e.to_string()))??
    };
    emit(15.0, "compiling_graph", "Compiled the ComfyUI video graph.".into());
    let graph = graph_result.get("prompt").cloned().unwrap_or(Value::Null);
    let started = SystemTime::now();
    // The progress socket is opened first, under the id the prompt is queued with, so no step is missed. Without it only the clock moves.
    let client_id = h3_comfy::unique_client_id();
    let feed = client.progress_feed(&client_id).await;
    let queued = client.queue_prompt_as(&graph, &client_id).await?;
    emit(30.0, "rendering", format!("Rendering with ComfyUI (prompt {})...", queued.prompt_id));
    let timeout = Duration::from_secs_f64((params.timeout_hours.unwrap_or(1.0) * 3600.0).max(60.0));
    // 30% is the graph handed to ComfyUI and 75% the clip found, so the render itself fills what lies between.
    let history = client
        .wait_for_prompt_live(&queued.prompt_id, &graph, feed.as_ref(), timeout, Duration::from_secs(1), &cancel, |live, elapsed| {
            emit(30.0 + 45.0 * live.fraction, "rendering", live.describe("Rendering with ComfyUI", elapsed));
        })
        .await?;
    drop(feed);
    emit(75.0, "collecting", "Locating the rendered clip...".into());

    // The final clip is saved by node 142. The 2 Pass Advanced backup comes from another node and is never the answer.
    let videos = extract_videos(&history, &queued.prompt_id, Some("142"));
    let output_folder = graph_result.get("output_folder").and_then(Value::as_str).unwrap_or("").to_string();
    let mut raw = videos
        .last()
        .map(|v| env.dirs.resolve_output(v))
        .filter(|p| Path::new(p).is_file())
        .unwrap_or_default();
    if raw.is_empty() {
        raw = find_output_in_folder(&output_folder, prepared.scene_number, started).unwrap_or_default();
    }
    if raw.is_empty() {
        return Err(RenderError::Comfy(ComfyError::Execution(format!(
            "Video generation completed but the output video was not found for prompt '{}'.",
            queued.prompt_id
        ))));
    }
    emit(100.0, "rendered", format!("Rendered {raw}"));
    let continuity_mode = text_or_empty(prepared.payload.get("continuity_mode")).trim().to_string();
    Ok(RenderOutcome {
        scene_index: prepared.scene_index,
        scene_number: prepared.scene_number,
        scene_id: prepared.scene_id.clone(),
        continuity_mode,
        prompt_id: queued.prompt_id,
        raw_video_path: raw,
        output_folder,
        graph: graph_result.clone().into_iter().filter(|(k, _)| k != "prompt").collect(),
        graph_nodes: graph,
        restore_audio: prepared.restore_audio.clone(),
    })
}
