//! Peaks, beats and tempo against `fixtures/media/peaks_cases.json`, made by the pack's own functions.

use std::path::PathBuf;

use h3_media::peaks::{estimate_beats_from_peaks, read_audio_peaks, tempo_from_beat_times};
use h3_media::Tools;
use serde_json::Value;

#[test]
fn peaks_beats_and_tempo_match_the_pack() {
    let media = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/media");
    let cases: Value = serde_json::from_str(&std::fs::read_to_string(media.join("peaks_cases.json")).unwrap()).unwrap();
    let tools = Tools::find("ffmpeg", "ffprobe").expect("ffmpeg on PATH");
    for case in cases["cases"].as_array().unwrap() {
        let target = case["target"].as_u64().unwrap() as usize;
        let info = read_audio_peaks(&tools, &media.join("pulses.wav"), target).unwrap();
        let want: Vec<f64> = case["peaks"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
        assert_eq!(info.peaks.len(), want.len(), "target {target}: count");
        for (i, (a, b)) in info.peaks.iter().zip(&want).enumerate() {
            assert!((a - b).abs() < 1e-6, "target {target}: peak {i}: {a} vs {b}");
        }
        assert!((info.duration - case["duration"].as_f64().unwrap()).abs() < 1e-9);
        let beats = estimate_beats_from_peaks(&info.peaks, info.duration);
        let want_beats: Vec<f64> = case["beats"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
        assert_eq!(beats, want_beats, "target {target}: beats");
        assert!((tempo_from_beat_times(&beats) - case["tempo"].as_f64().unwrap()).abs() < 1e-9, "target {target}: tempo");
    }
}
