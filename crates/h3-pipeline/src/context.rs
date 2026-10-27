//! Connect to ComfyUI and ffmpeg and build the [`RunContext`] a render needs.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use h3_comfy::{ComfyClient, ComfyDirs};
use h3_media::Tools;
use h3_project::AppConfig;

use crate::env::LiveEnv;
use crate::finish::RunContext;

/// ComfyUI's output, input and temp folders. `comfy_output_dir` in the config wins for the output folder, so the folders
/// are known without ComfyUI running only when it is set.
pub async fn comfy_dirs(cfg: &AppConfig) -> Result<ComfyDirs, String> {
    let client = ComfyClient::new(cfg.comfy_base());
    let detected = match client.system_stats().await {
        Ok(stats) => {
            let folder_paths = client.folder_paths().await;
            ComfyDirs::detect(&stats.system.argv, folder_paths.as_ref())
        }
        Err(e) => {
            if cfg.comfy_output_dir.trim().is_empty() {
                return Err(e.to_string());
            }
            None
        }
    };
    let mut dirs = match detected {
        Some(dirs) => dirs,
        None if !cfg.comfy_output_dir.trim().is_empty() => ComfyDirs { output: cfg.comfy_output_dir.trim().to_string(), input: String::new(), temp: String::new() },
        None => return Err("ComfyUI did not say where it keeps its files. Set comfy_output_dir in the config.".to_string()),
    };
    if !cfg.comfy_output_dir.trim().is_empty() {
        dirs.output = cfg.comfy_output_dir.trim().to_string();
    }
    Ok(dirs)
}

/// ComfyUI's `models` folder: the parent named `models` of the first checkpoint, LoRA, diffusion model, VAE or UNet folder it reports.
/// `None` when ComfyUI reports none (a setup that keeps all of those somewhere else, for example through `extra_model_paths.yaml`).
pub fn models_dir(folder_paths: Option<&serde_json::Value>) -> Option<std::path::PathBuf> {
    use std::path::PathBuf;
    let paths_of = |key: &str| -> Vec<PathBuf> {
        folder_paths.and_then(|f| f.get(key)).and_then(serde_json::Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str()).map(PathBuf::from).collect()).unwrap_or_default()
    };
    ["checkpoints", "loras", "diffusion_models", "vae", "unet"].iter().flat_map(|k| paths_of(k)).find_map(|p| p.parent().filter(|d| d.file_name().is_some_and(|n| n == "models")).map(PathBuf::from))
}

/// `models` of the running ComfyUI. `None` when ComfyUI is not reachable or does not say.
pub async fn models_dir_of(cfg: &AppConfig) -> Option<std::path::PathBuf> {
    let client = ComfyClient::new(cfg.comfy_base());
    models_dir(client.folder_paths().await.as_ref())
}

/// Folders the saved RefMods are read from: the ones ComfyUI registers for `refmods`, `models/refmods` and the
/// `ComfyUI-MiniMaxH3Mod/mods` folder of the MiniMaxH3Mod node pack. Only folders that exist are kept.
pub fn refmod_dirs(folder_paths: Option<&serde_json::Value>) -> Vec<std::path::PathBuf> {
    use std::path::PathBuf;
    let paths_of = |key: &str| -> Vec<PathBuf> {
        folder_paths.and_then(|f| f.get(key)).and_then(serde_json::Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str()).map(PathBuf::from).collect()).unwrap_or_default()
    };
    let mut dirs = paths_of("refmods");
    if let Some(models) = models_dir(folder_paths) {
        dirs.push(models.join("refmods"));
    }
    if let Some(custom_nodes) = paths_of("custom_nodes").first() {
        dirs.push(custom_nodes.join("ComfyUI-MiniMaxH3Mod").join("mods"));
    }
    let mut seen: Vec<PathBuf> = Vec::new();
    for dir in dirs {
        if dir.is_dir() && !seen.contains(&dir) {
            seen.push(dir);
        }
    }
    seen
}

/// The RefMod folders of the running ComfyUI. Empty when ComfyUI is not reachable.
pub async fn refmod_dirs_of(cfg: &AppConfig) -> Vec<std::path::PathBuf> {
    let client = ComfyClient::new(cfg.comfy_base());
    refmod_dirs(client.folder_paths().await.as_ref())
}

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
    Ok(RunContext { client, env: Arc::new(LiveEnv::new(object_info, dirs, tools).with_refmod_dirs(refmod_dirs(folder_paths.as_ref()))), cancel: Arc::new(AtomicBool::new(false)), before_scene: None, after_stitch: None })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    #[test]
    fn models_folder_is_the_parent_named_models() {
        // ComfyUI also lists output sub folders for these keys, which must not win.
        let reported = json!({"checkpoints": ["D:/elsewhere/output/checkpoints", "C:/ComfyUI/models/checkpoints"], "vae": ["C:/ComfyUI/models/vae"]});
        assert_eq!(models_dir(Some(&reported)), Some(PathBuf::from("C:/ComfyUI/models")));
        let moved = json!({"checkpoints": ["D:/stash/checkpoints"], "loras": ["E:/loras"]});
        assert_eq!(models_dir(Some(&moved)), None);
        assert_eq!(models_dir(None), None);
    }
}
