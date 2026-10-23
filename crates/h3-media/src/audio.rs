//! Audio length and scene audio cutting. Port of `_probe_media_duration_seconds` and
//! `_trim_minimax_h3_audio_context` in `runner/minimax_inputs.py`.

use std::fs;
use std::path::{Path, PathBuf};

use h3_core::timing::TimingPlan;

use crate::tools::{failure_text, run, MediaError, Tools};

/// Length of a media file in seconds, from ffprobe.
pub fn probe_duration(tools: &Tools, path: &str) -> Result<f64, MediaError> {
    let args: Vec<String> = ["-v", "error", "-show_entries", "format=duration", "-of", "default=noprint_wrappers=1:nokey=1", path]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let output = run(&tools.ffprobe, &args)?;
    if !output.status.success() {
        return Err(MediaError::Failed(failure_text(&output, "FFprobe could not read the audio duration.")));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let duration: f64 = text
        .lines()
        .next()
        .and_then(|l| l.trim().parse().ok())
        .ok_or_else(|| MediaError::Failed(format!("FFprobe did not return a valid duration for: {path}")))?;
    if duration <= 0.0 {
        return Err(MediaError::Failed(format!("Source audio has no usable duration: {path}")));
    }
    Ok(duration)
}

/// Length of a PCM WAV file from its header, or `None` when it is not a readable WAV.
pub fn wav_duration(path: &Path) -> Option<f64> {
    let bytes = fs::read(path).ok()?;
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let (mut rate, mut block, mut data): (u32, u32, Option<u64>) = (0, 0, None);
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().ok()?) as u64;
        let body = pos + 8;
        if id == b"fmt " && body + 16 <= bytes.len() {
            rate = u32::from_le_bytes(bytes[body + 4..body + 8].try_into().ok()?);
            block = u16::from_le_bytes(bytes[body + 12..body + 14].try_into().ok()?) as u32;
        } else if id == b"data" {
            // A streamed file can report a size larger than what is there.
            data = Some(size.min((bytes.len() - body) as u64));
            break;
        }
        pos = body + size as usize + (size as usize & 1);
    }
    if rate == 0 || block == 0 {
        return None;
    }
    Some(data? as f64 / block as f64 / rate as f64)
}

/// The audio the render needs: the scene's context cut out of the song as 44.1 kHz stereo WAV in the project's
/// `minimax_h3_scene_audio` folder. Returns `{audio_path, start, duration, requested_duration, format}`.
pub fn trim_scene_audio(tools: &Tools, source: &str, project: &str, scene: u32, timing: &TimingPlan) -> Result<serde_json::Value, MediaError> {
    let dir = Path::new(project).join("minimax_h3_scene_audio");
    fs::create_dir_all(&dir)?;
    let target: PathBuf = dir.join(format!("scene_audio_{scene:04}.wav"));
    let mut args: Vec<String> = vec![
        "-y".into(),
        "-ss".into(),
        format!("{:.9}", timing.audio_trim_start_seconds),
        "-i".into(),
        source.into(),
        "-t".into(),
        format!("{:.9}", timing.audio_trim_duration_seconds),
        "-vn".into(),
        "-ac".into(),
        "2".into(),
        "-ar".into(),
        "44100".into(),
        "-c:a".into(),
        "pcm_s16le".into(),
    ];
    if timing.audio_leading_padding_seconds > 0.0 {
        args.push("-af".into());
        args.push(format!("adelay={:.9}:all=1", timing.audio_leading_padding_seconds * 1000.0));
    }
    args.push(target.display().to_string());
    let output = run(&tools.ffmpeg, &args)?;
    if !output.status.success() || !target.is_file() {
        return Err(MediaError::Failed(failure_text(&output, "FFmpeg failed to trim MiniMax H3 scene audio.")));
    }
    let actual = wav_duration(&target)
        .ok_or_else(|| MediaError::Failed(format!("Could not verify the trimmed MiniMax H3 audio: {}", target.display())))?;
    if actual + 0.02 < timing.audio_trim_duration_seconds {
        return Err(MediaError::Failed(format!(
            "The trimmed MiniMax H3 audio ended before the required scene context. Needed {:.3}s; received {:.3}s.",
            timing.audio_trim_duration_seconds, actual
        )));
    }
    Ok(serde_json::json!({
        "audio_path": target.display().to_string(),
        "start": timing.audio_trim_start_seconds,
        "duration": actual,
        "requested_duration": timing.audio_trim_duration_seconds,
        "format": "pcm_s16le_wav",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(seconds: f64) -> Vec<u8> {
        let (rate, channels) = (44_100u32, 2u16);
        let frames = (seconds * rate as f64) as u32;
        let data = frames * u32::from(channels) * 2;
        let mut out = Vec::new();
        out.extend(b"RIFF");
        out.extend((36 + data).to_le_bytes());
        out.extend(b"WAVEfmt ");
        out.extend(16u32.to_le_bytes());
        out.extend(1u16.to_le_bytes());
        out.extend(channels.to_le_bytes());
        out.extend(rate.to_le_bytes());
        out.extend((rate * u32::from(channels) * 2).to_le_bytes());
        out.extend((channels * 2).to_le_bytes());
        out.extend(16u16.to_le_bytes());
        out.extend(b"data");
        out.extend(data.to_le_bytes());
        out.extend(vec![0u8; data as usize]);
        out
    }

    #[test]
    fn wav_length_comes_from_the_header() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.wav");
        fs::write(&path, wav(1.5)).unwrap();
        assert!((wav_duration(&path).unwrap() - 1.5).abs() < 1e-6);
        fs::write(&path, b"not a wav").unwrap();
        assert_eq!(wav_duration(&path), None);
    }
}
