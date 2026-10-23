//! Connect to ComfyUI and ffmpeg and build the [`RunContext`] a render needs.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use h3_comfy::{ComfyClient, ComfyDirs};
use h3_media::Tools;
use h3_project::AppConfig;

use crate::env::LiveEnv;
use crate::finish::RunContext;

/// ComfyUI, ffmpeg and the folders, ready to run a render. Reads `/object_info` once, so build a new context per run.
pub async fn connect(cfg: &AppConfig) -> Result<RunContext, String> {
    let tools = Tools::find(&cfg.ffmpeg_path, &cfg.ffprobe_path).map_err(|e| e.to_string())?;
    let client = ComfyClient::new(cfg.comfy_base());
    let stats = client.system_stats().await.map_err(|e| e.to_string())?;
    let folder_paths = client.folder_paths().await;
    let mut dirs = ComfyDirs::detect(&stats.system.argv, folder_paths.as_ref())
        .ok_or_else(|| "ComfyUI did not say where it keeps its files. Set comfy_output_dir in the config.".to_string())?;
    if !cfg.comfy_output_dir.trim().is_empty() {
        dirs.output = cfg.comfy_output_dir.trim().to_string();
    }
    let object_info = client.object_info().await.map_err(|e| e.to_string())?;
    Ok(RunContext { client, env: Arc::new(LiveEnv::new(object_info, dirs, tools)), cancel: Arc::new(AtomicBool::new(false)) })
}
