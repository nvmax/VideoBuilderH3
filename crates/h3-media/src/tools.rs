//! Finding and running ffmpeg and ffprobe. Every run is hidden, so no console window flashes on Windows.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("{0}")]
    Failed(String),
    #[error("ffmpeg and ffprobe were not found. Install them or set their paths in the app settings.")]
    NotFound,
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// The two programs everything media related runs.
#[derive(Debug, Clone)]
pub struct Tools {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

fn exe_names(name: &str) -> Vec<String> {
    if cfg!(windows) { vec![format!("{name}.exe")] } else { vec![name.to_string()] }
}

fn on_path(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&paths) {
        for file in exe_names(name) {
            let candidate = dir.join(&file);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

impl Tools {
    /// Use the configured paths when they are set, else look on `PATH`. A configured ffmpeg without a configured
    /// ffprobe uses the ffprobe next to it.
    pub fn find(ffmpeg: &str, ffprobe: &str) -> Result<Self, MediaError> {
        let ffmpeg_path = if ffmpeg.trim().is_empty() { on_path("ffmpeg") } else { Some(PathBuf::from(ffmpeg.trim())) };
        let ffmpeg_path = ffmpeg_path.ok_or(MediaError::NotFound)?;
        let ffprobe_path = if !ffprobe.trim().is_empty() {
            Some(PathBuf::from(ffprobe.trim()))
        } else {
            let sibling = exe_names("ffprobe").into_iter().map(|n| ffmpeg_path.with_file_name(n)).find(|p| p.is_file());
            sibling.or_else(|| on_path("ffprobe"))
        };
        Ok(Self { ffmpeg: ffmpeg_path, ffprobe: ffprobe_path.ok_or(MediaError::NotFound)? })
    }
}

/// Run a program to completion with its window hidden.
pub fn run(program: &Path, args: &[String]) -> std::io::Result<Output> {
    let mut command = Command::new(program);
    command.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    command.output()
}

/// The text a failed run printed: stderr, else stdout, else `fallback`.
pub fn failure_text(output: &Output, fallback: &str) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !stderr.is_empty() {
        return stderr;
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() { fallback.to_string() } else { stdout }
}
