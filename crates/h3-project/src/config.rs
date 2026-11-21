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
    /// Where the overlay library (overlay clips and fonts) is kept. Empty means `models/overlay` of the running ComfyUI.
    pub overlay_library_dir: String,
    /// The LLM runner settings, saved for every project.
    pub llm: h3_llm::LlmConfig,
    /// Look for a new version of the app when it starts. The check asks GitHub for a public file and sends nothing else.
    pub check_for_updates: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        // A Linux account without a Documents folder set up has no document folder at all, so the home folder is the next place.
        let documents = dirs::document_dir().or_else(|| dirs::home_dir().map(|home| home.join("Documents"))).unwrap_or_else(|| PathBuf::from("."));
        Self {
            comfy_url: "http://127.0.0.1:8188".into(),
            project_root: documents.join("VRGDG-H3 Projects"),
            ffmpeg_path: "ffmpeg".into(),
            ffprobe_path: "ffprobe".into(),
            api_bind: "127.0.0.1:8190".into(),
            comfy_output_dir: String::new(),
            overlay_library_dir: String::new(),
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

/// Where the Hugging Face token is kept: `hf_token.json` next to the config file. It has a file of its own and is not a field of `config.json`,
/// because the Settings screen reads the config back and the token never goes to a page.
pub fn hf_token_path(config_path: &Path) -> PathBuf {
    config_path.parent().unwrap_or_else(|| Path::new(".")).join("hf_token.json")
}

/// The saved Hugging Face token, or an empty string when none is saved. The model installer sends it with its downloads from huggingface.co.
pub fn load_hf_token(config_path: &Path) -> String {
    let text = std::fs::read_to_string(hf_token_path(config_path)).unwrap_or_default();
    let value: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).unwrap_or_default();
    value.get("hf_token").and_then(serde_json::Value::as_str).unwrap_or_default().trim().to_string()
}

/// Save the Hugging Face token atomically. An empty token removes it.
pub fn save_hf_token(config_path: &Path, token: &str) -> io::Result<()> {
    let path = hf_token_path(config_path);
    let token = token.trim();
    if token.is_empty() {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)?;
    }
    atomic_write_json(&path, &serde_json::json!({"hf_token": token}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hugging_face_token_is_kept_in_its_own_file_next_to_the_config() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("sub").join("config.json");
        assert_eq!(load_hf_token(&config), "", "nothing is saved at first");
        save_hf_token(&config, "  hf_abc123  ").unwrap();
        assert_eq!(load_hf_token(&config), "hf_abc123", "it is saved trimmed, and the folder is made when it is missing");
        assert_eq!(hf_token_path(&config), dir.path().join("sub").join("hf_token.json"));
        assert!(!config.exists(), "the config file is not touched");
        save_hf_token(&config, "hf_other").unwrap();
        assert_eq!(load_hf_token(&config), "hf_other");
        // An empty token removes it, and removing what is not there is fine.
        save_hf_token(&config, "   ").unwrap();
        assert_eq!(load_hf_token(&config), "");
        assert!(!hf_token_path(&config).exists());
        save_hf_token(&config, "").unwrap();
    }

    #[test]
    fn a_damaged_token_file_reads_as_no_token() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.json");
        std::fs::write(hf_token_path(&config), "not json").unwrap();
        assert_eq!(load_hf_token(&config), "");
        std::fs::write(hf_token_path(&config), "\u{feff}{\"hf_token\": \"hf_x\"}").unwrap();
        assert_eq!(load_hf_token(&config), "hf_x", "a byte order mark is fine");
    }

    #[test]
    fn the_default_project_folder_is_never_relative() {
        // A relative folder would end up wherever the app was started from.
        assert!(AppConfig::default().project_root.is_absolute());
    }

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
