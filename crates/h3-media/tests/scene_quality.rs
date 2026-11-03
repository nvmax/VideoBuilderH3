//! Scene copies and stitch re-encodes keep the render's quality instead of x264's CRF 23 / veryfast default.
//!
//! A MiniMax H3 render is CRF 19, but the trimmed scene copy (and with it the stitched final video) came out as
//! `rc=crf crf=23.0` with AAC at its default bitrate, because the ffmpeg commands passed no `-crf` and no `-b:a`.
//! The tests read the CRF back from the x264 settings string inside the files and the audio bitrate with ffprobe.
//! They need ffmpeg and ffprobe on `PATH`; the transition test also needs a graphics adapter, like the other
//! transition stitch tests.

use std::fs;
use std::path::Path;

use h3_media::stitch::{stitch_scene_videos, StitchRequest, TimingItem};
use h3_media::tools::{args, probe_x264_crf, run, scene_audio_args, scene_video_args, scene_video_crf, source_crf, Tools, SCENE_AUDIO_BITRATE, SCENE_VIDEO_CRF};
use h3_media::transition::TransitionItem;
use h3_media::video::{trim_scene_video, TrimRequest};
use serde_json::json;

const FPS: u32 = 24;
const _: () = assert!(SCENE_VIDEO_CRF < 19, "scene copies must not be coarser than the CRF 19 render");

fn tools() -> Tools {
    Tools::find("", "").expect("ffmpeg and ffprobe on PATH")
}

fn ffmpeg(tools: &Tools, a: &[&str]) {
    let out = run(&tools.ffmpeg, &args(a)).unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

/// A two second clip written at `crf` with stereo noise for sound, like the MiniMax H3 output the trim reads (CRF 19).
/// Noise, not a sine: AAC spends only the bits the signal needs, so a pure tone would not reach the requested bitrate.
fn render_at(tools: &Tools, path: &Path, source: &str, crf: u32) {
    ffmpeg(tools, &[
        "-y", "-f", "lavfi", "-i", source, "-f", "lavfi", "-i", "anoisesrc=color=pink:sample_rate=48000:amplitude=0.5",
        "-t", "2", "-ac", "2", "-c:v", "libx264", "-crf", &crf.to_string(), "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "128k",
        &path.display().to_string(),
    ]);
}

fn render(tools: &Tools, path: &Path, source: &str) {
    render_at(tools, path, source, 19);
}

fn song(tools: &Tools, path: &Path) {
    ffmpeg(tools, &["-y", "-f", "lavfi", "-i", "anoisesrc=color=pink:sample_rate=48000:amplitude=0.5", "-t", "4", "-ac", "2", &path.display().to_string()]);
}

/// The CRF of every x264 encode in the file, read from the settings string x264 writes into each stream.
fn x264_crfs(path: &Path) -> Vec<f64> {
    let data = fs::read(path).unwrap();
    let needle = b"x264 - core";
    let (mut found, mut at) = (Vec::new(), 0);
    while let Some(pos) = data[at..].windows(needle.len()).position(|w| w == needle) {
        let start = at + pos;
        let end = data[start..].iter().position(|b| *b == 0).map_or(data.len(), |n| start + n);
        let text = String::from_utf8_lossy(&data[start..end]);
        if let Some(value) = text.split_whitespace().find_map(|t| t.strip_prefix("crf=")) {
            found.push(value.parse().unwrap());
        }
        at = end.max(start + needle.len());
    }
    found
}

fn audio_bitrate(tools: &Tools, path: &Path) -> u64 {
    let out = run(&tools.ffprobe, &args(&["-v", "error", "-select_streams", "a:0", "-show_entries", "stream=bit_rate", "-of", "csv=p=0", &path.display().to_string()])).unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
}

fn assert_all_at_scene_crf(path: &Path) {
    assert_all_at_crf(path, f64::from(SCENE_VIDEO_CRF));
}

fn assert_all_at_crf(path: &Path, want: f64) {
    let crfs = x264_crfs(path);
    assert!(!crfs.is_empty(), "{} has no x264 settings string", path.display());
    assert!(crfs.iter().all(|crf| *crf == want), "{}: crf {crfs:?}, want {want}", path.display());
}

#[test]
fn the_encoder_arguments_set_crf_preset_and_bitrate() {
    let video = scene_video_args(None);
    assert_eq!(video[video.iter().position(|a| a == "-crf").unwrap() + 1], SCENE_VIDEO_CRF.to_string());
    let audio = scene_audio_args();
    assert_eq!(audio[audio.iter().position(|a| a == "-b:a").unwrap() + 1], SCENE_AUDIO_BITRATE);
}

#[test]
fn a_copy_is_never_coarser_than_its_source() {
    assert_eq!(scene_video_crf(None), SCENE_VIDEO_CRF);
    assert_eq!(scene_video_crf(Some(19.0)), SCENE_VIDEO_CRF, "a CRF 19 render gets the default, which is finer");
    assert_eq!(scene_video_crf(Some(12.0)), 12, "a finer source keeps its own CRF");
    assert_eq!(scene_video_crf(Some(12.9)), 12, "a fractional CRF rounds toward the finer side");
    assert_eq!(scene_video_crf(Some(-4.0)), 0);
    assert_eq!(scene_video_crf(Some(f64::NAN)), SCENE_VIDEO_CRF);
}

#[test]
fn the_crf_of_a_file_is_read_from_the_file() {
    let tools = tools();
    let dir = tempfile::tempdir().unwrap();
    let (a, b, audio) = (dir.path().join("a.mp4"), dir.path().join("b.mp4"), dir.path().join("song.wav"));
    render_at(&tools, &a, "testsrc2=size=320x180:rate=24", 19);
    render_at(&tools, &b, "testsrc2=size=320x180:rate=24", 10);
    song(&tools, &audio);
    assert_eq!(probe_x264_crf(&a), Some(19.0));
    assert_eq!(probe_x264_crf(&b), Some(10.0));
    assert_eq!(probe_x264_crf(&audio), None, "a file with no x264 stream states no CRF");
    assert_eq!(probe_x264_crf(&dir.path().join("missing.mp4")), None);
    assert_eq!(source_crf(&[a.clone(), b.clone()]), Some(10.0), "the lowest of several");
    assert_eq!(source_crf::<std::path::PathBuf>(&[]), None);
}

#[test]
fn a_finer_render_keeps_its_crf_in_the_copy() {
    let tools = tools();
    let dir = tempfile::tempdir().unwrap();
    let fine = dir.path().join("fine.mp4");
    render_at(&tools, &fine, "testsrc2=size=320x180:rate=24", 10);
    let request = TrimRequest {
        project: dir.path().join("proj"),
        scene: 1,
        source: fine,
        start: 0.0,
        duration: 1.5,
        frames: 36,
        label: "minimax_exact".into(),
        mark_as_audio_video: true,
        restore_audio: None,
    };
    let result = trim_scene_video(&tools, &request, "20260101_000000").unwrap();
    assert_all_at_crf(&result.video_path, 10.0);
}

#[test]
fn a_stitch_of_finer_copies_keeps_their_crf() {
    let tools = tools();
    let dir = tempfile::tempdir().unwrap();
    let (a, b, audio) = (dir.path().join("a.mp4"), dir.path().join("b.mp4"), dir.path().join("song.wav"));
    render_at(&tools, &a, "testsrc2=size=320x180:rate=24", 12);
    render_at(&tools, &b, "testsrc2=size=320x180:rate=24", 12);
    song(&tools, &audio);
    let request = StitchRequest {
        project: dir.path().join("proj"),
        scene_paths: vec![a, b],
        audio_path: Some(audio),
        scene_timing: vec![TimingItem { start: 0.0, end: 1.0 }, TimingItem { start: 1.0, end: 2.0 }],
        timeline_fps: FPS,
        output_prefix: "FINAL_VIDEO".into(),
        ..Default::default()
    };
    let result = stitch_scene_videos(&tools, &request).unwrap();
    assert!(result.timeline_frame_sync);
    assert_all_at_crf(&result.final_video_path, 12.0);
}

#[test]
fn a_trimmed_scene_copy_keeps_quality() {
    let tools = tools();
    let dir = tempfile::tempdir().unwrap();
    let raw = dir.path().join("raw.mp4");
    render(&tools, &raw, &format!("testsrc2=size=320x180:rate={FPS}"));
    let request = TrimRequest {
        project: dir.path().join("proj"),
        scene: 1,
        source: raw,
        start: 0.0,
        duration: 1.5,
        frames: 36,
        label: "minimax_exact".into(),
        mark_as_audio_video: true,
        restore_audio: None,
    };
    let result = trim_scene_video(&tools, &request, "20260101_000000").unwrap();
    assert_all_at_scene_crf(&result.video_path);
    assert!(audio_bitrate(&tools, &result.video_path) >= 200_000);
}

#[test]
fn a_timeline_synced_stitch_keeps_quality() {
    let tools = tools();
    let dir = tempfile::tempdir().unwrap();
    let (a, b, audio) = (dir.path().join("a.mp4"), dir.path().join("b.mp4"), dir.path().join("song.wav"));
    render(&tools, &a, &format!("testsrc2=size=320x180:rate={FPS}"));
    render(&tools, &b, &format!("testsrc2=size=320x180:rate={FPS}"));
    song(&tools, &audio);
    let request = StitchRequest {
        project: dir.path().join("proj"),
        scene_paths: vec![a, b],
        audio_path: Some(audio),
        scene_timing: vec![TimingItem { start: 0.0, end: 1.0 }, TimingItem { start: 1.0, end: 2.0 }],
        timeline_fps: FPS,
        output_prefix: "FINAL_VIDEO".into(),
        ..Default::default()
    };
    let result = stitch_scene_videos(&tools, &request).unwrap();
    assert!(result.timeline_frame_sync);
    assert_all_at_scene_crf(&result.final_video_path);
    assert!(audio_bitrate(&tools, &result.final_video_path) >= 200_000, "the final mux keeps its audio bitrate");
}

#[test]
fn a_canvas_fit_keeps_quality() {
    let tools = tools();
    let dir = tempfile::tempdir().unwrap();
    let (a, audio) = (dir.path().join("a.mp4"), dir.path().join("song.wav"));
    render(&tools, &a, &format!("testsrc2=size=320x180:rate={FPS}"));
    song(&tools, &audio);
    let request = StitchRequest {
        project: dir.path().join("proj"),
        scene_paths: vec![a],
        audio_path: Some(audio),
        width: 640,
        height: 360,
        output_prefix: "FINAL_VIDEO".into(),
        ..Default::default()
    };
    let result = stitch_scene_videos(&tools, &request).unwrap();
    assert!(result.normalized_canvas);
    assert_all_at_scene_crf(&result.final_video_path);
}

#[test]
fn a_transition_clip_keeps_quality() {
    let tools = tools();
    let dir = tempfile::tempdir().unwrap();
    let (a, b, audio) = (dir.path().join("a.mp4"), dir.path().join("b.mp4"), dir.path().join("song.wav"));
    render(&tools, &a, &format!("color=c=red:s=320x180:r={FPS}"));
    render(&tools, &b, &format!("color=c=blue:s=320x180:r={FPS}"));
    song(&tools, &audio);
    let request = StitchRequest {
        project: dir.path().join("proj"),
        scene_paths: vec![a, b],
        audio_path: Some(audio),
        scene_timing: vec![TimingItem { start: 0.0, end: 2.0 }, TimingItem { start: 2.0, end: 4.0 }],
        timeline_fps: FPS,
        transitions: vec![TransitionItem { name: "WipeLeft".into(), params: json!({}), start: 1.75, seconds: 0.5 }],
        output_prefix: "FINAL_VIDEO".into(),
        ..Default::default()
    };
    let result = stitch_scene_videos(&tools, &request).unwrap();
    assert_eq!(result.transitions.len(), 1, "the transition was drawn");
    // The scene pieces and the transition clip are separate encodes joined without re-encoding: every one counts.
    let crfs = x264_crfs(&result.final_video_path);
    assert!(crfs.len() >= 2, "scene pieces and transition clip: {crfs:?}");
    assert_all_at_scene_crf(&result.final_video_path);
}
