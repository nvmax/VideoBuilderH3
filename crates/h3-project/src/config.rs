//! App config stored in `%APPDATA%\VRGDG-H3\config.json`. Independent of the pack's settings.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::atomic::atomic_write_json;

/// User settings for the app. Missing keys fall back to defaults so older files keep loading.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AppConfig {
    /// Base URL of the running ComfyUI, without a trailing slash.
    pub comfy_url: String,
    /// Folder that holds the user's projects.
    pub project_root: PathBuf,
    /// ffmpeg executable. A bare name is looked up on PATH.
    pub ffmpeg_path: String,
    /// ffprobe executable. A bare name is looked up on PATH.
    pub ffprobe_path: String,
    /// Address for the local agent API.
    pub api_bind: String,
    /// ComfyUI's output folder. Empty means work it out from the running ComfyUI.
    pub comfy_output_dir: String,
    /// The LLM runner settings, saved for every project.
    pub llm: h3_llm::LlmConfig,
    /// Look for a new version of the app when it starts. The check asks GitHub for a public file and sends nothing else.
    pub check_for_updates: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        let documents = dirs::document_dir().unwrap_or_else(|| PathBuf::from("."));
        Self {
            comfy_url: "http://127.0.0.1:8188".into(),
            project_root: documents.join("VRGDG-H3 Projects"),
            ffmpeg_path: "ffmpeg".into(),
            ffprobe_path: "ffprobe".into(),
            api_bind: "127.0.0.1:8190".into(),
            comfy_output_dir: String::new(),
            llm: h3_llm::LlmConfig::default(),
            check_for_updates: true,
        }
    }
}

impl AppConfig {
    /// Default config file location.
    pub fn default_path() -> PathBuf {
        // `H3_CONFIG` points at another config file, for tests and for running two copies side by side.
        if let Some(path) = std::env::var_os("H3_CONFIG").filter(|p| !p.is_empty()) {
            return PathBuf::from(path);
        }
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("VRGDG-H3")
            .join("config.json")
    }

    /// Load the config at `path`, or defaults when the file does not exist yet.
    pub fn load(path: &Path) -> io::Result<Self> {
        match std::fs::read_to_string(path) {
            // Windows editors save a byte order mark that JSON parsers refuse.
            Ok(text) => serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| {
                io::Error::new(io::ErrorKind::InvalidData, format!("{}: {e}", path.display()))
            }),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e),
        }
    }

    /// Save atomically.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let value = serde_json::to_value(self).map_err(io::Error::other)?;
        atomic_write_json(path, &value)
    }

    /// ComfyUI base URL with any trailing slash removed.
    pub fn comfy_base(&self) -> &str {
        self.comfy_url.trim_end_matches('/')
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_gives_defaults_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let cfg = AppConfig::load(&path).unwrap();
        assert_eq!(cfg, AppConfig::default());
        let changed = AppConfig { comfy_url: "http://localhost:9000/".into(), ..cfg };
        changed.save(&path).unwrap();
        let back = AppConfig::load(&path).unwrap();
        assert_eq!(back, changed);
        assert_eq!(back.comfy_base(), "http://localhost:9000");
    }

    #[test]
    fn file_with_a_byte_order_mark_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, "\u{feff}{\"comfy_url\": \"http://localhost:9000\"}").unwrap();
        assert_eq!(AppConfig::load(&path).unwrap().comfy_url, "http://localhost:9000");
    }

    #[test]
    fn partial_file_keeps_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{"comfy_url": "http://10.0.0.2:8188"}"#).unwrap();
        let cfg = AppConfig::load(&path).unwrap();
        assert_eq!(cfg.comfy_url, "http://10.0.0.2:8188");
        assert_eq!(cfg.ffmpeg_path, "ffmpeg");
    }
}
