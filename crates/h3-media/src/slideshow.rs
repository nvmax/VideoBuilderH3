//! A preview video made of still scene images over the song. Port of `_render_image_slideshow` in `runner/video_files.py`.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::stitch::unique_final_video_path;
use crate::tools::{args, failure_text, run};
use crate::{MediaError, Tools};

const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp", "tif", "tiff"];

/// One still and how long it stays on screen.
#[derive(Debug, Clone)]
pub struct SlideItem {
    pub path: PathBuf,
    pub seconds: f64,
}

/// What to render.
#[derive(Debug, Clone)]
pub struct SlideshowRequest {
    pub project: PathBuf,
    pub items: Vec<SlideItem>,
    pub audio: PathBuf,
    pub audio_start: f64,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub output_prefix: String,
}

fn concat_path(path: &Path) -> String {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()).display().to_string().replace('\\', "/").replace('\'', "'\\''")
}

fn ffmpeg(tools: &Tools, a: &[String], what: &str) -> Result<(), MediaError> {
    let out = run(&tools.ffmpeg, a)?;
    if out.status.success() {
        Ok(())
    } else {
        Err(MediaError::Failed(format!("{what}:\n{}", failure_text(&out, "ffmpeg failed."))))
    }
}

/// Check the request like the pack does, before any file is made.
pub fn check_request(req: &SlideshowRequest) -> Result<(), MediaError> {
    if req.items.is_empty() {
        return Err(MediaError::Failed("No scene images were provided for the slideshow preview.".into()));
    }
    if !req.audio.is_file() {
        return Err(MediaError::Failed(format!("Global audio file was not found: {}", req.audio.display())));
    }
    for (i, item) in req.items.iter().enumerate() {
        if !item.path.is_file() {
            return Err(MediaError::Failed(format!("Scene {} image was not found: {}", i + 1, item.path.display())));
        }
        let ext = item.path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        if !IMAGE_EXTENSIONS.contains(&ext.as_str()) {
            return Err(MediaError::Failed(format!("Scene {} media is not a supported slideshow image: {}", i + 1, item.path.display())));
        }
    }
    Ok(())
}

/// Render the slideshow into the project folder. Every image is first scaled onto one common canvas so a mixed resolution
/// project cannot lose a scene.
pub fn render_slideshow(tools: &Tools, req: &SlideshowRequest) -> Result<Value, MediaError> {
    check_request(req)?;
    let target_dir = req.project.join("slideshow_previews");
    fs::create_dir_all(&target_dir)?;
    let total: f64 = req.items.iter().map(|i| i.seconds).sum();
    let scratch = tempfile::Builder::new().prefix("_slideshow_").tempdir_in(&target_dir)?;
    let concat_file = scratch.path().join("images.txt");
    let video_only = scratch.path().join("video.mp4");
    let final_output = unique_final_video_path(&req.project, &req.output_prefix);

    let (w, h) = (req.width, req.height);
    let normalize = format!("scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2:color=black,setsar=1,format=rgb24");
    let mut normalized: Vec<(PathBuf, f64)> = Vec::new();
    for (i, item) in req.items.iter().enumerate() {
        let out = scratch.path().join(format!("image_{:06}.png", i + 1));
        let a = args(&["-y", "-i", &item.path.display().to_string(), "-vf", &normalize, "-frames:v", "1", &out.display().to_string()]);
        ffmpeg(tools, &a, &format!("Could not normalize slideshow Scene {}", i + 1))?;
        normalized.push((out, item.seconds));
    }
    let mut list = fs::File::create(&concat_file)?;
    for (path, seconds) in &normalized {
        writeln!(list, "file '{}'", concat_path(path))?;
        writeln!(list, "duration {seconds:.6}")?;
    }
    // The concat demuxer only applies the last duration when the last still is listed once more.
    if let Some((last, _)) = normalized.last() {
        writeln!(list, "file '{}'", concat_path(last))?;
    }
    drop(list);

    let a = args(&[
        "-y", "-f", "concat", "-safe", "0", "-i", &concat_file.display().to_string(), "-vf", &format!("fps={},format=yuv420p", req.fps), "-an", "-c:v", "libx264",
        "-preset", "veryfast", "-crf", "20", "-t", &format!("{total:.6}"), "-movflags", "+faststart", &video_only.display().to_string(),
    ]);
    ffmpeg(tools, &a, "ffmpeg could not build the slideshow video")?;

    let mut mux = args(&["-y", "-i", &video_only.display().to_string()]);
    if req.audio_start > 0.0 {
        mux.extend(args(&["-ss", &format!("{:.6}", req.audio_start)]));
    }
    mux.extend(args(&[
        "-i", &req.audio.display().to_string(), "-map", "0:v:0", "-map", "1:a:0", "-t", &format!("{total:.6}"), "-c:v", "copy", "-c:a", "aac", "-shortest",
        "-movflags", "+faststart", &final_output.display().to_string(),
    ]));
    ffmpeg(tools, &mux, "ffmpeg could not add the audio to the slideshow")?;
    if fs::metadata(&final_output).map(|m| m.len() == 0).unwrap_or(true) {
        return Err(MediaError::Failed("FFmpeg did not create the slideshow preview video.".into()));
    }
    Ok(json!({
        "final_video_path": final_output.display().to_string(),
        "video_folder": target_dir.display().to_string(),
        "scene_count": req.items.len(),
        "duration": total,
        "audio_start": req.audio_start,
        "output_width": w,
        "output_height": h,
        "fps": req.fps,
    }))
}
