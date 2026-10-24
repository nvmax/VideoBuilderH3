//! Color grading of a scene clip with a `.cube` LUT. Port of `apply_lut_to_video` and `_render_video_effect` in
//! `post_process/lut_video_tools.py`: ffmpeg's `lut3d` filter, blended with the source when the strength is below 10.

use std::fs;
use std::path::Path;
use std::time::Instant;

use serde_json::{json, Value};

use crate::tools::{args, failure_text, run_in, MediaError, Tools};
use crate::video::{clip_frame_length, probe_output_media, probe_stream_fields};

const VIDEO_EXTENSIONS: [&str; 5] = [".mp4", ".mov", ".avi", ".webm", ".mkv"];
const PRESETS: [&str; 9] = ["ultrafast", "superfast", "veryfast", "faster", "fast", "medium", "slow", "slower", "veryslow"];

/// `int(round(float(value)))` kept between 16 and 35.
pub fn normalize_encode_crf(value: Option<f64>) -> i64 {
    (value.map_or(23.0, f64::round) as i64).clamp(16, 35)
}

pub fn normalize_encode_preset(value: &str) -> String {
    let preset = value.trim().to_lowercase();
    if PRESETS.contains(&preset.as_str()) { preset } else { "medium".into() }
}

/// The filter graph for a LUT at `strength` (0 to 10).
pub fn lut_filter(strength: f64) -> String {
    let blend = strength.clamp(0.0, 10.0) / 10.0;
    let lut3d = "lut3d=file=lut.cube:interp=trilinear";
    if blend <= 0.0 {
        "null".into()
    } else if blend >= 1.0 {
        lut3d.into()
    } else {
        format!("split[source][graded];[graded]{lut3d}[lut];[lut][source]blend=all_mode=normal:all_opacity={}", h3_core::pyval::py_float_str(blend))
    }
}

pub(crate) fn has_audio(tools: &Tools, path: &Path) -> Result<bool, MediaError> {
    Ok(!probe_stream_fields(tools, path, "a:0", &["codec_type"], false)?.is_empty())
}

/// Grade `input` with the `.cube` file and write an H.264 clip to `output`. The source audio is kept when `preserve_audio`
/// is set and there is some. Returns what the pack reports about the render.
#[allow(clippy::too_many_arguments)]
pub fn apply_lut_to_video(tools: &Tools, input: &Path, output: &Path, cube: &Path, strength: f64, preserve_audio: bool, crf: i64, preset: &str) -> Result<Value, MediaError> {
    if !input.is_file() {
        return Err(MediaError::Failed(format!("Input video was not found: {}", input.display())));
    }
    let extension = input.extension().map(|e| format!(".{}", e.to_string_lossy().to_lowercase())).unwrap_or_default();
    if !VIDEO_EXTENSIONS.contains(&extension.as_str()) {
        return Err(MediaError::Failed("Input video type is not supported.".into()));
    }
    if !cube.is_file() {
        return Err(MediaError::Failed(format!("LUT file was not found: {}", cube.display())));
    }
    let parent = output.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let source_had_audio = has_audio(tools, input)?;
    let keep_audio = preserve_audio && source_had_audio;
    let work = tempfile::Builder::new().prefix("vrgdg_lut_").tempdir()?;
    fs::copy(cube, work.path().join("lut.cube"))?;
    let temp_output = parent.join(format!(".vrgdg_effect_{}_{}.mp4", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis())));
    let mut a = args(&["-y", "-loglevel", "error", "-i", &input.display().to_string(), "-vf", &lut_filter(strength), "-map", "0:v:0"]);
    if keep_audio {
        a.extend(args(&["-map", "0:a:0", "-c:a", "aac", "-b:a", "192k"]));
    } else {
        a.push("-an".into());
    }
    a.extend(args(&["-c:v", "libx264", "-preset", &normalize_encode_preset(preset), "-crf", &crf.clamp(16, 35).to_string(), "-pix_fmt", "yuv420p", "-movflags", "+faststart", &temp_output.display().to_string()]));
    let started = Instant::now();
    let out = run_in(&tools.ffmpeg, &a, work.path())?;
    if !out.status.success() {
        let _ = fs::remove_file(&temp_output);
        return Err(MediaError::Failed(format!("ffmpeg could not encode the video: {}", failure_text(&out, "ffmpeg failed."))));
    }
    let (frames, fps) = clip_frame_length(tools, &temp_output).inspect_err(|_| {
        let _ = fs::remove_file(&temp_output);
    })?;
    let media = probe_output_media(tools, &temp_output);
    let output_has_audio = has_audio(tools, &temp_output).unwrap_or(false);
    fs::rename(&temp_output, output).or_else(|_| fs::copy(&temp_output, output).and_then(|_| fs::remove_file(&temp_output)))?;
    let elapsed = started.elapsed().as_secs_f64();
    Ok(json!({
        "width": media.width,
        "height": media.height,
        "fps": fps.as_f64(),
        "processed_frames": frames,
        "elapsed_seconds": elapsed,
        "processed_fps": if elapsed > 0.0 { frames as f64 / elapsed } else { 0.0 },
        "audio_preserved": keep_audio && output_has_audio,
        "source_had_audio": source_had_audio,
        "preserve_audio": preserve_audio,
        "encode_crf": crf.clamp(16, 35),
        "encode_preset": normalize_encode_preset(preset),
        "encoder": "ffmpeg libx264",
        "browser_friendly": true,
    }))
}
