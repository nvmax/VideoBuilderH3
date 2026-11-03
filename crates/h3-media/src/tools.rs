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

/// What [`Tools::describe`] found out about ffmpeg.
#[derive(Debug, Clone)]
pub struct ToolInfo {
    pub version: String,
    pub has_libx264: bool,
    pub source: String,
    pub path: String,
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

/// A program that ships with the app: next to the app's own executable, or in a `bin` folder there.
pub fn bundled(name: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    for folder in [dir.to_path_buf(), dir.join("bin")] {
        for file in exe_names(name) {
            let candidate = folder.join(&file);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
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

    /// Which ffmpeg this is: its version line, whether it has the libx264 encoder and where it was found.
    pub fn describe(&self) -> ToolInfo {
        let version = run(&self.ffmpeg, &args(&["-hide_banner", "-version"]))
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).lines().next().unwrap_or("").trim().to_string())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "ffmpeg".into());
        let has_libx264 = run(&self.ffmpeg, &args(&["-hide_banner", "-encoders"])).ok().is_some_and(|o| String::from_utf8_lossy(&o.stdout).contains("libx264"));
        let source = if bundled("ffmpeg").is_some_and(|b| b == self.ffmpeg) { "bundled with the app" } else { "installed on this computer" };
        ToolInfo { version, has_libx264, source: source.into(), path: self.ffmpeg.display().to_string() }
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

/// A program with its arguments and no console window, for callers that wire up the pipes themselves.
pub fn command(program: &Path, args: &[String]) -> Command {
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
        return bundled(default_name).or_else(|| on_path(default_name));
    }
    if !text.contains(['/', '\\']) {
        let name = text.strip_suffix(".exe").unwrap_or(text);
        return bundled(name).or_else(|| on_path(name));
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

/// Scene copies are the files the final video is made of, and a stitch can encode them again (timeline sync, inserts,
/// canvas fit, transitions). The MiniMax H3 render is CRF 19, so these stay below it and add no visible loss. Without
/// explicit values x264 falls back to CRF 23 / veryfast and AAC to its default bitrate. Keep in step with the pack's
/// `SCENE_VIDEO_CRF`, `SCENE_VIDEO_PRESET` and `SCENE_AUDIO_BITRATE` in `runner/video_files.py`.
pub const SCENE_VIDEO_CRF: u32 = 16;
pub const SCENE_VIDEO_PRESET: &str = "medium";
pub const SCENE_AUDIO_BITRATE: &str = "256k";

/// Bytes from the start of a file that are searched for the settings string x264 writes into its first frame.
const X264_SETTINGS_SCAN_BYTES: u64 = 1 << 20;

/// The CRF an x264 file was encoded at, read from the settings string x264 stores in it (`rc=crf ... crf=19.0`).
/// `None` when the file is not CRF encoded by x264 or cannot be read.
pub fn probe_x264_crf(path: &Path) -> Option<f64> {
    use std::io::Read;
    let mut head = Vec::new();
    std::fs::File::open(path).ok()?.take(X264_SETTINGS_SCAN_BYTES).read_to_end(&mut head).ok()?;
    let needle = b"x264 - core";
    let start = head.windows(needle.len()).position(|w| w == needle)?;
    let end = head[start..].iter().position(|b| *b == 0).map_or(head.len(), |n| start + n);
    let text = String::from_utf8_lossy(&head[start..end]);
    if !text.split_whitespace().any(|word| word == "rc=crf") {
        return None;
    }
    text.split_whitespace().find_map(|word| word.strip_prefix("crf=")).and_then(|value| value.parse().ok())
}

/// The lowest CRF the given files state, or `None` when none of them does.
pub fn source_crf<P: AsRef<Path>>(paths: &[P]) -> Option<f64> {
    paths.iter().filter_map(|path| probe_x264_crf(path.as_ref())).fold(None, |lowest, crf| Some(lowest.map_or(crf, |l: f64| l.min(crf))))
}

/// The CRF for a scene copy: `SCENE_VIDEO_CRF`, or the source's own CRF when that is lower, so a copy is never coarser
/// than the clip it is made from.
pub fn scene_video_crf(source: Option<f64>) -> u32 {
    match source {
        Some(crf) if crf.is_finite() => (crf.max(0.0) as u32).min(SCENE_VIDEO_CRF),
        _ => SCENE_VIDEO_CRF,
    }
}

/// The ffmpeg video encoder arguments for every scene copy and stitch re-encode, given the CRF the source was written at.
pub fn scene_video_args(source: Option<f64>) -> Vec<String> {
    args(&["-c:v", "libx264", "-preset", SCENE_VIDEO_PRESET, "-crf", &scene_video_crf(source).to_string(), "-pix_fmt", "yuv420p"])
}

/// The ffmpeg AAC encoder arguments for every scene copy and stitch audio encode.
pub fn scene_audio_args() -> Vec<String> {
    args(&["-c:a", "aac", "-b:a", SCENE_AUDIO_BITRATE])
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
