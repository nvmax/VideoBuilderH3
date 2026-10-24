//! Pipeline plans and song scene boundaries against `fixtures/pipeline/plan.json`, recorded from the pack.

use std::path::Path;

use h3_pipeline::pipeline_plan::{pipeline_plan, plan_scene_boundaries};
use serde_json::Value;

fn load() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/pipeline/plan.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn plans_match_the_pack() {
    let data = load();
    let dir = tempfile::tempdir().unwrap();
    let audio = dir.path().join("song.wav");
    std::fs::write(&audio, b"RIFF").unwrap();
    let audio_text = audio.display().to_string();
    let cases = data["plans"].as_array().unwrap();
    assert!(cases.len() >= 100);
    for (n, case) in cases.iter().enumerate() {
        let name = case["session"].as_str().unwrap();
        let text = serde_json::to_string(&data["sessions"][name]).unwrap().replace("@exists@", &audio_text.replace('\\', "\\\\"));
        let session: Value = serde_json::from_str(&text).unwrap();
        let params = case["params"].as_object().unwrap();
        let got = pipeline_plan("proj", session.as_object().unwrap(), params);
        assert_eq!(got, case["return"], "case {n}: session {name} params {}", case["params"]);
    }
}

#[test]
fn song_scene_boundaries_match_the_pack() {
    let data = load();
    let cases = data["boundaries"].as_array().unwrap();
    assert!(cases.len() >= 150);
    for case in cases {
        let duration = case["duration"].as_f64().unwrap();
        let seconds = case["scene_seconds"].as_f64().unwrap_or(4.0);
        let got: Vec<Value> = plan_scene_boundaries(duration, seconds).into_iter().map(|(a, b)| serde_json::json!([a, b])).collect();
        assert_eq!(Value::Array(got), case["return"], "duration {duration} scene_seconds {}", case["scene_seconds"]);
    }
}
