//! After the render: cut the clip to the scene's exact length, collect it into the project, record it on the scene, and
//! keep the latent bookkeeping right. Also render all and stitch. Port of the second half of `render_scene_video_async`,
//! `run_batch_video_render_job` and `run_video_stitch_job` in `agent_api/orchestrator/video_orchestrator.py`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use h3_comfy::ComfyClient;
use h3_core::pyval::{text_or_empty, truthy, Obj};
use h3_media::collect::{collect_scene_video, Existing};
use h3_media::stitch::{stitch_scene_videos, StitchResult};
use h3_media::video::{apply_color_match, trim_scene_video, TrimRequest};
use h3_project::{latents, load_session, persist_session, takes, Stamp};
use serde_json::{json, Value};

use crate::env::LiveEnv;
use crate::render::{render_scene, RenderError, RenderEvent, RenderOutcome, RenderParams};
use crate::scene_video::{apply_scene_video, continuity_used};
use crate::stitch_plan::build_stitch_request;

/// Options of the steps after the render.
#[derive(Debug, Clone)]
pub struct FinishOptions {
    /// Match the clip's opening color to the previous scene's clip.
    pub match_start_color: bool,
    pub color_match_fade_seconds: f64,
    pub color_match_strength: f64,
}

impl Default for FinishOptions {
    fn default() -> Self {
        Self { match_start_color: false, color_match_fade_seconds: 1.0, color_match_strength: 0.85 }
    }
}

/// A finished scene render.
#[derive(Debug, Clone)]
pub struct SceneResult {
    pub scene_id: String,
    pub scene_number: u32,
    pub video_path: String,
    pub thumbnail_path: String,
    pub backup_path: String,
    pub raw_video_path: String,
    pub history_index: usize,
    pub history_count: usize,
    pub revision: i64,
}

fn other(message: impl std::fmt::Display) -> RenderError {
    RenderError::Other(message.to_string())
}

fn f64_of(value: Option<&Value>) -> f64 {
    value.and_then(h3_core::pyval::to_float).unwrap_or(0.0)
}

/// Cut, collect, color match, record and bookkeep one rendered scene. Blocking: call it on a blocking thread.
pub fn finish_scene(env: &LiveEnv, project: &Path, outcome: &RenderOutcome, options: &FinishOptions) -> Result<SceneResult, RenderError> {
    let scene = outcome.scene_number;
    let raw = PathBuf::from(&outcome.raw_video_path);

    // Like the Video Builder: cut the raw render to the exact timeline length first (label `minimax_exact`, marked as the
    // audio video), then collect that clip as video_NNNN-audio.mp4. Collecting the raw render would keep its extra frames
    // and drift the stitched video off the song.
    let trim = outcome.graph.get("post_render_trim").cloned().unwrap_or(Value::Null);
    let mut source = raw.clone();
    if trim.is_object() {
        let request = TrimRequest {
            project: project.to_path_buf(),
            scene,
            source: raw.clone(),
            start: f64_of(trim.get("start")),
            duration: f64_of(trim.get("duration")),
            frames: f64_of(trim.get("frames")) as u32,
            label: "minimax_exact".into(),
            mark_as_audio_video: true,
            restore_audio: None,
        };
        match trim_scene_video(&env.tools, &request, &Stamp::now().compact) {
            Ok(result) => source = result.video_path,
            Err(e) => eprintln!("[VRGDG-RS Render] Trimming failed for scene {scene}: {e}"),
        }
    }
    let now = Stamp::now();
    let collected = collect_scene_video(&env.tools, project, scene, &source, Existing::Backup, &now.compact, &now).map_err(other)?;
    let final_video = collected.video_path.clone();
    let mut final_thumbnail = collected.thumbnail_path.clone();

    // The previous scene's clip, for the optional opening color match.
    let loaded = load_session(project).map_err(other)?;
    let segments: Vec<Value> = loaded.session.get("segments").and_then(Value::as_array).cloned().unwrap_or_default();
    let index = outcome.scene_index;
    if options.match_start_color && index > 0 {
        let previous = segments.get(index - 1).and_then(Value::as_object);
        let previous_video = previous
            .and_then(|s| ["video_path", "rendered_video_path"].iter().filter_map(|k| s.get(*k)).find(|v| truthy(v)))
            .map(|v| text_or_empty(Some(v)).trim().to_string())
            .unwrap_or_default();
        if !previous_video.is_empty() && Path::new(&previous_video).is_file() {
            let before = takes::video_fingerprint(&final_video);
            let token = format!("{}_{}", Stamp::now().millis(), std::process::id());
            match apply_color_match(&env.tools, &final_video, Path::new(&previous_video), options.color_match_fade_seconds, options.color_match_strength, &token) {
                Ok(result) => {
                    if result.applied {
                        if let Err(e) = takes::rename_video(project, scene, &before, &takes::video_fingerprint(&final_video)) {
                            eprintln!("[VRGDG-RS Latent Takes] Could not keep the scene's latent take after the color match: {e}");
                        }
                    }
                    if result.thumbnail_path.is_some() {
                        final_thumbnail = result.thumbnail_path;
                    }
                }
                Err(e) => eprintln!("[VRGDG-RS Render] Color match failed for scene {scene}: {e}"),
            }
        }
    }

    // Record the clip on the scene, in a session read just now so edits made while the render ran are kept.
    let mut session = load_session(project).map_err(other)?.session;
    let video_text = final_video.display().to_string();
    let thumbnail_text = final_thumbnail.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
    let mut all_segments: Vec<Value> = session.get("segments").and_then(Value::as_array).cloned().unwrap_or_default();
    let Some(segment) = all_segments.get_mut(index).and_then(Value::as_object_mut) else {
        return Err(RenderError::SceneNotFound(outcome.scene_id.clone()));
    };
    apply_scene_video(segment, &video_text, &thumbnail_text, (Stamp::now().epoch * 1000.0) as i64);
    let report = outcome.graph.get("latent_continuation_settings");
    let snapshot: Vec<Value> = segments.clone();
    for (k, v) in continuity_used(&outcome.continuity_mode, report, &snapshot, scene as usize) {
        segment.insert(k, v);
    }
    // The untrimmed render, so a later re-trim can start from the whole take.
    segment.insert("raw_video_path".into(), json!(outcome.raw_video_path));
    let mut raw_history: Vec<String> = segment
        .get("raw_video_history")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|v| text_or_empty(Some(v))).filter(|p| !p.trim().is_empty()).collect())
        .unwrap_or_default();
    if !raw_history.contains(&outcome.raw_video_path) {
        raw_history.push(outcome.raw_video_path.clone());
    }
    segment.insert("raw_video_history".into(), json!(raw_history));
    // A replaced render is kept as a backup the timeline can switch back to.
    if let Some(backup) = &collected.backup_path {
        let backup_text = backup.display().to_string();
        let mut list: Vec<Value> = segment.get("video_backup_paths").and_then(Value::as_array).cloned().unwrap_or_default();
        if !list.iter().any(|v| v.as_str() == Some(backup_text.as_str())) {
            list.push(json!(backup_text));
        }
        segment.insert("video_backup_paths".into(), Value::Array(list));
        if let Some(thumb) = &collected.backup_thumbnail_path {
            let thumb_text = thumb.display().to_string();
            let mut thumbs: Vec<Value> = segment.get("video_backup_thumbnail_paths").and_then(Value::as_array).cloned().unwrap_or_default();
            if !thumbs.iter().any(|v| v.as_str() == Some(thumb_text.as_str())) {
                thumbs.push(json!(thumb_text));
            }
            segment.insert("video_backup_thumbnail_paths".into(), Value::Array(thumbs));
        }
    }
    let history_index = segment.get("video_history_index").and_then(Value::as_u64).unwrap_or(0) as usize;
    let history_count = segment.get("video_history").and_then(Value::as_array).map_or(0, Vec::len);
    session.insert("segments".into(), Value::Array(all_segments));
    let saved = persist_session(project, session, Stamp::now()).map_err(other)?;

    latents::clear_dirty(project, scene);
    latents::mark_successor_dirty(project, scene, &format!("Scene {scene:03} re-rendered"), &Stamp::now());
    Ok(SceneResult {
        scene_id: outcome.scene_id.clone(),
        scene_number: scene,
        video_path: video_text,
        thumbnail_path: thumbnail_text,
        backup_path: collected.backup_path.map(|p| p.display().to_string()).unwrap_or_default(),
        raw_video_path: outcome.raw_video_path.clone(),
        history_index,
        history_count,
        revision: saved.revision,
    })
}

/// Work to do just before a scene renders, such as writing its prompt from the previous scene's final frame. It gets the
/// project folder, the scene reference and whether the render was given a prompt of its own.
pub type SceneHook = Arc<dyn Fn(std::path::PathBuf, String, bool) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>> + Send + Sync>;

/// Everything a run needs: the ComfyUI client, the environment and a flag that cancels it.
#[derive(Clone)]
pub struct RunContext {
    pub client: ComfyClient,
    pub env: Arc<LiveEnv>,
    pub cancel: Arc<AtomicBool>,
    /// Runs before each scene renders. `None` renders the saved project as it is.
    pub before_scene: Option<SceneHook>,
}

/// The MiniMax seed settings. Same list as `SEED_FIELDS` in the pack's `settings_payload.py`.
pub const SEED_FIELDS: [&str; 5] = ["seed", "two_pass_pass1_seed", "two_pass_pass2_seed", "advanced_two_pass_pass1_seed", "advanced_two_pass_pass2_seed"];

/// Give every seed setting of a scene a fresh value in 1..=2147483647 and save it, like the Render All "new seeds"
/// option: into the scene's own settings when they are locked, else into the project's. The render then reads the new
/// seeds from the saved project, so it can be repeated.
pub fn randomize_seeds(env: &LiveEnv, project: &Path, scene_ref: &str) -> Result<(), RenderError> {
    use h3_graph::GraphEnv;
    let loaded = load_session(project).map_err(other)?;
    let mut session = loaded.session;
    let segments = session.get("segments").and_then(Value::as_array).cloned().unwrap_or_default();
    let index = segments
        .iter()
        .position(|s| s.get("id").and_then(Value::as_str) == Some(scene_ref))
        .or_else(|| scene_ref.parse::<usize>().ok().filter(|n| *n >= 1 && *n <= segments.len()).map(|n| n - 1))
        .ok_or_else(|| RenderError::SceneNotFound(scene_ref.to_string()))?;
    let fresh: Obj = SEED_FIELDS.iter().map(|f| (f.to_string(), json!(env.random_seed() % 2_147_483_647 + 1))).collect();
    let locked = segments[index].get("use_scene_minimax_h3_settings").is_some_and(truthy) && segments[index].get("minimax_h3_settings").is_some_and(Value::is_object);
    let target = if locked {
        session.get_mut("segments").and_then(Value::as_array_mut).and_then(|a| a.get_mut(index)).and_then(Value::as_object_mut).and_then(|s| s.get_mut("minimax_h3_settings"))
    } else {
        Some(session.entry("minimax_h3_settings").or_insert_with(|| json!({})))
    };
    match target.and_then(Value::as_object_mut) {
        Some(settings) => settings.extend(fresh),
        None => return Err(other("The project's MiniMax settings are not an object.")),
    }
    persist_session(project, session, Stamp::now()).map_err(other)?;
    Ok(())
}

/// Render one scene and finish it: the whole of "Render" for a scene.
pub async fn render_scene_full(
    ctx: &RunContext,
    project: &Path,
    scene_ref: &str,
    params: RenderParams,
    options: FinishOptions,
    mut on_event: impl FnMut(RenderEvent),
) -> Result<SceneResult, RenderError> {
    let project_path = project.to_path_buf();
    let mut params = params;
    if params.overrides.remove("randomize_seed").is_some_and(|v| truthy(&v)) {
        let (env, path, scene) = (Arc::clone(&ctx.env), project_path.clone(), scene_ref.to_string());
        tokio::task::spawn_blocking(move || randomize_seeds(&env, &path, &scene)).await.map_err(other)??;
    }
    if let Some(hook) = &ctx.before_scene {
        let has_prompt = params.overrides.get("prompt").is_some_and(truthy);
        hook(project_path.clone(), scene_ref.to_string(), has_prompt).await.map_err(other)?;
    }
    let loaded = {
        let path = project_path.clone();
        tokio::task::spawn_blocking(move || load_session(&path)).await.map_err(other)?.map_err(other)?
    };
    let outcome = render_scene(&ctx.client, Arc::clone(&ctx.env), loaded, scene_ref, params, Arc::clone(&ctx.cancel), &mut on_event).await?;
    on_event(RenderEvent { percent: 80.0, stage: "trimming", message: "Trimming the clip to the exact timeline length and collecting it...".into() });
    let env = Arc::clone(&ctx.env);
    let finished = tokio::task::spawn_blocking(move || finish_scene(&env, &project_path, &outcome, &options)).await.map_err(other)??;
    on_event(RenderEvent { percent: 100.0, stage: "completed", message: "Scene video rendered successfully.".into() });
    Ok(finished)
}

/// Which scenes a batch renders.
#[derive(Debug, Clone, Default)]
pub struct BatchOptions {
    /// `all`, `selected`, `range` or `missing`.
    pub scope: String,
    pub scene_ids: Vec<String>,
    pub range: Option<(usize, usize)>,
    pub force: bool,
    /// Skip scenes that already have a clip (a resumed run).
    pub skip_existing: bool,
    pub skip_final_stitch: bool,
    pub params: RenderParams,
    pub finish: FinishOptions,
}

#[derive(Debug, Clone)]
pub struct BatchSceneResult {
    pub scene_id: String,
    pub video_path: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BatchResult {
    pub results: Vec<BatchSceneResult>,
    pub final_video_path: Option<String>,
}

/// The scenes a batch will render, as `(scene id, scene number)`, in timeline order.
pub fn batch_targets(segments: &[Value], options: &BatchOptions) -> Vec<(String, usize)> {
    let scope = if options.scope.trim().is_empty() { "all" } else { options.scope.trim() }.to_lowercase();
    let mut targets = Vec::new();
    for (index, segment) in segments.iter().enumerate() {
        let number = index + 1;
        let id = segment.get("id").filter(|v| truthy(v)).map(|v| text_or_empty(Some(v))).unwrap_or_else(|| number.to_string());
        if scope == "selected" && !options.scene_ids.is_empty() && !options.scene_ids.contains(&id) {
            continue;
        }
        if scope == "range" {
            if let Some((low, high)) = options.range {
                if !(low <= number && number <= high) {
                    continue;
                }
            }
        }
        let has_video = ["video_path", "rendered_video_path"].iter().any(|k| segment.get(*k).is_some_and(truthy));
        if (scope == "missing" || (options.skip_existing && !options.force)) && has_video {
            continue;
        }
        targets.push((id, number));
    }
    targets
}

/// Render every scene the options select, one after the other, then stitch the project unless told not to. A scene that
/// fails does not stop the rest.
pub async fn render_all(ctx: &RunContext, project: &Path, options: BatchOptions, mut on_event: impl FnMut(RenderEvent)) -> Result<BatchResult, RenderError> {
    let loaded = load_session(project).map_err(other)?;
    let segments: Vec<Value> = loaded.session.get("segments").and_then(Value::as_array).cloned().unwrap_or_default();
    let targets = batch_targets(&segments, &options);
    if targets.is_empty() {
        return Ok(BatchResult { results: vec![], final_video_path: None });
    }
    let total = targets.len();
    on_event(RenderEvent { percent: 0.0, stage: "batch_videos", message: format!("Starting batch render for {total} scene(s)...") });
    let mut results = Vec::new();
    for (i, (id, number)) in targets.iter().enumerate() {
        if ctx.cancel.load(Ordering::Relaxed) {
            return Err(RenderError::Comfy(h3_comfy::ComfyError::Cancelled));
        }
        let percent = ((i as f64 / total as f64) * 90.0 * 10.0).round() / 10.0;
        on_event(RenderEvent { percent, stage: "batch_videos", message: format!("Rendering video for scene {} of {total} ({id})...", i + 1) });
        match render_scene_full(ctx, project, &number.to_string(), options.params.clone(), options.finish.clone(), &mut on_event).await {
            Ok(done) => results.push(BatchSceneResult { scene_id: id.clone(), video_path: Some(done.video_path), error: None }),
            Err(RenderError::Comfy(h3_comfy::ComfyError::Cancelled)) => return Err(RenderError::Comfy(h3_comfy::ComfyError::Cancelled)),
            Err(e) => {
                eprintln!("[VRGDG-RS Render] Video render failed for scene {id}: {e}");
                results.push(BatchSceneResult { scene_id: id.clone(), video_path: None, error: Some(e.to_string()) });
            }
        }
    }
    let mut final_video_path = None;
    if !options.skip_final_stitch && options.scope.trim().to_lowercase() != "selected" {
        on_event(RenderEvent { percent: 92.0, stage: "stitching", message: "Stitching final project video...".into() });
        match stitch_project(ctx, project, &Obj::new(), false).await {
            Ok((result, _)) => final_video_path = Some(result.final_video_path.display().to_string()),
            Err(e) => eprintln!("[VRGDG-RS Render] Final video stitch after batch render failed: {e}"),
        }
    }
    on_event(RenderEvent { percent: 100.0, stage: "completed", message: "Batch video render finished.".into() });
    Ok(BatchResult { results, final_video_path })
}

/// Stitch the project's scene clips with the song. `strict` makes a selected scene without a clip an error.
pub async fn stitch_project(
    ctx: &RunContext,
    project: &Path,
    params: &Obj,
    strict: bool,
) -> Result<(StitchResult, crate::stitch_plan::StitchSummary), RenderError> {
    let project = project.to_path_buf();
    let params = params.clone();
    let tools = ctx.env.tools.clone();
    tokio::task::spawn_blocking(move || -> Result<_, RenderError> {
        let loaded = load_session(&project).map_err(other)?;
        let (request, summary) = build_stitch_request(&loaded.project_folder, &loaded.session, &params, strict).map_err(RenderError::Invalid)?;
        let result = stitch_scene_videos(&tools, &request).map_err(other)?;
        Ok((result, summary))
    })
    .await
    .map_err(other)?
}
