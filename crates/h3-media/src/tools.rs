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
    /// Use the configured programs when they are set, else look on `PATH`. A bare name such as `ffmpeg` is looked up on
    /// `PATH` and a path must point at a file, so a missing program fails here with a clear message instead of when the
    /// first clip is cut. A configured ffmpeg without a configured ffprobe uses the ffprobe next to it.
    pub fn find(ffmpeg: &str, ffprobe: &str) -> Result<Self, MediaError> {
        let ffmpeg_path = resolve(ffmpeg, "ffmpeg").ok_or(MediaError::NotFound)?;
        let ffprobe_path = if !ffprobe.trim().is_empty() {
            resolve(ffprobe, "ffprobe")
        } else {
            let sibling = exe_names("ffprobe").into_iter().map(|n| ffmpeg_path.with_file_name(n)).find(|p| p.is_file());
            sibling.or_else(|| on_path("ffprobe"))
        };
        Ok(Self { ffmpeg: ffmpeg_path, ffprobe: ffprobe_path.ok_or(MediaError::NotFound)? })
    }

    /// Run ffmpeg and fail with its own message when it exits with an error.
    pub fn ffmpeg_checked(&self, args: &[String], what: &str) -> Result<Output, MediaError> {
        let output = run(&self.ffmpeg, args)?;
        if output.status.success() { Ok(output) } else { Err(MediaError::Failed(failure_text(&output, what))) }
    }

    /// Run ffprobe and fail with its own message when it exits with an error.
    pub fn ffprobe_checked(&self, args: &[String], what: &str) -> Result<Output, MediaError> {
        let output = run(&self.ffprobe, args)?;
        if output.status.success() { Ok(output) } else { Err(MediaError::Failed(failure_text(&output, what))) }
    }
}

fn command(program: &Path, args: &[String]) -> Command {
    let mut command = Command::new(program);
    command.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    command
}

/// Run a program to completion with its window hidden.
/// A configured program: empty means `default_name` on `PATH`, a bare name is looked up on `PATH`, anything with a
/// folder in it must be an existing file.
fn resolve(configured: &str, default_name: &str) -> Option<PathBuf> {
    let text = configured.trim();
    if text.is_empty() {
        return on_path(default_name);
    }
    if !text.contains(['/', '\\']) {
        return on_path(text.strip_suffix(".exe").unwrap_or(text));
    }
    let path = PathBuf::from(text);
    if path.is_file() {
        return Some(path);
    }
    exe_names(text).into_iter().map(PathBuf::from).find(|p| p.is_file())
}

pub fn run(program: &Path, args: &[String]) -> std::io::Result<Output> {
    command(program, args).output()
}

/// Like [`run`], from a given working folder.
pub fn run_in(program: &Path, args: &[String], cwd: &Path) -> std::io::Result<Output> {
    command(program, args).current_dir(cwd).output()
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

/// Command line arguments from string slices.
pub fn args(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_configured_program_that_does_not_exist_is_an_error_now() {
        assert!(matches!(Tools::find("no-such-program-here", ""), Err(MediaError::NotFound)));
        assert!(matches!(Tools::find(r"C:\nowhere\ffmpeg.exe", ""), Err(MediaError::NotFound)));
    }
}
