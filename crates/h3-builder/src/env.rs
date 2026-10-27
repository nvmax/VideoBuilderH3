//! Where the Builder keeps things on this computer.

use std::path::PathBuf;

use h3_media::Tools;

/// The folders and tools a Builder call works with.
#[derive(Debug, Clone)]
pub struct Env {
    /// ComfyUI's output folder, where the pack keeps profiles, model defaults and projects. When ComfyUI's folder is not
    /// known this is the app's own project folder, so the same sub folders still work.
    pub output: PathBuf,
    /// Folders projects are listed from and may be deleted in.
    pub roots: Vec<PathBuf>,
    pub ffmpeg: String,
    pub ffprobe: String,
    /// The folder of `.cube` LUT files the Post Process tab lists.
    pub luts: PathBuf,
    /// The overlay library folder when it is not the default `overlay` folder beside `luts` (`models/overlay` of ComfyUI, or the folder set in the config).
    pub overlay_dir: Option<PathBuf>,
}

impl Env {
    /// ffmpeg and ffprobe, or why they are missing.
    pub fn tools(&self) -> Result<Tools, String> {
        Tools::find(&self.ffmpeg, &self.ffprobe).map_err(|e| e.to_string())
    }
}
