//! `h3 scenes`, `h3 render`, `h3 render-all` and `h3 stitch`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use h3_comfy::{ComfyClient, ComfyDirs};
use h3_core::pyval::Obj;
use h3_media::Tools;
use h3_pipeline::{
    prepare_scene_render, render_all, render_scene, render_scene_full, stitch_project, BatchOptions, FinishOptions, LiveEnv, RenderEvent,
    RenderParams, RunContext,
};
use h3_project::{load_session, AppConfig};
use serde_json::Value;

pub struct RenderArgs {
    pub project: PathBuf,
    pub scene: String,
    pub set: Vec<String>,
    pub dry_run: bool,
    pub raw_only: bool,
    pub match_start_color: bool,
    pub graph_out: Option<PathBuf>,
    pub timeout_hours: Option<f64>,
}

pub struct BatchArgs {
    pub project: PathBuf,
    pub scope: String,
    pub scenes: Vec<String>,
    pub range: Option<String>,
    pub force: bool,
    pub skip_stitch: bool,
    pub set: Vec<String>,
    pub match_start_color: bool,
    pub timeout_hours: Option<f64>,
}

pub struct StitchArgs {
    pub project: PathBuf,
    pub scenes: Vec<String>,
    pub audio: String,
    pub audio_path: Option<PathBuf>,
    pub prefix: Option<String>,
}

/// `key=value` pairs into payload overrides. A value that parses as JSON keeps its type (`seed=7` is a number).
fn parse_overrides(pairs: &[String]) -> Result<Obj> {
    let mut map = Obj::new();
    for pair in pairs {
        let (key, value) = pair.split_once('=').ok_or_else(|| anyhow!("--set needs KEY=VALUE, got '{pair}'"))?;
        let parsed = serde_json::from_str::<Value>(value).unwrap_or_else(|_| Value::String(value.to_string()));
        map.insert(key.trim().to_string(), parsed);
    }
    Ok(map)
}

pub fn scenes(project: &Path) -> Result<()> {
    let loaded = load_session(project).with_context(|| format!("opening {}", project.display()))?;
    let segments = loaded.session.get("segments").and_then(Value::as_array).cloned().unwrap_or_default();
    println!("{} (revision {}), {} scenes", loaded.project_folder.display(), loaded.revision, segments.len());
    for (i, segment) in segments.iter().enumerate() {
        let text = |key: &str| segment.get(key).and_then(Value::as_str).unwrap_or("");
        let number = |key: &str| segment.get(key).and_then(Value::as_f64).unwrap_or(0.0);
        let prompt = if text("minimax_h3_prompt").is_empty() { "no prompt" } else { "prompt" };
        let video = if text("video_path").is_empty() { "no video" } else { "video" };
        println!("{:>3}  {}  {:>8.2}s - {:>8.2}s  {}, {}", i + 1, text("id"), number("start"), number("end"), prompt, video);
    }
    Ok(())
}

/// ComfyUI, ffmpeg and the folders, ready to run a render. Ctrl+C cancels the run.
async fn make_context(cfg: &AppConfig) -> Result<RunContext> {
    let ctx = h3_pipeline::connect(cfg).await.map_err(|e| anyhow!("{e}"))?;
    let watcher = Arc::clone(&ctx.cancel);
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            eprintln!("[VRGDG-RS] Cancelling...");
            watcher.store(true, Ordering::Relaxed);
        }
    });
    Ok(ctx)
}

fn print_event(event: RenderEvent) {
    // The render reports about once a second while ComfyUI works. Its line is printed every 15 seconds, any other event at once.
    static LAST_LIVE: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
    if event.stage == "rendering" {
        let mut last = LAST_LIVE.lock().unwrap_or_else(|e| e.into_inner());
        if last.is_some_and(|at| at.elapsed() < std::time::Duration::from_secs(15)) {
            return;
        }
        *last = Some(std::time::Instant::now());
    }
    println!("[{:>5.1}%] {}: {}", event.percent, event.stage, event.message);
}

pub async fn render(cfg: &AppConfig, args: RenderArgs) -> Result<()> {
    let overrides = parse_overrides(&args.set)?;
    let ctx = make_context(cfg).await?;
    let loaded = load_session(&args.project).with_context(|| format!("opening {}", args.project.display()))?;
    let params = RenderParams { overrides, timeout_hours: args.timeout_hours };

    if args.dry_run {
        let env = Arc::clone(&ctx.env);
        let scene = args.scene.clone();
        let params2 = params.clone();
        let (prepared, result) = tokio::task::spawn_blocking(move || -> Result<_> {
            let prepared = prepare_scene_render(&*env, &loaded, &scene, &params2).map_err(|e| anyhow!("{e}"))?;
            let result = h3_graph::build(&*env, prepared.workflow_key, &prepared.payload).map_err(|e| anyhow!("{e}"))?;
            Ok((prepared, result))
        })
        .await??;
        println!("scene {} ({}), workflow {}", prepared.scene_number, prepared.scene_id, prepared.workflow_key);
        let graph = result.get("prompt").cloned().unwrap_or(Value::Null);
        println!("graph: {} nodes", graph.as_object().map_or(0, |g| g.len()));
        for key in ["used_seed", "timing", "post_render_trim", "output_folder", "latent_continuation_settings"] {
            if let Some(v) = result.get(key) {
                println!("{key}: {v}");
            }
        }
        if let Some(path) = &args.graph_out {
            std::fs::write(path, h3_core::pyjson::dumps_file(&graph))?;
            println!("graph written to {}", path.display());
        }
        return Ok(());
    }

    if args.raw_only {
        let outcome = render_scene(&ctx.client, Arc::clone(&ctx.env), loaded, &args.scene, params, Arc::clone(&ctx.cancel), print_event)
            .await
            .map_err(|e| anyhow!("{e}"))?;
        println!("prompt id: {}", outcome.prompt_id);
        println!("raw clip: {}", outcome.raw_video_path);
        if let Some(path) = &args.graph_out {
            std::fs::write(path, h3_core::pyjson::dumps_file(&outcome.graph_nodes))?;
        }
        return Ok(());
    }
    let options = FinishOptions { match_start_color: args.match_start_color, ..FinishOptions::default() };
    let done = render_scene_full(&ctx, &args.project, &args.scene, params, options, print_event).await.map_err(|e| anyhow!("{e}"))?;
    println!("scene {} ({})", done.scene_number, done.scene_id);
    println!("clip: {}", done.video_path);
    println!("raw render: {}", done.raw_video_path);
    if !done.backup_path.is_empty() {
        println!("previous clip kept as: {}", done.backup_path);
    }
    println!("history: {} of {}, project revision {}", done.history_index + 1, done.history_count, done.revision);
    Ok(())
}

pub async fn render_all_cmd(cfg: &AppConfig, args: BatchArgs) -> Result<()> {
    let ctx = make_context(cfg).await?;
    let range = match &args.range {
        None => None,
        Some(text) => {
            let (a, b) = text.split_once('-').ok_or_else(|| anyhow!("--range needs FIRST-LAST, for example 3-8"))?;
            Some((a.trim().parse()?, b.trim().parse()?))
        }
    };
    let scope = if !args.scenes.is_empty() {
        "selected".to_string()
    } else if range.is_some() {
        "range".to_string()
    } else {
        args.scope.clone()
    };
    let options = BatchOptions {
        scope,
        scene_ids: args.scenes.clone(),
        range,
        force: args.force,
        skip_existing: false,
        skip_final_stitch: args.skip_stitch,
        params: RenderParams { overrides: parse_overrides(&args.set)?, timeout_hours: args.timeout_hours },
        finish: FinishOptions { match_start_color: args.match_start_color, ..FinishOptions::default() },
    };
    let result = render_all(&ctx, &args.project, options, print_event).await.map_err(|e| anyhow!("{e}"))?;
    for r in &result.results {
        match (&r.video_path, &r.error) {
            (Some(path), _) => println!("ok    {}: {path}", r.scene_id),
            (_, Some(error)) => println!("fail  {}: {error}", r.scene_id),
            _ => {}
        }
    }
    if let Some(path) = &result.final_video_path {
        println!("final video: {path}");
    }
    let failed = result.results.iter().filter(|r| r.error.is_some()).count();
    if failed > 0 {
        return Err(anyhow!("{failed} scene(s) failed"));
    }
    Ok(())
}

pub async fn stitch(cfg: &AppConfig, args: StitchArgs) -> Result<()> {
    let tools = Tools::find(&cfg.ffmpeg_path, &cfg.ffprobe_path).map_err(|e| anyhow!("{e}"))?;
    // A stitch needs no ComfyUI, only ffmpeg.
    let env = Arc::new(LiveEnv::new(Value::Null, ComfyDirs { output: String::new(), input: String::new(), temp: String::new() }, tools));
    let ctx = RunContext { client: ComfyClient::new(cfg.comfy_base()), env, cancel: Arc::new(AtomicBool::new(false)), before_scene: None };
    let mut params = Obj::new();
    if !args.scenes.is_empty() {
        params.insert("scene_ids".into(), Value::Array(args.scenes.iter().map(|s| s.parse::<i64>().map(Value::from).unwrap_or_else(|_| Value::String(s.clone()))).collect()));
    }
    params.insert("audio".into(), Value::String(args.audio));
    if let Some(path) = &args.audio_path {
        params.insert("audio_path".into(), Value::String(path.display().to_string()));
    }
    if let Some(prefix) = &args.prefix {
        params.insert("output_prefix".into(), Value::String(prefix.clone()));
    }
    let (result, summary) = stitch_project(&ctx, &args.project, &params, true).await.map_err(|e| anyhow!("{e}"))?;
    println!("scenes: {:?}, audio: {}", summary.scene_numbers, summary.audio);
    println!("final video: {}", result.final_video_path.display());
    println!(
        "{}x{}, {:.3}s video, {:.3}s audio, {} frames on the timeline{}",
        result.output_width,
        result.output_height,
        result.output_duration,
        result.output_audio_duration,
        result.timeline_frame_count,
        if result.embedded_audio_mode.is_empty() { String::new() } else { format!(", embedded audio mode {}", result.embedded_audio_mode) }
    );
    Ok(())
}
