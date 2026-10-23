//! Scene clips: probing, thumbnails, the exact trim, the final frame and the opening color match. Port of the clip
//! functions in `runner/video_files.py` and `_extract_video_final_frame_as_scene_image` in `builder/media.py`.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::tools::{args, failure_text, run, run_in, MediaError, Tools};

/// A frame rate as a fraction, so a clip's length is exactly `frames / fps` seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ratio {
    pub num: i128,
    pub den: i128,
}

impl Ratio {
    pub fn new(num: i128, den: i128) -> Self {
        let g = gcd(num.abs(), den.abs()).max(1);
        Self { num: num / g, den: den / g }
    }

    /// Parse `24`, `24/1` or `24000/1001`.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let (n, d) = match text.split_once('/') {
            Some((n, d)) => (n.trim().parse::<i128>().ok()?, d.trim().parse::<i128>().ok()?),
            None => (text.parse::<i128>().ok()?, 1),
        };
        (d != 0).then(|| Self::new(n, d))
    }

    pub fn as_f64(&self) -> f64 {
        self.num as f64 / self.den as f64
    }

    pub fn is_positive(&self) -> bool {
        self.num > 0 && self.den > 0
    }
}

fn gcd(a: i128, b: i128) -> i128 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// Width and height of a video's first video stream.
pub fn video_size(tools: &Tools, path: &Path) -> Result<(u32, u32), MediaError> {
    let out = tools.ffprobe_checked(
        &args(&["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height", "-of", "csv=s=x:p=0", &path.display().to_string()]),
        "ffprobe could not read the video size.",
    )?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().next().unwrap_or("").trim().to_lowercase();
    let (w, h) = line.split_once('x').ok_or_else(|| MediaError::Failed(format!("ffprobe returned no size for {}", path.display())))?;
    let parse = |s: &str| s.trim().parse::<u32>().map_err(|_| MediaError::Failed(format!("ffprobe returned a bad size for {}", path.display())));
    Ok((parse(w)?, parse(h)?))
}

/// Width, height and stream lengths of a written file. Zero where unknown.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OutputMedia {
    pub width: u32,
    pub height: u32,
    pub video_duration: f64,
    pub audio_duration: f64,
}

pub fn probe_output_media(tools: &Tools, path: &Path) -> OutputMedia {
    let mut info = OutputMedia::default();
    let Ok(out) = tools.ffprobe_checked(
        &args(&["-v", "error", "-show_entries", "stream=codec_type,width,height,duration", "-of", "json", &path.display().to_string()]),
        "ffprobe failed",
    ) else {
        return info;
    };
    let parsed: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    for stream in parsed.get("streams").and_then(Value::as_array).into_iter().flatten() {
        let duration = stream.get("duration").and_then(Value::as_str).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
        match stream.get("codec_type").and_then(Value::as_str) {
            Some("video") if info.width == 0 => {
                info.width = stream.get("width").and_then(Value::as_u64).unwrap_or(0) as u32;
                info.height = stream.get("height").and_then(Value::as_u64).unwrap_or(0) as u32;
                info.video_duration = duration;
            }
            Some("audio") if info.audio_duration == 0.0 => info.audio_duration = duration,
            _ => {}
        }
    }
    info
}

/// `key=value` lines of ffprobe's default writer for the first stream of a kind. Empty when there is no such stream.
pub fn probe_stream_fields(tools: &Tools, path: &Path, stream: &str, fields: &[&str], count_packets: bool) -> Result<std::collections::HashMap<String, String>, MediaError> {
    let mut a = args(&["-v", "error", "-select_streams", stream]);
    if count_packets {
        a.push("-count_packets".into());
    }
    a.extend(args(&["-show_entries", &format!("stream={}", fields.join(",")), "-of", "default=noprint_wrappers=1", &path.display().to_string()]));
    // Fail loudly: reading "no audio stream" from a failed probe would silence the scene.
    let out = tools.ffprobe_checked(&a, &format!("ffprobe could not read {}", path.display()))?;
    let mut values = std::collections::HashMap::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some((k, v)) = line.split_once('=') {
            values.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    Ok(values)
}

/// `(frames, fps)` of a clip's video stream.
pub fn clip_frame_length(tools: &Tools, path: &Path) -> Result<(i64, Ratio), MediaError> {
    let values = probe_stream_fields(tools, path, "v:0", &["nb_read_packets", "r_frame_rate", "avg_frame_rate"], true)?;
    let fps = ["r_frame_rate", "avg_frame_rate"]
        .iter()
        .filter_map(|k| values.get(*k).and_then(|v| Ratio::parse(v)))
        .find(Ratio::is_positive);
    let frames = values.get("nb_read_packets").and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
    match fps {
        Some(fps) if frames > 0 => Ok((frames, fps)),
        _ => Err(MediaError::Failed(format!("Could not read the frame count and frame rate of scene video: {}", path.display()))),
    }
}

/// Where the thumbnail of a scene video lives: `scene_video_thumbnails/<name>.jpg` for clips in the project's video
/// folders, else next to the clip.
pub fn thumbnail_path_for(video: &Path) -> PathBuf {
    let stem = video.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut current = video.parent();
    while let Some(dir) = current {
        let name = dir.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        if name == "rendered_scene_videos" || name == "rendered_scene_videos_backup" {
            if let Some(project) = dir.parent() {
                return project.join("scene_video_thumbnails").join(format!("{stem}.jpg"));
            }
        }
        current = dir.parent().filter(|p| p != &dir);
    }
    video.with_extension("jpg")
}

/// The pre 2025 thumbnail location, next to the clip. It is removed whenever a clip is replaced.
pub fn legacy_thumbnail_path(video: &Path) -> PathBuf {
    video.with_extension("jpg")
}

/// A 480 pixel wide JPEG of the clip at half a second (the first frame when the clip is shorter).
pub fn create_thumbnail(tools: &Tools, video: &Path, thumbnail: Option<&Path>) -> Option<PathBuf> {
    if !video.is_file() {
        return None;
    }
    let target = thumbnail.map(Path::to_path_buf).unwrap_or_else(|| thumbnail_path_for(video));
    fs::create_dir_all(target.parent()?).ok()?;
    let extract = |at: &str| {
        let a = args(&["-y", "-ss", at, "-i", &video.display().to_string(), "-frames:v", "1", "-vf", "scale=480:-2", "-q:v", "3", &target.display().to_string()]);
        run(&tools.ffmpeg, &a).ok()
    };
    let first = extract("0.5");
    if first.as_ref().is_some_and(|o| o.status.success()) && target.is_file() {
        return Some(target);
    }
    let second = extract("0");
    if second.as_ref().is_some_and(|o| o.status.success()) && target.is_file() {
        return Some(target);
    }
    if let Some(out) = second {
        eprintln!("[VRGDG-RS Media] Could not create a thumbnail for '{}': {}", video.display(), failure_text(&out, "ffmpeg could not extract a thumbnail."));
    }
    None
}

const VIDEO_EXTENSIONS: [&str; 6] = ["mp4", "mov", "mkv", "webm", "avi", "m4v"];

pub fn is_video_file(path: &Path) -> bool {
    path.extension().map(|e| VIDEO_EXTENSIONS.contains(&e.to_string_lossy().to_lowercase().as_str())).unwrap_or(false)
}

/// What to cut out of a raw render.
#[derive(Debug, Clone)]
pub struct TrimRequest {
    pub project: PathBuf,
    pub scene: u32,
    pub source: PathBuf,
    pub start: f64,
    pub duration: f64,
    pub frames: u32,
    pub label: String,
    pub mark_as_audio_video: bool,
    /// A masked Audio Mask render gets the real scene audio back: the file and where in it the scene starts.
    pub restore_audio: Option<(PathBuf, f64)>,
}

#[derive(Debug, Clone)]
pub struct TrimResult {
    pub video_path: PathBuf,
    pub thumbnail_path: Option<PathBuf>,
}

fn clean_label(label: &str) -> String {
    let mut out = String::new();
    let mut in_run = false;
    for c in label.trim().to_lowercase().chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            out.push(c);
            in_run = false;
        } else if !in_run {
            out.push('_');
            in_run = true;
        }
    }
    let trimmed = out.trim_matches('_');
    if trimmed.is_empty() { "trim".into() } else { trimmed.to_string() }
}

fn first_free(dir: &Path, build: impl Fn(Option<u32>) -> String) -> PathBuf {
    let mut path = dir.join(build(None));
    let mut index = 2;
    while path.exists() {
        path = dir.join(build(Some(index)));
        index += 1;
    }
    path
}

/// Cut the raw render to the scene's exact length (and exact frame count) as `video_NNNN-<label>_<stamp>[-audio].mp4`
/// in the project's `rendered_scene_videos` folder. `stamp` is `%Y%m%d_%H%M%S`.
pub fn trim_scene_video(tools: &Tools, req: &TrimRequest, stamp: &str) -> Result<TrimResult, MediaError> {
    if !req.source.is_file() {
        return Err(MediaError::Failed(format!("Scene video was not found: {}", req.source.display())));
    }
    if !is_video_file(&req.source) {
        return Err(MediaError::Failed(format!("Scene media is not a supported video file: {}", req.source.display())));
    }
    let dir = req.project.join("rendered_scene_videos");
    fs::create_dir_all(&dir)?;
    let start = req.start.max(0.0);
    let duration = req.duration.max(0.05);
    let label = clean_label(&req.label);
    let suffix = if req.mark_as_audio_video { "-audio" } else { "" };
    let scene = req.scene;
    let target = first_free(&dir, |n| match n {
        None => format!("video_{scene:04}-{label}_{stamp}{suffix}.mp4"),
        Some(i) => format!("video_{scene:04}-{label}_{stamp}_{i:02}{suffix}.mp4"),
    });
    if let Some((audio, _)) = &req.restore_audio {
        if !audio.is_file() {
            return Err(MediaError::Failed(format!("Scene audio to restore was not found: {}", audio.display())));
        }
    }
    let mut a = args(&["-y", "-ss", &format!("{start:.6}"), "-i", &req.source.display().to_string()]);
    if let Some((audio, audio_start)) = &req.restore_audio {
        a.extend(args(&["-ss", &format!("{:.6}", audio_start.max(0.0)), "-i", &audio.display().to_string()]));
    }
    a.extend(args(&["-t", &format!("{duration:.6}"), "-map", "0:v:0", "-map", if req.restore_audio.is_some() { "1:a:0" } else { "0:a?" }]));
    a.extend(args(&["-c:v", "libx264", "-pix_fmt", "yuv420p", "-preset", "veryfast", "-c:a", "aac", "-movflags", "+faststart"]));
    if req.frames > 0 {
        // Match the frame count the stitcher keeps so the clip's last frame is the one shown.
        a.extend(args(&["-frames:v", &req.frames.to_string()]));
    }
    a.push(target.display().to_string());
    let out = run(&tools.ffmpeg, &a)?;
    if !out.status.success() || !target.is_file() {
        return Err(MediaError::Failed(failure_text(&out, "ffmpeg failed to trim scene video.")));
    }
    let thumbnail_path = create_thumbnail(tools, &target, None);
    Ok(TrimResult { video_path: target, thumbnail_path })
}

/// Save the last frame of a clip as a PNG in the scene's preview folder. Returns its path.
pub fn extract_final_frame(tools: &Tools, project: &Path, source: &Path, scene: u32, stamp: &str) -> Result<PathBuf, MediaError> {
    let source_abs = std::path::absolute(source).unwrap_or_else(|_| source.to_path_buf());
    let project_abs = std::path::absolute(project).unwrap_or_else(|_| project.to_path_buf());
    if !source.is_file() || !source_abs.starts_with(&project_abs) {
        return Err(MediaError::Failed("Source video must be inside the current project folder.".into()));
    }
    let folder = project.join("scene_image_previews").join(format!("scene_{:04}", scene.max(1)));
    fs::create_dir_all(&folder)?;
    let target = first_free(&folder, |n| match n {
        None => format!("preview_{stamp}.png"),
        Some(i) => format!("preview_{stamp}_{i:02}.png"),
    });
    let mut last_error = String::new();
    for seek in ["-0.04", "-0.12", "-0.5"] {
        let a = args(&["-y", "-sseof", seek, "-i", &source.display().to_string(), "-frames:v", "1", "-update", "1", &target.display().to_string()]);
        let out = run(&tools.ffmpeg, &a)?;
        if out.status.success() && fs::metadata(&target).map(|m| m.len() > 0).unwrap_or(false) {
            return Ok(target);
        }
        last_error = failure_text(&out, "ffmpeg could not extract a final frame.");
        let _ = fs::remove_file(&target);
    }
    Err(MediaError::Failed(if last_error.is_empty() { "ffmpeg could not extract a final frame.".into() } else { last_error }))
}

/// Per channel mean and population standard deviation of an image, like PIL's `ImageStat`.
fn channel_stats(path: &Path) -> Result<([f64; 3], [f64; 3]), MediaError> {
    let image = image::open(path).map_err(|e| MediaError::Failed(format!("Could not read {}: {e}", path.display())))?.to_rgb8();
    let count = (image.width() as f64) * (image.height() as f64);
    let (mut sum, mut sum_sq) = ([0f64; 3], [0f64; 3]);
    for pixel in image.pixels() {
        for c in 0..3 {
            let v = f64::from(pixel[c]);
            sum[c] += v;
            sum_sq[c] += v * v;
        }
    }
    let mean = [sum[0] / count, sum[1] / count, sum[2] / count];
    let std = [0, 1, 2].map(|c| (sum_sq[c] / count - mean[c] * mean[c]).max(0.0).sqrt());
    Ok((mean, std))
}

/// What a color match did.
#[derive(Debug, Clone)]
pub struct ColorMatchResult {
    pub applied: bool,
    pub thumbnail_path: Option<PathBuf>,
}

/// Match a new clip's opening color to the previous clip, then fade the correction out over `fade_seconds`. The clip is
/// replaced in place. `token` makes the scratch file names unique.
pub fn apply_color_match(tools: &Tools, video: &Path, reference: &Path, fade_seconds: f64, strength: f64, token: &str) -> Result<ColorMatchResult, MediaError> {
    for (label, path) in [("Scene video", video), ("Previous scene video", reference)] {
        if !path.is_file() {
            return Err(MediaError::Failed(format!("{label} was not found: {}", path.display())));
        }
    }
    let fade = fade_seconds.clamp(0.05, 30.0);
    let strength = strength.clamp(0.0, 1.0);
    if strength <= 0.0 {
        return Ok(ColorMatchResult { applied: false, thumbnail_path: None });
    }
    let work_dir = video.parent().unwrap_or(Path::new(".")).to_path_buf();
    let reference_frame = work_dir.join(format!(".vrgdg_color_reference_{token}.png"));
    let target_frame = work_dir.join(format!(".vrgdg_color_target_{token}.png"));
    let cube_path = work_dir.join(format!(".vrgdg_color_match_{token}.cube"));
    let output_path = work_dir.join(format!(".vrgdg_color_matched_{token}.mp4"));
    let cleanup = || {
        for p in [&reference_frame, &target_frame, &cube_path, &output_path] {
            let _ = fs::remove_file(p);
        }
    };
    let result = (|| -> Result<(), MediaError> {
        let ffmpeg = |a: Vec<String>, message: &str| -> Result<(), MediaError> {
            let out = run_in(&tools.ffmpeg, &a, &work_dir)?;
            if out.status.success() { Ok(()) } else { Err(MediaError::Failed(failure_text(&out, message))) }
        };
        // -update 1 leaves the last decoded frame in the PNG after processing the final second.
        ffmpeg(args(&["-y", "-sseof", "-1", "-i", &reference.display().to_string(), "-map", "0:v:0", "-an", "-update", "1", &reference_frame.display().to_string()]), "FFmpeg could not read the previous clip's final frame.")?;
        ffmpeg(args(&["-y", "-i", &video.display().to_string(), "-map", "0:v:0", "-an", "-frames:v", "1", &target_frame.display().to_string()]), "FFmpeg could not read the new clip's first frame.")?;
        let (ref_mean, ref_std) = channel_stats(&reference_frame)?;
        let (tgt_mean, tgt_std) = channel_stats(&target_frame)?;
        let scales: Vec<f64> = (0..3).map(|i| (ref_std[i].max(1.0) / tgt_std[i].max(1.0)).clamp(0.25, 4.0)).collect();
        let offsets: Vec<f64> = (0..3).map(|i| ref_mean[i] - tgt_mean[i] * scales[i]).collect();
        let size = 17;
        let mut cube = String::from("TITLE \"VRGDG opening color match\"\n");
        cube.push_str(&format!("LUT_3D_SIZE {size}\nDOMAIN_MIN 0.0 0.0 0.0\nDOMAIN_MAX 1.0 1.0 1.0\n"));
        for blue in 0..size {
            for green in 0..size {
                for red in 0..size {
                    let values = [red, green, blue];
                    let c: Vec<f64> = (0..3)
                        .map(|i| (((values[i] as f64 / (size - 1) as f64) * 255.0 * scales[i] + offsets[i]) / 255.0).clamp(0.0, 1.0))
                        .collect();
                    cube.push_str(&format!("{:.8} {:.8} {:.8}\n", c[0], c[1], c[2]));
                }
            }
        }
        fs::write(&cube_path, cube)?;
        let cube_name = cube_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let weight = format!("max(0\\,min(1\\,{strength:.6}*(1-T/{fade:.6})))");
        let graph = format!(
            "[0:v]split=2[original][to_match];[to_match]lut3d=file='{cube_name}'[matched];[original][matched]blend=all_expr='A*(1-({weight}))+B*({weight})'[video]"
        );
        ffmpeg(
            args(&["-y", "-i", &video.display().to_string(), "-filter_complex", &graph, "-map", "[video]", "-map", "0:a?", "-c:v", "libx264", "-preset", "veryfast", "-crf", "16", "-pix_fmt", "yuv420p", "-c:a", "copy", "-movflags", "+faststart", &output_path.display().to_string()]),
            "FFmpeg could not apply the opening color match.",
        )?;
        if !fs::metadata(&output_path).map(|m| m.len() > 0).unwrap_or(false) {
            return Err(MediaError::Failed("Opening color match did not create a valid video.".into()));
        }
        Ok(())
    })();
    if let Err(e) = result {
        cleanup();
        return Err(e);
    }
    // The caller notes the clip's identity before this call and updates the take archive after it.
    let replaced = fs::rename(&output_path, video);
    cleanup();
    replaced?;
    let thumbnail_path = create_thumbnail(tools, video, Some(&thumbnail_path_for(video)));
    Ok(ColorMatchResult { applied: true, thumbnail_path })
}
