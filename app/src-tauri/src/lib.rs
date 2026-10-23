//! Tauri entry point. Commands here only translate between the UI and the backend crates.

use h3_comfy::{check_nodes, ComfyClient};
use h3_project::AppConfig;
use serde::Serialize;

#[derive(Serialize)]
struct MissingNodeView {
    class_type: String,
    provider: String,
    feature: String,
}

#[derive(Serialize)]
struct ComfyStatus {
    url: String,
    reachable: bool,
    error: Option<String>,
    comfyui_version: String,
    gpus: Vec<String>,
    core_ready: bool,
    missing: Vec<MissingNodeView>,
}

/// Ping ComfyUI and check the H3 nodes.
#[tauri::command]
async fn comfy_status() -> Result<ComfyStatus, String> {
    let cfg = AppConfig::load(&AppConfig::default_path()).map_err(|e| e.to_string())?;
    let client = ComfyClient::new(cfg.comfy_base());
    let mut status = ComfyStatus {
        url: client.base_url().to_string(),
        reachable: false,
        error: None,
        comfyui_version: String::new(),
        gpus: Vec::new(),
        core_ready: false,
        missing: Vec::new(),
    };
    let stats = match client.system_stats().await {
        Ok(s) => s,
        Err(e) => {
            status.error = Some(e.to_string());
            return Ok(status);
        }
    };
    status.reachable = true;
    status.comfyui_version = stats.system.comfyui_version;
    status.gpus = stats
        .devices
        .iter()
        .map(|d| format!("{} ({:.1} GB)", d.name, d.vram_total as f64 / 1e9))
        .collect();
    match client.object_info().await {
        Ok(info) => {
            let report = check_nodes(&info);
            status.core_ready = report.core_ready();
            status.missing = report
                .missing
                .iter()
                .map(|m| MissingNodeView {
                    class_type: m.class_type.to_string(),
                    provider: m.provider.to_string(),
                    feature: m.feature.label().to_string(),
                })
                .collect();
        }
        Err(e) => status.error = Some(e.to_string()),
    }
    Ok(status)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![comfy_status])
        .run(tauri::generate_context!())
        .expect("error while running the VRGDG H3 Studio app");
}
