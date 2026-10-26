//! Clip operations against `fixtures/media/media_cases.json`. Each case builds small synthetic inputs with ffmpeg, runs
//! the operation and compares what ffprobe reports about the result with what the pack's Python produced. Needs ffmpeg
//! and ffprobe on `PATH` (the tests say so and stop when they are missing).
#![cfg_attr(not(windows), allow(dead_code, unused_imports))] // the tests that use these helpers only run on Windows

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use h3_media::collect::{collect_scene_video, Existing};
use h3_media::stitch::{stitch_scene_videos, OverlayItem, SceneAudioItem, StitchRequest, TimingItem};
use h3_media::video::{apply_color_match, extract_final_frame, trim_scene_video, TrimRequest};
use h3_media::Tools;
use h3_project::Stamp;
use serde_json::{json, Value};
use sha1::{Digest, Sha1};

fn tools() -> Option<Tools> {
    let found = Tools::find("", "").ok();
    if found.is_none() {
        eprintln!("ffmpeg and ffprobe are not on PATH: media tests skipped");
    }
    found
}

fn fixture() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/media/media_cases.json");
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn ffmpeg_version(tools: &Tools) -> String {
    let out = Command::new(&tools.ffmpeg).arg("-version").output().unwrap();
    String::from_utf8_lossy(&out.stdout).lines().next().unwrap_or("").to_string()
}

fn stamp(t: u64) -> Stamp {
    Stamp {
        compact: format!("20260101_{:02}{:02}{:02}", t / 3600 % 24, t / 60 % 60, t % 60),
        iso: format!("2026-01-01T{:02}:{:02}:{:02}", t / 3600 % 24, t / 60 % 60, t % 60),
        epoch: 1_700_000_000.0 + t as f64 + 0.123,
    }
}

fn run_ffmpeg(tools: &Tools, work: &Path, args: &[String]) {
    let mut cmd = Command::new(&tools.ffmpeg);
    cmd.current_dir(work).args(["-y", "-v", "error"]).args(args);
    let out = cmd.output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

fn ffprobe_json(tools: &Tools, args: &[&str], path: &Path) -> Value {
    let out = Command::new(&tools.ffprobe).args(args).arg(path).output().unwrap();
    serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
}

/// The same facts the generator records about a file.
fn probe(tools: &Tools, path: &Path) -> Value {
    if !path.is_file() {
        return json!({});
    }
    let video = ffprobe_json(tools, &["-v", "error", "-select_streams", "v:0", "-count_packets", "-show_entries", "stream=nb_read_packets,r_frame_rate,width,height,duration", "-of", "json"], path);
    let audio = ffprobe_json(tools, &["-v", "error", "-select_streams", "a:0", "-show_entries", "stream=duration,sample_rate", "-of", "json"], path);
    let format = ffprobe_json(tools, &["-v", "error", "-show_entries", "format=duration", "-of", "json"], path);
    let v = &video["streams"][0];
    let a = &audio["streams"][0];
    let num = |value: &Value| value.as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
    json!({
        "frames": v["nb_read_packets"].as_str().and_then(|s| s.parse::<i64>().ok()).unwrap_or(0),
        "fps": v["r_frame_rate"],
        "width": v["width"],
        "height": v["height"],
        "video_duration": num(&v["duration"]),
        "has_audio": a.is_object(),
        "audio_duration": if a.is_object() { num(&a["duration"]) } else { 0.0 },
        "format_duration": num(&format["format"]["duration"]),
    })
}

fn audio_hash(tools: &Tools, path: &Path) -> String {
    let out = Command::new(&tools.ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-map", "0:a:0", "-ac", "2", "-ar", "48000", "-f", "s16le", "-"])
        .output()
        .unwrap();
    if !out.status.success() || out.stdout.is_empty() {
        return String::new();
    }
    Sha1::digest(&out.stdout).iter().map(|b| format!("{b:02x}")).collect()
}

fn same_probe(got: &Value, want: &Value, context: &str) {
    for key in ["frames", "fps", "width", "height", "has_audio"] {
        assert_eq!(got[key], want[key], "{context}: {key}");
    }
    for (key, tolerance) in [("video_duration", 0.002), ("audio_duration", 0.011), ("format_duration", 0.011)] {
        let (a, b) = (got[key].as_f64().unwrap_or(0.0), want[key].as_f64().unwrap_or(0.0));
        assert!((a - b).abs() <= tolerance, "{context}: {key} {a} vs {b}");
    }
}

fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).to_string_lossy().replace('\\', "/")
}

fn rel_opt(root: &Path, path: &Option<PathBuf>) -> String {
    path.as_ref().map(|p| rel(root, p)).unwrap_or_default()
}

fn tree(root: &Path) -> Vec<String> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) {
        for entry in fs::read_dir(dir).unwrap().filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, root, out);
            } else {
                out.push(rel(root, &path));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.retain(|t| !["song", "scene", "raw", "other", "silent", "insert"].iter().any(|p| t.starts_with(p)));
    out.sort();
    out
}

fn f(value: &Value, key: &str) -> f64 {
    value[key].as_f64().unwrap_or(0.0)
}

fn stitch_request(work: &Path, payload: &Value) -> StitchRequest {
    let path = |v: &Value| work.join(v.as_str().unwrap());
    let optional = |key: &str| payload[key].as_str().filter(|s| !s.is_empty()).map(|s| work.join(s));
    StitchRequest {
        project: work.join("proj"),
        scene_paths: payload["scene_paths"].as_array().unwrap().iter().map(path).collect(),
        audio_path: optional("audio_path"),
        song_path: optional("song_path"),
        scene_audio_items: vec![],
        scene_timing: payload["scene_timing_items"].as_array().unwrap().iter().map(|t| TimingItem { start: f(t, "start"), end: f(t, "end") }).collect(),
        timeline_fps: payload["timeline_fps"].as_u64().unwrap_or(0) as u32,
        use_embedded_scene_audio: payload["use_embedded_scene_audio"].as_bool().unwrap_or(false),
        overlays: payload["overlay_items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| OverlayItem { path: path(&o["path"]), start: f(o, "start"), end: f(o, "end"), source_start: f(o, "source_start") })
            .collect(),
        width: payload["width"].as_u64().unwrap_or(0) as u32,
        height: payload["height"].as_u64().unwrap_or(0) as u32,
        audio_start: f(payload, "audio_start"),
        audio_duration: f(payload, "audio_duration"),
        output_prefix: payload["output_prefix"].as_str().unwrap_or("FINAL_VIDEO").to_string(),
    }
}

// Its fixtures were made with the bundled ffmpeg build, whose clip lengths other ffmpeg versions do not repeat, so it only runs on Windows.
#[cfg(windows)]
#[test]
fn clip_operations_match_the_pack() {
    let Some(tools) = tools() else { return };
    let fixture = fixture();
    // Encoded audio only matches byte for byte when the same ffmpeg made it. Another build still has to match on frames,
    // lengths and sizes.
    let exact_audio = fixture["ffmpeg_version"].as_str() == Some(ffmpeg_version(&tools).as_str());
    if !exact_audio {
        eprintln!("ffmpeg differs from the one that made the fixtures: decoded audio is not compared");
    }
    let all = fixture["cases"].as_array().cloned().unwrap();
    assert!(all.len() >= 19);
    for case in all {
        let name = case["name"].as_str().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path();
        fs::create_dir_all(work.join("proj")).unwrap();
        for asset in case["assets"].as_array().unwrap() {
            let mut a: Vec<String> = asset["args"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
            a.push(asset["out"].as_str().unwrap().to_string());
            run_ffmpeg(&tools, work, &a);
        }
        let project = work.join("proj");
        for (step, want) in case["steps"].as_array().unwrap().iter().zip(case["results"].as_array().unwrap()) {
            let t = step["t"].as_u64().unwrap_or(0);
            let now = stamp(t);
            let context = format!("{name} {}", step["op"]);
            match step["op"].as_str().unwrap() {
                "trim" => {
                    let result = trim_scene_video(
                        &tools,
                        &TrimRequest {
                            project: project.clone(),
                            scene: step["scene"].as_u64().unwrap() as u32,
                            source: work.join(step["source"].as_str().unwrap()),
                            start: f(step, "start"),
                            duration: f(step, "duration"),
                            frames: step["frames"].as_u64().unwrap_or(0) as u32,
                            label: step["label"].as_str().unwrap_or("trim").to_string(),
                            mark_as_audio_video: step["mark_audio"].as_bool().unwrap_or(false),
                            restore_audio: None,
                        },
                        &now.compact,
                    )
                    .unwrap();
                    assert_eq!(rel(work, &result.video_path), want["video"], "{context}");
                    assert_eq!(rel_opt(work, &result.thumbnail_path), want["thumbnail"].as_str().unwrap(), "{context}");
                    same_probe(&probe(&tools, &result.video_path), &want["probe"], &context);
                    if exact_audio {
                        assert_eq!(json!(audio_hash(&tools, &result.video_path)), want["audio_hash"], "{context}: audio samples");
                    }
                }
                "collect" => {
                    let existing = if step["existing_action"].as_str() == Some("overwrite") { Existing::Overwrite } else { Existing::Backup };
                    let result = collect_scene_video(&tools, &project, step["scene"].as_u64().unwrap() as u32, &work.join(step["source"].as_str().unwrap()), existing, &now.compact, &now).unwrap();
                    assert_eq!(rel(work, &result.video_path), want["video"], "{context}");
                    assert_eq!(rel_opt(work, &result.thumbnail_path), want["thumbnail"].as_str().unwrap(), "{context}");
                    assert_eq!(rel_opt(work, &result.backup_path), want["backup"].as_str().unwrap(), "{context}");
                    assert_eq!(rel_opt(work, &result.backup_thumbnail_path), want["backup_thumbnail"].as_str().unwrap(), "{context}");
                    same_probe(&probe(&tools, &result.video_path), &want["probe"], &context);
                }
                "final_frame" => {
                    let image = extract_final_frame(&tools, &project, &work.join(step["source"].as_str().unwrap()), step["scene"].as_u64().unwrap() as u32, &now.compact).unwrap();
                    assert_eq!(rel(work, &image), want["image"], "{context}");
                    assert!(fs::metadata(&image).unwrap().len() > 0);
                }
                "color_match" => {
                    let video = work.join(step["video"].as_str().unwrap());
                    let result = apply_color_match(&tools, &video, &work.join(step["reference"].as_str().unwrap()), f(step, "fade"), f(step, "strength"), "token").unwrap();
                    assert_eq!(json!(result.applied), want["applied"], "{context}");
                    assert_eq!(rel_opt(work, &result.thumbnail_path), want["thumbnail"].as_str().unwrap(), "{context}");
                    same_probe(&probe(&tools, &video), &want["probe"], &context);
                }
                "stitch" => {
                    let result = stitch_scene_videos(&tools, &stitch_request(work, &step["payload"])).unwrap();
                    assert_eq!(rel(work, &result.final_video_path), want["final"], "{context}");
                    same_probe(&probe(&tools, &result.final_video_path), &want["probe"], &context);
                    assert_eq!(result.embedded_audio_mode, want["embedded_audio_mode"].as_str().unwrap(), "{context}");
                    assert_eq!(json!(result.normalized_canvas), want["normalized_canvas"], "{context}");
                    assert_eq!(json!(result.timeline_frame_sync), want["timeline_frame_sync"], "{context}");
                    assert_eq!(json!(result.timeline_frame_count), want["timeline_frame_count"], "{context}");
                    assert_eq!(json!(result.scene_count), want["scene_count"], "{context}");
                    assert_eq!(json!(result.insert_count), want["insert_count"], "{context}");
                    assert_eq!(json!(result.used_scene_audio), want["used_scene_audio"], "{context}");
                    assert_eq!(json!(result.output_width), want["output_width"], "{context}");
                    assert_eq!(json!(result.output_height), want["output_height"], "{context}");
                    if exact_audio {
                        assert_eq!(json!(audio_hash(&tools, &result.final_video_path)), want["audio_hash"], "{context}: decoded audio");
                    }
                }
                other => panic!("{other}"),
            }
        }
        assert_eq!(json!(tree(work)), case["tree"], "{name}: files left behind");
    }
    let _ = SceneAudioItem { path: PathBuf::new(), start: 0.0, duration: 0.0, embedded: false };
}
