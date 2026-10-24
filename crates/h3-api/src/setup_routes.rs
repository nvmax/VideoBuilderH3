//! First-run setup: what is ready, what is not and how to fix it, and saving the paths the app needs. `GET /setup/status`
//! checks the saved settings, `POST /setup/check` checks settings that are not saved yet, `PUT /setup/config` saves them.
//! Saved settings take effect after a restart, because the running API holds them.

use std::path::{Path as FsPath, PathBuf};
use std::time::Duration;

use axum::extract::State;
use axum::Json;
use h3_comfy::{check_nodes, ComfyClient, Feature};
use h3_media::Tools;
use h3_project::AppConfig;
use serde_json::{json, Value};

use crate::app::{blocking, body_object, Shared};
use crate::error::{success, ApiError, ApiResult};

/// The settings the setup can change.
const SETTINGS: [&str; 5] = ["comfy_url", "project_root", "ffmpeg_path", "ffprobe_path", "comfy_output_dir"];

fn check(id: &str, label: &str, level: &str, detail: impl Into<String>, fix: &str) -> Value {
    json!({"id": id, "label": label, "level": level, "ok": level != "fail", "detail": detail.into(), "fix": fix})
}

/// The saved config with any of the sent settings applied.
fn with_overrides(base: &AppConfig, over: &serde_json::Map<String, Value>) -> Result<AppConfig, ApiError> {
    let mut cfg = base.clone();
    for (key, value) in over {
        if !SETTINGS.contains(&key.as_str()) {
            return Err(ApiError::validation(format!("Unknown setting '{key}'. Settings: {}.", SETTINGS.join(", "))));
        }
        let Some(text) = value.as_str() else {
            return Err(ApiError::validation(format!("{key} must be text.")));
        };
        let text = text.trim().to_string();
        match key.as_str() {
            "comfy_url" => {
                if !(text.starts_with("http://") || text.starts_with("https://")) {
                    return Err(ApiError::validation("comfy_url must start with http:// or https://, for example http://127.0.0.1:8188."));
                }
                cfg.comfy_url = text;
            }
            "project_root" => {
                if text.is_empty() {
                    return Err(ApiError::validation("project_root cannot be empty."));
                }
                cfg.project_root = PathBuf::from(text);
            }
            "ffmpeg_path" => cfg.ffmpeg_path = if text.is_empty() { "ffmpeg".into() } else { text },
            "ffprobe_path" => cfg.ffprobe_path = if text.is_empty() { "ffprobe".into() } else { text },
            _ => cfg.comfy_output_dir = text,
        }
    }
    cfg.llm.comfy_url = cfg.comfy_url.clone();
    Ok(cfg)
}

fn config_view(cfg: &AppConfig) -> Value {
    json!({
        "comfy_url": cfg.comfy_url,
        "project_root": cfg.project_root.display().to_string(),
        "ffmpeg_path": cfg.ffmpeg_path,
        "ffprobe_path": cfg.ffprobe_path,
        "comfy_output_dir": cfg.comfy_output_dir,
        "api_bind": cfg.api_bind,
    })
}

/// Is the project folder usable: an existing folder that can be written to, or one that can be created.
fn project_folder_check(root: &FsPath) -> Value {
    let label = "Project folder";
    if root.exists() {
        if !root.is_dir() {
            return check("project_folder", label, "fail", format!("{} is a file, not a folder.", root.display()), "Choose a folder.");
        }
        let probe = root.join(".h3-write-test");
        return match std::fs::write(&probe, b"x") {
            Ok(()) => {
                let _ = std::fs::remove_file(&probe);
                check("project_folder", label, "ok", root.display().to_string(), "")
            }
            Err(e) => check("project_folder", label, "fail", format!("{} cannot be written to: {e}", root.display()), "Choose a folder you can write to."),
        };
    }
    let mut parent = root.parent();
    while let Some(p) = parent {
        if p.exists() {
            return check("project_folder", label, "ok", format!("{} (will be created)", root.display()), "");
        }
        parent = p.parent();
    }
    check("project_folder", label, "fail", format!("{} cannot be created.", root.display()), "Choose a folder on an existing drive.")
}

fn ffmpeg_check(cfg: &AppConfig) -> Vec<Value> {
    let found = Tools::find(&cfg.ffmpeg_path, &cfg.ffprobe_path);
    match found {
        Err(e) => vec![check("ffmpeg", "ffmpeg and ffprobe", "fail", e.to_string(), "Install ffmpeg and ffprobe, or set their paths.")],
        Ok(tools) => {
            let info = tools.describe();
            let mut out = vec![check("ffmpeg", "ffmpeg and ffprobe", "ok", format!("{} ({})", info.version, info.source), "")];
            if !info.has_libx264 {
                out.push(check(
                    "ffmpeg_h264",
                    "H.264 software encoder",
                    "fail",
                    "This ffmpeg has no libx264, which trimming and stitching use.",
                    "Use an ffmpeg build with libx264 (a GPL build), or set the path to one.",
                ));
            }
            out
        }
    }
}

async fn comfy_checks(cfg: &AppConfig) -> Vec<Value> {
    let client = ComfyClient::new(cfg.comfy_base());
    let stats = tokio::time::timeout(Duration::from_secs(5), client.system_stats()).await;
    let Ok(Ok(stats)) = stats else {
        let why = match stats {
            Ok(Err(e)) => e.to_string(),
            _ => "no answer in 5 seconds".into(),
        };
        return vec![
            check("comfyui", "ComfyUI", "fail", format!("{} is not reachable ({why}).", cfg.comfy_base()), "Start ComfyUI and check the address."),
            check("nodes", "ComfyUI nodes", "skip", "Needs a running ComfyUI.", ""),
            check("output_folder", "ComfyUI output folder", "skip", "Needs a running ComfyUI.", ""),
        ];
    };
    let gpu = stats.devices.first().map(|d| format!(", {} ({} GB)", d.name, d.vram_total / 1_073_741_824)).unwrap_or_default();
    let mut out = vec![check("comfyui", "ComfyUI", "ok", format!("version {}{gpu}", stats.system.comfyui_version), "")];

    match tokio::time::timeout(Duration::from_secs(30), client.object_info()).await {
        Ok(Ok(info)) => {
            let report = check_nodes(&info);
            if report.missing.is_empty() {
                out.push(check("nodes", "ComfyUI nodes", "ok", format!("All required node classes are installed ({} nodes in total).", report.installed_count), ""));
            } else {
                let mut by_provider: Vec<(&str, Vec<&str>)> = Vec::new();
                for m in &report.missing {
                    match by_provider.iter_mut().find(|(p, _)| *p == m.provider) {
                        Some((_, list)) => list.push(m.class_type),
                        None => by_provider.push((m.provider, vec![m.class_type])),
                    }
                }
                let detail = by_provider.iter().map(|(p, nodes)| format!("{p}: {} missing ({})", nodes.len(), nodes.join(", "))).collect::<Vec<_>>().join("; ");
                let features: Vec<&str> = report.missing.iter().map(|m| m.feature).filter(|f| *f != Feature::Core).map(Feature::label).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
                if report.core_ready() {
                    out.push(check("nodes", "ComfyUI nodes", "warn", format!("Optional features are not available ({}). {detail}", features.join(", ")), "Install the node packs listed to use those features."));
                } else {
                    out.push(check("nodes", "ComfyUI nodes", "fail", detail, "Install the missing node packs in ComfyUI and restart it."));
                }
            }
        }
        _ => out.push(check("nodes", "ComfyUI nodes", "warn", "ComfyUI did not return its node list.", "Try again in a moment.")),
    }

    match h3_pipeline::context::comfy_dirs(cfg).await {
        Ok(dirs) if FsPath::new(&dirs.output).is_dir() => out.push(check("output_folder", "ComfyUI output folder", "ok", dirs.output, "")),
        Ok(dirs) => out.push(check("output_folder", "ComfyUI output folder", "fail", format!("{} does not exist on this computer.", dirs.output), "The app must run on the same computer as ComfyUI. Set the output folder if it is somewhere else.")),
        Err(e) => out.push(check("output_folder", "ComfyUI output folder", "warn", e, "Set the ComfyUI output folder.")),
    }
    out
}

async fn llm_check(cfg: &AppConfig) -> Value {
    let runner = cfg.llm.runner.clone();
    match tokio::time::timeout(Duration::from_secs(8), h3_llm::describe_active(&cfg.llm)).await {
        Ok(Ok(info)) => check("llm", "LLM for prompt writing", "ok", format!("{} with model {}", info.runner, info.model), ""),
        Ok(Err(e)) => check("llm", "LLM for prompt writing", "warn", format!("{runner}: {e}"), "Prompts are written by an LLM. Start LM Studio, or set another runner in the LLM settings. Rendering does not need it."),
        Err(_) => check("llm", "LLM for prompt writing", "warn", format!("{runner}: no answer in 8 seconds"), "Check the LLM settings. Rendering does not need it."),
    }
}

async fn report(state: &crate::app::AppState, cfg: AppConfig) -> Value {
    let first_run = !state.config_path.exists();
    let mut checks = comfy_checks(&cfg).await;
    checks.push(project_folder_check(&cfg.project_root));
    let c = cfg.clone();
    checks.extend(blocking(move || ffmpeg_check(&c)).await.unwrap_or_default());
    checks.push(llm_check(&cfg).await);
    let ready = checks.iter().all(|c| c["level"] != "fail");
    json!({"first_run": first_run, "config": config_view(&cfg), "config_path": state.config_path.display().to_string(), "checks": checks, "ready": ready})
}

/// `GET /setup/status`: the saved settings, checked.
pub async fn status(State(state): Shared) -> ApiResult {
    Ok(success(report(&state, state.cfg.clone()).await, None))
}

/// `POST /setup/check`: settings that are not saved yet, checked.
pub async fn check_settings(State(state): Shared, body: Option<Json<Value>>) -> ApiResult {
    let cfg = with_overrides(&state.cfg, &body_object(body)?)?;
    Ok(success(report(&state, cfg).await, None))
}

/// `PUT /setup/config`: save the settings to the config file. They are used after the app restarts.
pub async fn save(State(state): Shared, body: Option<Json<Value>>) -> ApiResult {
    let sent = body_object(body)?;
    if sent.is_empty() {
        return Err(ApiError::validation(format!("Send the settings to change: {}.", SETTINGS.join(", "))));
    }
    // The file is read again, because the LLM settings in it change while the app runs.
    let path = state.config_path.clone();
    let saved = blocking(move || -> Result<AppConfig, ApiError> {
        let on_disk = AppConfig::load(&path).map_err(|e| ApiError::internal(e.to_string()))?;
        let cfg = with_overrides(&on_disk, &sent)?;
        cfg.save(&path).map_err(|e| ApiError::internal(e.to_string()))?;
        Ok(cfg)
    })
    .await??;
    Ok(success(json!({"saved": true, "restart_required": true, "config": config_view(&saved)}), None))
}
