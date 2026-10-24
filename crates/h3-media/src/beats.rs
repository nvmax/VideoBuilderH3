//! Beat tracking, natively. Port of the librosa path of `_estimate_beats_from_audio` in the pack's `builder/audio.py`, which is
//! `librosa.onset.onset_strength` + `librosa.beat.beat_track(trim=False)` + `librosa.onset.onset_backtrack`.
//!
//! 1. The song is read as mono at 22,050 Hz.
//! 2. Onset strength: how much each mel band got louder from one 23 ms frame to the next, averaged over the bands.
//! 3. Tempo: the lag at which the onset strength repeats itself (autocorrelation over 8 second windows), weighted towards 120 BPM.
//! 4. Beats: Ellis's dynamic programme. It picks the frames with strong onsets that are about one beat period apart, so it
//!    keeps a steady grid through quiet passages, which a simple "loud moment" rule cannot.
//! 5. Each beat moves back to the start of its onset, so the marker sits on the front of the hit.
//!
//! The loudness peaks estimate in `peaks.rs` is the fallback, as in the pack, for a file that cannot be decoded or has no onsets.

use std::f64::consts::PI;
use std::path::Path;

use h3_core::pyval::{round_half_even, round_to};
use rustfft::num_complex::Complex;
use rustfft::FftPlanner;

use crate::peaks::{estimate_beats_from_peaks, stream_format, tempo_from_beat_times};
use crate::tools::{MediaError, Tools};

pub const SAMPLE_RATE: u32 = 22_050;
const N_FFT: usize = 2048;
const HOP: usize = 512;
const N_MELS: usize = 128;
const START_BPM: f64 = 120.0;
const MAX_TEMPO: f64 = 320.0;
const TIGHTNESS: f64 = 100.0;
const TOP_DB: f32 = 80.0;
/// `time_to_frames(8 s)`: the length of the autocorrelation windows.
const AC_FRAMES: usize = 344;
const AC_FFT: usize = 1024;

/// What the tracker found: the tempo and the beats in seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct Tracked {
    pub tempo_bpm: f64,
    pub beats: Vec<f64>,
    /// The onset strength the beats were picked from, one value per 512 samples. Kept for tests.
    pub onset: Vec<f32>,
}

// -- mel spectrogram ---------------------------------------------------------------------------------------------------------

/// Slaney's mel scale, as `librosa.hz_to_mel(htk=False)`.
fn hz_to_mel(hz: f64) -> f64 {
    const SP: f64 = 200.0 / 3.0;
    const MIN_LOG_HZ: f64 = 1000.0;
    if hz >= MIN_LOG_HZ {
        MIN_LOG_HZ / SP + (hz / MIN_LOG_HZ).ln() / (6.4f64.ln() / 27.0)
    } else {
        hz / SP
    }
}

fn mel_to_hz(mel: f64) -> f64 {
    const SP: f64 = 200.0 / 3.0;
    const MIN_LOG_HZ: f64 = 1000.0;
    let min_log_mel = MIN_LOG_HZ / SP;
    if mel >= min_log_mel {
        MIN_LOG_HZ * ((6.4f64.ln() / 27.0) * (mel - min_log_mel)).exp()
    } else {
        SP * mel
    }
}

/// One triangular mel filter: the first FFT bin it covers and its weights from there.
struct MelFilter {
    first: usize,
    weights: Vec<f32>,
}

/// `librosa.filters.mel(sr=22050, n_fft=2048, n_mels=128, fmin=0, fmax=11025, htk=False, norm="slaney")`, without the zeros.
fn mel_filters() -> Vec<MelFilter> {
    let bins = N_FFT / 2 + 1;
    let (low, high) = (hz_to_mel(0.0), hz_to_mel(f64::from(SAMPLE_RATE) / 2.0));
    let points = N_MELS + 2;
    let centers: Vec<f64> = (0..points)
        .map(|i| if i + 1 == points { high } else { low + (high - low) / (points - 1) as f64 * i as f64 })
        .map(mel_to_hz)
        .collect();
    let fft_freqs: Vec<f64> = (0..bins).map(|k| k as f64 * f64::from(SAMPLE_RATE) / N_FFT as f64).collect();
    (0..N_MELS)
        .map(|i| {
            let (left_step, right_step) = (centers[i + 1] - centers[i], centers[i + 2] - centers[i + 1]);
            let scale = 2.0 / (centers[i + 2] - centers[i]);
            let weights: Vec<f64> = fft_freqs.iter().map(|f| ((f - centers[i]) / left_step).min((centers[i + 2] - f) / right_step).max(0.0) * scale).collect();
            let first = weights.iter().position(|w| *w > 0.0).unwrap_or(0);
            let last = weights.iter().rposition(|w| *w > 0.0).map_or(first, |p| p + 1);
            MelFilter { first, weights: weights[first..last.max(first)].iter().map(|w| *w as f32).collect() }
        })
        .collect()
}

fn hann(length: usize) -> Vec<f32> {
    (0..length).map(|i| (0.5 - 0.5 * (2.0 * PI * i as f64 / length as f64).cos()) as f32).collect()
}

/// The mel spectrogram in decibels, `frames` rows of `N_MELS` values: `power_to_db(melspectrogram(power=2))`.
fn mel_db(samples: &[f32]) -> Vec<f32> {
    let frames = 1 + samples.len() / HOP;
    let window = hann(N_FFT);
    let filters = mel_filters();
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(N_FFT);
    let mut spectrum = vec![Complex::new(0.0f32, 0.0); N_FFT];
    let mut power = vec![0.0f32; N_FFT / 2 + 1];
    let mut out = vec![0.0f32; frames * N_MELS];
    for frame in 0..frames {
        // The signal is padded with n_fft / 2 zeros on both sides, so frame t starts at t * hop - n_fft / 2.
        let start = (frame * HOP) as isize - (N_FFT / 2) as isize;
        for (i, slot) in spectrum.iter_mut().enumerate() {
            let at = start + i as isize;
            let value = if at >= 0 { samples.get(at as usize).copied().unwrap_or(0.0) } else { 0.0 };
            *slot = Complex::new(value * window[i], 0.0);
        }
        fft.process(&mut spectrum);
        for (k, p) in power.iter_mut().enumerate() {
            *p = spectrum[k].re * spectrum[k].re + spectrum[k].im * spectrum[k].im;
        }
        for (m, filter) in filters.iter().enumerate() {
            let sum: f64 = filter.weights.iter().enumerate().map(|(j, w)| f64::from(*w) * f64::from(power[filter.first + j])).sum();
            out[frame * N_MELS + m] = 10.0 * (sum as f32).max(1e-10).log10();
        }
    }
    let loudest = out.iter().copied().fold(f32::MIN, f32::max);
    let floor = loudest - TOP_DB;
    out.iter_mut().for_each(|v| *v = v.max(floor));
    out
}

/// `librosa.onset.onset_strength`: the rise in each mel band from the frame before, averaged over the bands.
fn onset_strength(samples: &[f32]) -> Vec<f32> {
    let db = mel_db(samples);
    let frames = db.len() / N_MELS;
    let mut env = vec![0.0f32; frames];
    // The rise is measured between frames t - 1 and t, and shifted by `1 + n_fft / (2 * hop)` frames to line up with the audio.
    let shift = 1 + N_FFT / (2 * HOP);
    for t in 1..frames {
        let rise: f64 = (0..N_MELS).map(|m| f64::from((db[t * N_MELS + m] - db[(t - 1) * N_MELS + m]).max(0.0))).sum::<f64>() / N_MELS as f64;
        if t - 1 + shift < frames {
            env[t - 1 + shift] = rise as f32;
        }
    }
    env
}

// -- tempo -------------------------------------------------------------------------------------------------------------------

/// The mean of the normalised autocorrelations of the onset strength in 8 second windows (`librosa.feature.tempogram`).
fn tempogram_mean(onset: &[f32]) -> Vec<f64> {
    let n = onset.len();
    let half = AC_FRAMES / 2;
    // Both ends ramp down to zero, as `np.pad(mode="linear_ramp")` does.
    let mut padded = vec![0.0f64; n + 2 * half];
    let (first, last) = (f64::from(onset[0]), f64::from(onset[n - 1]));
    for k in 0..half {
        padded[k] = first * k as f64 / half as f64;
        padded[half + n + k] = last * (half - 1 - k) as f64 / half as f64;
    }
    for (i, v) in onset.iter().enumerate() {
        padded[half + i] = f64::from(*v);
    }
    let window = hann(AC_FRAMES);
    let mut planner = FftPlanner::<f32>::new();
    let forward = planner.plan_fft_forward(AC_FFT);
    let inverse = planner.plan_fft_inverse(AC_FFT);
    let mut buffer = vec![Complex::new(0.0f32, 0.0); AC_FFT];
    let mut sum = vec![0.0f64; AC_FRAMES];
    for t in 0..n {
        for (i, slot) in buffer.iter_mut().enumerate() {
            *slot = if i < AC_FRAMES { Complex::new(padded[t + i] as f32 * window[i], 0.0) } else { Complex::new(0.0, 0.0) };
        }
        forward.process(&mut buffer);
        for slot in buffer.iter_mut() {
            *slot = Complex::new(slot.re * slot.re + slot.im * slot.im, 0.0);
        }
        inverse.process(&mut buffer);
        let strongest = buffer[..AC_FRAMES].iter().map(|c| c.re.abs()).fold(0.0f32, f32::max);
        if strongest >= f32::MIN_POSITIVE {
            for (lag, slot) in sum.iter_mut().enumerate() {
                *slot += f64::from(buffer[lag].re / strongest);
            }
        }
    }
    sum.iter().map(|v| v / n as f64).collect()
}

/// The tempo in BPM (`librosa.feature.tempo` with its defaults): the autocorrelation lag with the most support, weighted by a
/// log-normal prior around 120 BPM, with nothing above 320 BPM.
fn tempo(onset: &[f32]) -> f64 {
    let mean = tempogram_mean(onset);
    let rate = f64::from(SAMPLE_RATE) / HOP as f64;
    let mut best = (f64::NEG_INFINITY, 0usize);
    for (lag, value) in mean.iter().enumerate().skip(1) {
        let bpm = 60.0 * rate / lag as f64;
        if bpm >= MAX_TEMPO {
            continue;
        }
        let prior = -0.5 * (bpm.log2() - START_BPM.log2()).powi(2);
        let score = (1.0e6 * value).ln_1p() + prior;
        if score > best.0 {
            best = (score, lag);
        }
    }
    60.0 * rate / best.1.max(1) as f64
}

// -- beats -------------------------------------------------------------------------------------------------------------------

/// The onset strength in units of its standard deviation, smoothed with a Gaussian one beat wide (`__beat_local_score`).
fn local_score(onset: &[f32], frames_per_beat: f64) -> Vec<f64> {
    let n = onset.len();
    let mean = onset.iter().map(|v| f64::from(*v)).sum::<f64>() / n as f64;
    let deviation = (onset.iter().map(|v| (f64::from(*v) - mean).powi(2)).sum::<f64>() / (n as f64 - 1.0).max(1.0)).sqrt();
    let scale = deviation + f64::from(f32::MIN_POSITIVE);
    let normal: Vec<f64> = onset.iter().map(|v| f64::from((f64::from(*v) / scale) as f32)).collect();
    let width = frames_per_beat as i64;
    let window: Vec<f64> = (-width..=width).map(|k| (-0.5 * (k as f64 * 32.0 / frames_per_beat).powi(2)).exp()).collect();
    let k = window.len();
    (0..n)
        .map(|i| {
            let from = (i + k / 2 + 1).saturating_sub(n);
            let to = (i + k / 2).min(k);
            (from..to).map(|j| window[j] * normal[i + k / 2 - j]).sum()
        })
        .collect()
}

/// Ellis's dynamic programme: the best score of a chain of beats ending at each frame, and where its previous beat is.
fn beat_chain(score: &[f64], frames_per_beat: f64) -> (Vec<i64>, Vec<f64>) {
    let n = score.len();
    let (mut backlink, mut cumulative) = (vec![-1i64; n], vec![0.0f64; n]);
    let threshold = 0.01 * score.iter().copied().fold(f64::MIN, f64::max);
    let mut first = true;
    cumulative[0] = score[0];
    let nearest = round_half_even(frames_per_beat / 2.0);
    let farthest = (2.0 * frames_per_beat) as i64;
    for i in 0..n {
        let (mut best, mut location) = (f64::NEG_INFINITY, -1i64);
        let mut at = i as i64 - nearest;
        while at >= i as i64 - farthest {
            if at < 0 {
                break;
            }
            let gap = (i as i64 - at) as f64;
            let candidate = cumulative[at as usize] - TIGHTNESS * (gap.ln() - frames_per_beat.ln()).powi(2);
            if candidate > best {
                best = candidate;
                location = at;
            }
            at -= 1;
        }
        cumulative[i] = if location >= 0 { score[i] + best } else { score[i] };
        if first && score[i] < threshold {
            backlink[i] = -1;
        } else {
            backlink[i] = location;
            first = false;
        }
    }
    (backlink, cumulative)
}

/// The frame the last beat is in: the last local maximum of the cumulative score that reaches half the median of the maxima.
fn last_beat(cumulative: &[f64]) -> usize {
    let n = cumulative.len();
    let maxima: Vec<bool> = (0..n)
        .map(|i| match i {
            0 => false,
            _ if i == n - 1 => cumulative[i] > cumulative[i - 1],
            _ => cumulative[i] > cumulative[i - 1] && cumulative[i] >= cumulative[i + 1],
        })
        .collect();
    let mut values: Vec<f64> = (0..n).filter(|i| maxima[*i]).map(|i| cumulative[i]).collect();
    values.sort_by(f64::total_cmp);
    let median = match values.len() {
        0 => return n - 1,
        len if len % 2 == 1 => values[len / 2],
        len => (values[len / 2 - 1] + values[len / 2]) / 2.0,
    };
    let threshold = 0.5 * median;
    (0..n).rev().find(|i| maxima[*i] && cumulative[*i] >= threshold).unwrap_or(n - 1)
}

/// The beat frames for a tempo.
fn track_frames(onset: &[f32], bpm: f64) -> Vec<usize> {
    let rate = f64::from(SAMPLE_RATE) / HOP as f64;
    let frames_per_beat = round_half_even(rate * 60.0 / bpm) as f64;
    let score = local_score(onset, frames_per_beat);
    let (backlink, cumulative) = beat_chain(&score, frames_per_beat);
    let mut beat = vec![false; onset.len()];
    let mut at = last_beat(&cumulative) as i64;
    while at >= 0 {
        beat[at as usize] = true;
        at = backlink[at as usize];
    }
    // No beats where the smoothed onset strength is zero, at either end.
    let mut start = 0;
    while start < score.len() && score[start] <= 0.0 {
        beat[start] = false;
        start += 1;
    }
    let mut end = score.len();
    while end > start && score[end - 1] <= 0.0 {
        beat[end - 1] = false;
        end -= 1;
    }
    beat.iter().enumerate().filter(|(_, b)| **b).map(|(i, _)| i).collect()
}

/// `librosa.onset.onset_backtrack`: each frame moves back to the nearest earlier frame where the onset strength stops falling
/// and starts rising, the front edge of the hit.
fn backtrack(frames: &[usize], onset: &[f32]) -> Vec<usize> {
    let mut minima = vec![0usize];
    for i in 0..onset.len().saturating_sub(2) {
        if onset[i + 1] <= onset[i] && onset[i + 1] < onset[i + 2] {
            minima.push(i + 1);
        }
    }
    frames.iter().map(|frame| minima[minima.partition_point(|m| m <= frame) - 1]).collect()
}

/// Track the beats of mono samples at 22,050 Hz. `None` when the audio has no onsets at all.
pub fn track(samples: &[f32], duration: f64) -> Option<Tracked> {
    if samples.len() < 2 {
        return None;
    }
    let onset = onset_strength(samples);
    if onset.len() < 2 || !onset.iter().any(|v| *v != 0.0) {
        return None;
    }
    let bpm = tempo(&onset);
    let frames = backtrack(&track_frames(&onset, bpm), &onset);
    let maximum = if duration > 0.0 { duration } else { samples.len() as f64 / f64::from(SAMPLE_RATE) };
    let mut beats: Vec<f64> = Vec::new();
    for frame in frames {
        let time = round_to(frame as f64 * HOP as f64 / f64::from(SAMPLE_RATE), 3);
        if time < 0.0 || (maximum > 0.0 && time > maximum + 0.001) {
            continue;
        }
        if beats.last().is_none_or(|last| time > *last) {
            beats.push(time);
        }
    }
    Some(Tracked { tempo_bpm: bpm, beats, onset })
}

/// Read a song as mono samples at 22,050 Hz with ffmpeg: resampled, channels averaged.
pub fn read_samples(tools: &Tools, path: &Path) -> Result<Vec<f32>, MediaError> {
    let probed = stream_format(tools, path);
    let channels = probed.map_or(1, |(_, channels)| channels) as usize;
    let mut args: Vec<String> = ["-hide_banner", "-loglevel", "error", "-i", &path.display().to_string(), "-vn"].iter().map(|s| s.to_string()).collect();
    if probed.is_none() {
        args.extend(["-ac", "1"].iter().map(|s| s.to_string()));
    }
    args.extend(["-ar", &SAMPLE_RATE.to_string(), "-f", "f32le", "pipe:1"].iter().map(|s| s.to_string()));
    let output = tools.ffmpeg_checked(&args, "ffmpeg could not read the audio")?;
    let width = channels * 4;
    Ok(output.stdout.chunks_exact(width).map(|frame| frame.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).sum::<f32>() / channels as f32).collect())
}

/// `_estimate_beats_from_audio(include_tempo=True)`: the beats and the tempo of a song. Falls back to the loudness peaks when the
/// song cannot be read or has no onsets.
pub fn estimate_beats(tools: &Tools, path: &Path, peaks: &[f64], duration: f64) -> (Vec<f64>, f64) {
    if let Ok(samples) = read_samples(tools, path) {
        if let Some(found) = track(&samples, duration) {
            if !found.beats.is_empty() {
                let bpm = if found.tempo_bpm.is_finite() && found.tempo_bpm > 0.0 { round_to(found.tempo_bpm, 6) } else { 0.0 };
                let bpm = if bpm > 0.0 { bpm } else { tempo_from_beat_times(&found.beats) };
                return (found.beats, bpm);
            }
        }
    }
    let beats = estimate_beats_from_peaks(peaks, duration);
    let bpm = tempo_from_beat_times(&beats);
    (beats, bpm)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A click track: a short decaying burst on every beat.
    fn clicks(bpm: f64, seconds: f64) -> Vec<f32> {
        let total = (seconds * f64::from(SAMPLE_RATE)) as usize;
        let mut samples = vec![0.0f32; total];
        let period = 60.0 / bpm * f64::from(SAMPLE_RATE);
        let mut at = 0.5 * f64::from(SAMPLE_RATE);
        while (at as usize) < total {
            for i in 0..2000usize {
                if let Some(slot) = samples.get_mut(at as usize + i) {
                    *slot += ((i as f64 * 0.31).sin() * (-(i as f64) / 300.0).exp()) as f32 * 0.8;
                }
            }
            at += period;
        }
        samples
    }

    #[test]
    fn the_mel_filters_cover_the_spectrum_and_are_slaney_scaled() {
        let filters = mel_filters();
        assert_eq!(filters.len(), N_MELS);
        assert!(filters.iter().all(|f| !f.weights.is_empty() && f.weights.iter().all(|w| *w >= 0.0)));
        // Slaney scaling keeps the weight of the first filter small and of the last ones much smaller than the peak of the first.
        let area = |f: &MelFilter| f.weights.iter().map(|w| f64::from(*w)).sum::<f64>();
        assert!(area(&filters[0]) > 0.0 && area(&filters[N_MELS - 1]) > 0.0);
        assert!((hz_to_mel(mel_to_hz(25.0)) - 25.0).abs() < 1e-9);
        assert!((hz_to_mel(1000.0) - 15.0).abs() < 1e-9);
    }

    #[test]
    fn a_click_track_gets_its_tempo_and_a_beat_on_every_click() {
        let samples = clicks(120.0, 16.0);
        let found = track(&samples, 16.0).expect("beats");
        // The tempo is a whole number of frames per beat, so it moves in steps of about 5 BPM here: 117.45 or 123.05 for 120.
        assert!((found.tempo_bpm - 120.0).abs() < 4.0, "tempo {}", found.tempo_bpm);
        // The clicks are every half second from 0.5 s. The marker sits on the front of each hit.
        let spacing: Vec<f64> = found.beats.windows(2).map(|w| w[1] - w[0]).collect();
        let median = {
            let mut sorted = spacing.clone();
            sorted.sort_by(f64::total_cmp);
            sorted[sorted.len() / 2]
        };
        assert!((median - 0.5).abs() < 0.03, "median spacing {median}");
        assert!(found.beats.len() >= 28, "{} beats", found.beats.len());
        for beat in &found.beats {
            let nearest = ((beat - 0.5) / 0.5).round() * 0.5 + 0.5;
            assert!((beat - nearest).abs() < 0.06, "beat {beat} is not on a click");
        }
    }

    /// The samples of a 16 bit mono WAV file written by `tools/make_beat_fixtures.py`.
    fn read_wav(path: &std::path::Path) -> Vec<f32> {
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(&bytes[..4], b"RIFF");
        let at = bytes.windows(4).position(|w| w == b"data").expect("data chunk") + 8;
        bytes[at..].as_chunks::<2>().0.iter().map(|b| f32::from(i16::from_le_bytes(*b)) / 32768.0).collect()
    }

    #[test]
    fn the_tracker_matches_librosa_on_the_fixture_songs() {
        let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/audio");
        let cases: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(folder.join("beats.json")).unwrap()).unwrap();
        let cases = cases["cases"].as_array().unwrap();
        assert!(cases.len() >= 5);
        for case in cases {
            let name = case["file"].as_str().unwrap();
            let samples = read_wav(&folder.join(name));
            let found = track(&samples, case["duration"].as_f64().unwrap()).unwrap_or_else(|| panic!("{name}: no beats"));
            // The onset strength is what everything else is built on, so it is checked first.
            assert_eq!(found.onset.len() as u64, case["onset_length"].as_u64().unwrap(), "{name}: onset frames");
            let head = case["onset_head"].as_array().unwrap();
            for (i, expected) in head.iter().enumerate() {
                let (got, want) = (f64::from(found.onset[i]), expected.as_f64().unwrap());
                assert!((got - want).abs() <= 1e-3 * want.abs().max(1.0), "{name}: onset[{i}] {got} against {want}");
            }
            for (k, expected) in case["onset_sample"].as_array().unwrap().iter().enumerate() {
                let (got, want) = (f64::from(found.onset[k * 37]), expected.as_f64().unwrap());
                assert!((got - want).abs() <= 1e-3 * want.abs().max(1.0), "{name}: onset[{}] {got} against {want}", k * 37);
            }
            let sum: f64 = found.onset.iter().map(|v| f64::from(*v)).sum();
            let want_sum = case["onset_sum"].as_f64().unwrap();
            assert!((sum - want_sum).abs() <= 1e-3 * want_sum, "{name}: onset sum {sum} against {want_sum}");
            assert!((round_to(found.tempo_bpm, 6) - case["tempo_bpm"].as_f64().unwrap()).abs() < 1e-3, "{name}: tempo {} against {}", found.tempo_bpm, case["tempo_bpm"]);
            let expected: Vec<f64> = case["beats"].as_array().unwrap().iter().map(|b| b.as_f64().unwrap()).collect();
            assert_eq!(found.beats, expected, "{name}: beats");
        }
    }

    #[test]
    fn silence_has_no_beats() {
        assert!(track(&vec![0.0f32; 44_100], 2.0).is_none());
        assert!(track(&[], 0.0).is_none());
        assert!(track(&[0.5], 0.0).is_none());
    }

    #[test]
    fn a_slower_track_is_found_at_its_own_tempo() {
        let found = track(&clicks(90.0, 20.0), 20.0).expect("beats");
        assert!((found.tempo_bpm - 90.0).abs() < 2.5, "tempo {}", found.tempo_bpm);
    }
}
