//! Where the overlay library is kept. One folder for every project and for the pack: `models/overlay` of the running ComfyUI. The config's
//! `overlay_library_dir` wins when it is set. The folder found from ComfyUI is written to `overlay_library.json` beside the config file, so it is
//! still used when the app starts while ComfyUI is not running.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};

use crate::app::AppState;

const REMEMBERED: &str = "overlay_library.json";

/// The folder remembered by an earlier run, if there is one.
pub(crate) fn remembered(config_path: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(config_path.parent()?.join(REMEMBERED)).ok()?;
    let value: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    value.get("dir").and_then(Value::as_str).map(str::trim).filter(|d| !d.is_empty()).map(PathBuf::from)
}

fn remember(config_path: &Path, dir: &Path) {
    let Some(parent) = config_path.parent() else { return };
    if let Err(e) = h3_project::atomic::atomic_write_json(&parent.join(REMEMBERED), &json!({"dir": dir.display().to_string()})) {
        eprintln!("[VRGDG-RS API] Could not remember the overlay library folder: {e}");
    }
}

impl AppState {
    /// The overlay library folder when one is known: the config's `overlay_library_dir`, else `models/overlay` of ComfyUI as last seen. `None` leaves the
    /// library in an `overlay` folder beside the config file.
    pub(crate) fn overlay_dir(&self) -> Option<PathBuf> {
        let configured = self.cfg.overlay_library_dir.trim();
        if !configured.is_empty() {
            return Some(PathBuf::from(configured));
        }
        self.overlay_found.read().ok().and_then(|found| found.clone())
    }

    /// Ask the running ComfyUI where its `models` folder is and use `models/overlay` as the overlay library. ComfyUI not running (or not saying) keeps the
    /// last answer. Call this before a call that lists or fills the library.
    pub async fn refresh_overlay_dir(&self) {
        if !self.cfg.overlay_library_dir.trim().is_empty() {
            return;
        }
        let Ok(Some(models)) = tokio::time::timeout(Duration::from_secs(2), h3_pipeline::context::models_dir_of(&self.cfg)).await else { return };
        let dir = models.join("overlay");
        if self.overlay_dir().as_ref() == Some(&dir) {
            return;
        }
        if let Ok(mut found) = self.overlay_found.write() {
            *found = Some(dir.clone());
        }
        remember(&self.config_path, &dir);
    }

    /// [`AppState::builder_env`] after [`AppState::refresh_overlay_dir`], for a call that works with the overlay library.
    pub async fn overlay_env(&self) -> h3_builder::Env {
        self.refresh_overlay_dir().await;
        self.builder_env()
    }
}
