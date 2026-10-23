//! Pipeline decisions against `fixtures/pipeline`: the stitch layout, how a clip is recorded on its scene and which
//! continuity a render used.

use std::fs;
use std::path::PathBuf;

use h3_core::pyval::Obj;
use h3_pipeline::scene_video::{apply_scene_video, continuity_used};
use h3_pipeline::stitch_plan::build_stitch_request;
use serde_json::{json, Value};

fn cases(name: &str) -> Vec<Value> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/pipeline").join(format!("{name}.json"));
    let value: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    value["cases"].as_array().cloned().unwrap()
}

/// Numbers compare by value, everything else exactly.
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q)),
        (Value::Object(x), Value::Object(y)) => x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w))),
        _ => a == b,
    }
}

fn rel(folder: &std::path::Path, path: &std::path::Path) -> String {
    path.strip_prefix(folder).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_else(|_| path.display().to_string())
}

#[test]
fn stitch_layouts() {
    for case in cases("stitch_payload") {
        let name = case["name"].as_str().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("proj");
        fs::create_dir_all(folder.join("clips")).unwrap();
        for file in ["song.wav", "other.wav"] {
            fs::write(folder.join(file), b"x").unwrap();
        }
        let mut session: Obj = case["session"].as_object().unwrap().clone();
        if let Some(Value::Array(segments)) = session.get_mut("segments") {
            for segment in segments {
                for key in ["video_path", "rendered_video_path"] {
                    if let Some(path) = segment.get(key).and_then(Value::as_str).filter(|p| !p.is_empty()).map(str::to_string) {
                        let full = folder.join(&path);
                        fs::write(&full, b"v").unwrap();
                        segment[key] = json!(full.display().to_string());
                    }
                }
            }
        }
        if let Some(path) = session.get("audio_path").and_then(Value::as_str).filter(|p| !p.is_empty()).map(str::to_string) {
            session.insert("audio_path".into(), json!(folder.join(path).display().to_string()));
        }
        let mut params: Obj = case["params"].as_object().unwrap().clone();
        if let Some(path) = params.get("audio_path").and_then(Value::as_str).map(str::to_string) {
            params.insert("audio_path".into(), json!(folder.join(path).display().to_string()));
        }
        let got = build_stitch_request(&folder, &session, &params, case["strict"].as_bool().unwrap());
        match (got, case.get("error")) {
            (Err(message), Some(want)) => assert_eq!(json!(message), *want, "{name}"),
            (Ok((request, summary)), None) => {
                let payload = &case["payload"];
                let paths: Vec<String> = request.scene_paths.iter().map(|p| rel(&folder, p)).collect();
                assert_eq!(json!(paths), payload["scene_paths"], "{name}: scene paths");
                let audio = request.audio_path.as_ref().map(|p| rel(&folder, p)).unwrap_or_default();
                assert_eq!(json!(audio), payload["audio_path"], "{name}: audio path");
                let song = request.song_path.as_ref().map(|p| rel(&folder, p)).unwrap_or_default();
                assert_eq!(json!(song), payload["song_path"], "{name}: song path");
                let timing: Vec<Value> = request.scene_timing.iter().map(|t| json!({"start": t.start, "end": t.end})).collect();
                assert!(same(&json!(timing), &payload["scene_timing_items"]), "{name}: timing {timing:?}");
                assert_eq!(json!(request.timeline_fps), payload["timeline_fps"], "{name}");
                assert_eq!(json!(request.use_embedded_scene_audio), payload["use_embedded_scene_audio"], "{name}");
                let overlays: Vec<Value> = request
                    .overlays
                    .iter()
                    .map(|o| json!({"path": o.path.display().to_string(), "start": o.start, "end": o.end, "source_start": o.source_start}))
                    .collect();
                assert!(same(&json!(overlays), &payload["overlay_items"]), "{name}: overlays");
                assert_eq!((request.width, request.height), (0, 0), "{name}");
                assert!(same(&json!(request.audio_start), &payload["audio_start"]), "{name}: audio start");
                assert!(same(&json!(request.audio_duration), &payload["audio_duration"]), "{name}: audio duration");
                assert_eq!(json!(request.output_prefix), payload["output_prefix"], "{name}");
                let want = &case["summary"];
                assert_eq!(json!(summary.scene_ids), want["scene_ids"], "{name}");
                assert_eq!(json!(summary.scene_numbers), want["scene_numbers"], "{name}");
                assert_eq!(json!(summary.skipped_scene_ids), want["skipped_scene_ids"], "{name}");
                assert_eq!(json!(summary.audio), want["audio"], "{name}");
                assert!(same(&json!(summary.audio_start), &want["audio_start"]), "{name}");
                assert!(same(&json!(summary.audio_duration), &want["audio_duration"]), "{name}");
                assert_eq!(json!(summary.expected_frame_count), want["expected_frame_count"], "{name}");
            }
            (got, want) => panic!("{name}: got ok={} but the pack said {want:?}", got.is_ok()),
        }
    }
}

#[test]
fn recording_a_clip_on_its_scene() {
    for case in cases("apply_scene_video") {
        let input = &case["input"];
        let mut segment: Obj = input["segment"].as_object().unwrap().clone();
        // The frozen clock of the generator: 1_700_000_005.123 s.
        let now_millis = (1_700_000_005.123_f64 * 1000.0) as i64;
        apply_scene_video(&mut segment, input["video"].as_str().unwrap(), input["thumbnail"].as_str().unwrap(), now_millis);
        assert!(same(&Value::Object(segment), &case["output"]), "{input}");
    }
}

#[test]
fn continuity_a_render_used() {
    let segments = vec![json!({"id": "a"}), json!({"id": "b"}), json!({"id": "c"})];
    for case in cases("continuity_used") {
        let i = &case["input"];
        let report = if i["report"].is_null() { None } else { Some(&i["report"]) };
        let got = continuity_used(i["mode"].as_str().unwrap(), report, &segments, i["scene_number"].as_u64().unwrap() as usize);
        assert_eq!(Value::Object(got), case["output"], "{i}");
    }
}
