//! `h3 scenes` and `h3 render`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use h3_comfy::{ComfyClient, ComfyDirs};
use h3_core::pyval::Obj;
use h3_media::Tools;
use h3_pipeline::{prepare_scene_render, render_scene, LiveEnv, RenderParams};
use h3_project::{load_session, AppConfig};
use serde_json::Value;

pub struct RenderArgs {
    pub project: PathBuf,
    pub scene: String,
    pub set: Vec<String>,
    pub dry_run: bool,
    pub graph_out: Option<PathBuf>,
    pub timeout_hours: Option<f64>,
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
        println!(
            "{:>3}  {}  {:>8.2}s - {:>8.2}s  {}, {}",
            i + 1,
            text("id"),
            number("start"),
            number("end"),
            prompt,
            video
        );
    }
    Ok(())
}

pub async fn render(cfg: &AppConfig, args: RenderArgs) -> Result<()> {
    let overrides = parse_overrides(&args.set)?;
    let tools = Tools::find(&cfg.ffmpeg_path, &cfg.ffprobe_path).map_err(|e| anyhow!("{e}"))?;
    let client = ComfyClient::new(cfg.comfy_base());
    let stats = client.system_stats().await.map_err(|e| anyhow!("{e}"))?;
    let folder_paths = client.folder_paths().await;
    let mut dirs = ComfyDirs::detect(&stats.system.argv, folder_paths.as_ref())
        .ok_or_else(|| anyhow!("ComfyUI did not say where it keeps its files. Set comfy_output_dir in the config."))?;
    if !cfg.comfy_output_dir.trim().is_empty() {
        dirs.output = cfg.comfy_output_dir.trim().to_string();
    }
    let object_info = client.object_info().await.map_err(|e| anyhow!("{e}"))?;
    let env = Arc::new(LiveEnv::new(object_info, dirs, tools));
    let loaded = load_session(&args.project).with_context(|| format!("opening {}", args.project.display()))?;
    let params = RenderParams { overrides, timeout_hours: args.timeout_hours };

    if args.dry_run {
        let env2 = Arc::clone(&env);
        let scene = args.scene.clone();
        let params2 = params.clone();
        let (prepared, result) = tokio::task::spawn_blocking(move || -> Result<_> {
            let prepared = prepare_scene_render(&*env2, &loaded, &scene, &params2).map_err(|e| anyhow!("{e}"))?;
            let result = h3_graph::build(&*env2, prepared.workflow_key, &prepared.payload).map_err(|e| anyhow!("{e}"))?;
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

    let cancel = Arc::new(AtomicBool::new(false));
    let watcher = Arc::clone(&cancel);
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            eprintln!("[VRGDG-RS] Cancelling...");
            watcher.store(true, Ordering::Relaxed);
        }
    });
    let outcome = render_scene(&client, Arc::clone(&env), loaded, &args.scene, params, cancel, |event| {
        println!("[{:>5.1}%] {}: {}", event.percent, event.stage, event.message);
    })
    .await
    .map_err(|e| anyhow!("{e}"))?;
    println!("prompt id: {}", outcome.prompt_id);
    println!("raw clip: {}", outcome.raw_video_path);
    for key in ["used_seed", "post_render_trim"] {
        if let Some(v) = outcome.graph.get(key) {
            println!("{key}: {v}");
        }
    }
    if let Some(path) = &args.graph_out {
        std::fs::write(path, h3_core::pyjson::dumps_file(&outcome.graph_nodes))?;
    }
    Ok(())
}
