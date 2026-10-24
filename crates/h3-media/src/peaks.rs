//! Waveform peaks, beats and tempo. Port of the ffmpeg path of `_read_audio_peaks` and of the peak based beat estimate
//! in `builder/audio.py`.
//!
//! The pack reads peaks with torchaudio when it can and falls back to ffmpeg. This port always decodes with ffmpeg
//! (at the song's own rate) and cuts RMS peaks the way torchaudio's path does. The pack's beat tracking uses librosa when it is installed and the
//! peak based estimate otherwise. The app tracks beats itself (`beats.rs`, a port of the librosa path) and keeps the peak based estimate as the fallback.

use std::path::Path;

use crate::tools::{MediaError, Tools};

const SAMPLE_RATE: u32 = 16_000;

/// What the waveform view and beat detection need from a song.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioPeaks {
    pub duration: f64,
    pub sample_rate: u32,
    pub channels: u32,
    pub peaks: Vec<f64>,
}

/// RMS peaks of mono samples, `target` of them at most. As the pack's torchaudio reader does, the last slice is padded with
/// zeros, so there are always `target` peaks unless the song has fewer samples.
pub fn peaks_from_samples(samples: &[f32], target: usize, sample_rate: u32) -> AudioPeaks {
    let total = samples.len();
    if total == 0 {
        return AudioPeaks { duration: 0.0, sample_rate, channels: 1, peaks: vec![] };
    }
    let count = target.clamp(1, total);
    let per_peak = total.div_ceil(count).max(1);
    let peaks = (0..count)
        .map(|i| {
            let slice = samples.get(i * per_peak..).map_or(&[][..], |rest| &rest[..per_peak.min(rest.len())]);
            let sum: f64 = slice.iter().map(|v| f64::from(*v) * f64::from(*v)).sum();
            (sum / per_peak as f64).sqrt().clamp(0.0, 1.0)
        })
        .collect();
    AudioPeaks { duration: total as f64 / f64::from(sample_rate), sample_rate, channels: 1, peaks }
}

/// The sample rate and channel count of a file's first audio stream, from ffprobe.
pub(crate) fn stream_format(tools: &Tools, path: &Path) -> Option<(u32, u32)> {
    let args: Vec<String> = ["-v", "error", "-select_streams", "a:0", "-show_entries", "stream=sample_rate,channels", "-of", "csv=p=0", &path.display().to_string()]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let output = tools.ffprobe_checked(&args, "ffprobe could not read the audio").ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let mut fields = text.trim().split(',');
    let rate = fields.next()?.trim().parse::<u32>().ok().filter(|r| *r > 0)?;
    let channels = fields.next()?.trim().parse::<u32>().ok().filter(|c| *c > 0)?;
    Some((rate, channels))
}

/// Decode a file with ffmpeg and cut it into RMS peaks. Like the pack's reader (torchaudio first), the song is read at
/// its own sample rate and the channels are averaged, so the peaks are as loud as the pack's and their number follows the
/// same count of samples.
pub fn read_audio_peaks(tools: &Tools, path: &Path, target: usize) -> Result<AudioPeaks, MediaError> {
    let probed = stream_format(tools, path);
    let (rate, channels) = probed.unwrap_or((SAMPLE_RATE, 1));
    // Native channels, so ffmpeg does not mix them (it scales a mono file by 0.7 when it makes stereo). Without a probe the
    // file is mixed down to mono, as the pack's fallback does.
    let mut args: Vec<String> = ["-hide_banner", "-loglevel", "error", "-i", &path.display().to_string(), "-vn"].iter().map(|s| s.to_string()).collect();
    if probed.is_none() {
        args.extend(["-ac", "1"].iter().map(|s| s.to_string()));
    }
    args.extend(["-ar", &rate.to_string(), "-f", "f32le", "pipe:1"].iter().map(|s| s.to_string()));
    let output = tools.ffmpeg_checked(&args, "ffmpeg could not read the audio")?;
    let width = channels as usize * 4;
    let samples: Vec<f32> = output
        .stdout
        .chunks_exact(width)
        .map(|frame| frame.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).sum::<f32>() / channels as f32)
        .collect();
    let mut info = peaks_from_samples(&samples, target, rate);
    info.channels = channels;
    Ok(info)
}

fn round_to(value: f64, digits: i32) -> f64 {
    h3_core::pyval::round_to(value, digits)
}

/// Beat times (seconds) from the peaks: local maxima above `mean + 0.65 std`, at least a minimum gap apart.
pub fn estimate_beats_from_peaks(peaks: &[f64], duration: f64) -> Vec<f64> {
    if peaks.len() < 8 || duration <= 0.0 {
        return vec![];
    }
    let step = duration / peaks.len() as f64;
    let mean = peaks.iter().sum::<f64>() / peaks.len() as f64;
    let variance = peaks.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / peaks.len() as f64;
    let threshold = mean + variance.max(0.0).sqrt() * 0.65;
    let min_gap = (duration / 500.0).clamp(0.22, 0.55);
    let mut beats: Vec<f64> = Vec::new();
    let mut values: Vec<f64> = Vec::new();
    let mut last_time = -999.0;
    for index in 1..peaks.len() - 1 {
        let value = peaks[index];
        if value < threshold || value < peaks[index - 1] || value < peaks[index + 1] {
            continue;
        }
        let time = index as f64 * step;
        if time - last_time < min_gap {
            if let (Some(last_beat), Some(last_value)) = (beats.last_mut(), values.last_mut()) {
                if value > *last_value {
                    *last_beat = round_to(time, 3);
                    *last_value = value;
                    last_time = time;
                }
            }
            continue;
        }
        beats.push(round_to(time, 3));
        values.push(value);
        last_time = time;
    }
    beats
}

/// Tempo from the median gap between beats. 0 when there are not enough beats.
pub fn tempo_from_beat_times(beats: &[f64]) -> f64 {
    let mut values: Vec<f64> = beats.iter().copied().filter(|v| v.is_finite()).collect();
    values.sort_by(f64::total_cmp);
    let mut intervals: Vec<f64> = values.windows(2).map(|w| w[1] - w[0]).filter(|d| *d > 0.05).collect();
    if intervals.is_empty() {
        return 0.0;
    }
    intervals.sort_by(f64::total_cmp);
    let mid = intervals.len() / 2;
    let median = if intervals.len() % 2 == 1 { intervals[mid] } else { (intervals[mid - 1] + intervals[mid]) / 2.0 };
    if median > 0.0 { round_to(60.0 / median, 6) } else { 0.0 }
}
