//! A clip run through a per frame effect (film grain, Adjust) and written as H.264 with the source audio. Port of the
//! `apply_batch` branch of `_render_video_effect` in `post_process/lut_video_tools.py`: frames come out of one ffmpeg as
//! raw RGB, the effect changes them, and a second ffmpeg encodes them with the audio of the source.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Instant;

use serde_json::{json, Value};

use crate::grade::Rgb;
use crate::post::{has_audio, normalize_encode_preset};
use crate::tools::{args, command, MediaError, Tools};
use crate::video::{probe_stream_fields, Ratio};

/// How the result is encoded.
pub struct Encode<'a> {
    pub preserve_audio: bool,
    pub crf: i64,
    pub preset: &'a str,
}

fn temp_beside(output: &Path) -> PathBuf {
    let parent = output.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    parent.join(format!(".vrgdg_effect_{}_{stamp}.mp4", std::process::id()))
}

/// What the encoder or decoder said, for an error message.
fn log_tail(log: &mut File) -> String {
    let mut text = String::new();
    if log.seek(SeekFrom::Start(0)).is_ok() {
        let _ = log.read_to_string(&mut text);
    }
    let text = text.trim();
    let start = text.char_indices().rev().nth(999).map_or(0, |(i, _)| i);
    text[start..].to_string()
}

/// Read one frame of `buffer.len()` bytes. False at the end of the stream, also when the last frame is cut short.
fn read_frame(source: &mut impl Read, buffer: &mut [u8]) -> std::io::Result<bool> {
    let mut filled = 0;
    while filled < buffer.len() {
        match source.read(&mut buffer[filled..]) {
            Ok(0) => return Ok(false),
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(true)
}

/// Run every frame of `input` through `effect(frame, frame_index)` and write the clip to `output`. Returns what the
/// pack reports about the render.
pub fn render_effect(tools: &Tools, input: &Path, output: &Path, encode: &Encode, mut effect: impl FnMut(&mut Rgb, usize)) -> Result<Value, MediaError> {
    let probe = probe_stream_fields(tools, input, "v:0", &["width", "height", "avg_frame_rate", "r_frame_rate", "nb_frames"], false)?;
    let number = |key: &str| probe.get(key).and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
    let (width, height) = (number("width"), number("height"));
    if width == 0 || height == 0 {
        return Err(MediaError::Failed(format!("Could not open input video: {}", input.display())));
    }
    let rate = ["avg_frame_rate", "r_frame_rate"].iter().filter_map(|k| probe.get(*k).and_then(|v| Ratio::parse(v))).find(Ratio::is_positive);
    let fps = rate.map_or(24.0, |r| r.as_f64());
    let rate_text = rate.map_or_else(|| "24".to_string(), |r| format!("{}/{}", r.num, r.den));
    let reported_frames = number("nb_frames");
    let crf = encode.crf.clamp(16, 35);
    let preset = normalize_encode_preset(encode.preset);
    let source_had_audio = has_audio(tools, input)?;
    let keep_audio = encode.preserve_audio && source_had_audio;

    let input_text = input.display().to_string();
    let temp_output = temp_beside(output);
    let decode_args = args(&["-loglevel", "error", "-i", &input_text, "-map", "0:v:0", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"]);
    let mut encode_args = args(&["-y", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", &format!("{width}x{height}"), "-framerate", &rate_text, "-i", "-"]);
    if keep_audio {
        // `apad` before `-shortest` ends the clip with its last frame. Plain `-shortest` cuts the last frame when the
        // audio is a few milliseconds shorter than the video, and a scene must keep its exact length.
        encode_args.extend(args(&["-i", &input_text, "-map", "0:v:0", "-map", "1:a:0", "-c:a", "aac", "-b:a", "192k", "-af", "apad", "-shortest"]));
    } else {
        encode_args.push("-an".into());
    }
    encode_args.extend(args(&["-c:v", "libx264", "-preset", &preset, "-crf", &crf.to_string(), "-pix_fmt", "yuv420p", "-movflags", "+faststart", &temp_output.display().to_string()]));

    let started = Instant::now();
    let result = run_pipeline(tools, &decode_args, &encode_args, width, height, &mut effect);
    let processed_frames = match result {
        Ok(frames) => frames,
        Err(e) => {
            let _ = fs::remove_file(&temp_output);
            return Err(e);
        }
    };
    if processed_frames == 0 {
        let _ = fs::remove_file(&temp_output);
        return Err(MediaError::Failed(format!("No frames could be read from the input video: {}", input.display())));
    }
    let output_has_audio = has_audio(tools, &temp_output).unwrap_or(false);
    fs::rename(&temp_output, output).or_else(|_| fs::copy(&temp_output, output).and_then(|_| fs::remove_file(&temp_output)))?;
    let elapsed = started.elapsed().as_secs_f64();
    Ok(json!({
        "width": width,
        "height": height,
        "fps": fps,
        "reported_frames": reported_frames,
        "processed_frames": processed_frames,
        "elapsed_seconds": elapsed,
        "processed_fps": if elapsed > 0.0 { processed_frames as f64 / elapsed } else { 0.0 },
        "audio_preserved": keep_audio && output_has_audio,
        "source_had_audio": source_had_audio,
        "preserve_audio": encode.preserve_audio,
        "encode_crf": crf,
        "encode_preset": preset,
        "encoder": "ffmpeg libx264",
        "browser_friendly": true,
    }))
}

/// Decode, change and encode every frame. The encoder writes the temp file named in `encode_args`.
fn run_pipeline(tools: &Tools, decode_args: &[String], encode_args: &[String], width: usize, height: usize, effect: &mut impl FnMut(&mut Rgb, usize)) -> Result<usize, MediaError> {
    // stderr goes to files so a chatty ffmpeg can never fill a pipe and stall the frame writes.
    let mut decoder_log = tempfile::tempfile()?;
    let mut encoder_log = tempfile::tempfile()?;
    let mut decoder = command(&tools.ffmpeg, decode_args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(decoder_log.try_clone()?).spawn()?;
    let mut encoder = match command(&tools.ffmpeg, encode_args).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(encoder_log.try_clone()?).spawn() {
        Ok(child) => child,
        Err(e) => {
            let _ = decoder.kill();
            let _ = decoder.wait();
            return Err(e.into());
        }
    };
    let mut frames_out = decoder.stdout.take().ok_or_else(|| MediaError::Failed("ffmpeg gave no frames.".into()))?;
    let mut frames_in = encoder.stdin.take().ok_or_else(|| MediaError::Failed("ffmpeg takes no frames.".into()))?;
    let mut bytes = vec![0u8; width * height * 3];
    let mut picture = Rgb::new(width, height);
    let mut count = 0usize;
    let mut failure: Option<MediaError> = None;
    loop {
        match read_frame(&mut frames_out, &mut bytes) {
            Ok(true) => {}
            Ok(false) => break,
            Err(e) => {
                failure = Some(e.into());
                break;
            }
        }
        picture.read_bytes(&bytes);
        effect(&mut picture, count);
        picture.write_bytes(&mut bytes);
        match frames_in.write_all(&bytes) {
            Ok(()) => count += 1,
            // The encoder stopped. Its log says why, below.
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => break,
            Err(e) => {
                failure = Some(e.into());
                break;
            }
        }
    }
    drop(frames_in);
    drop(frames_out);
    if failure.is_some() {
        let _ = decoder.kill();
        let _ = encoder.kill();
    }
    let decoder_status = decoder.wait()?;
    let encoder_status = encoder.wait()?;
    if let Some(e) = failure {
        return Err(e);
    }
    if !encoder_status.success() {
        let detail = log_tail(&mut encoder_log);
        return Err(MediaError::Failed(format!("ffmpeg could not encode the video: {}", if detail.is_empty() { format!("exit code {}", encoder_status.code().unwrap_or(-1)) } else { detail })));
    }
    if !decoder_status.success() && count == 0 {
        let detail = log_tail(&mut decoder_log);
        return Err(MediaError::Failed(format!("ffmpeg could not read the video: {}", if detail.is_empty() { format!("exit code {}", decoder_status.code().unwrap_or(-1)) } else { detail })));
    }
    Ok(count)
}
