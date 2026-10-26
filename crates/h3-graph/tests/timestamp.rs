//! The lyric timing graph against `fixtures/timeline/timestamp_graph.json`, recorded from the pack.
#![cfg_attr(not(windows), allow(dead_code, unused_imports))] // the tests that use these helpers only run on Windows

use std::path::Path;

use h3_graph::timestamp::timestamp_graph;
use serde_json::{json, Value};

// Its fixtures hold Windows paths, so it only runs on Windows.
#[cfg(windows)]
#[test]
fn timestamp_graphs_match_the_pack() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/timeline/timestamp_graph.json");
    let cases: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert!(cases.len() >= 6);
    let dir = std::env::temp_dir().join(format!("h3_timestamp_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let audio = dir.join("song.wav");
    std::fs::write(&audio, b"RIFF").unwrap();
    let real = h3_graph::inputs::abspath(&audio.display().to_string());
    for (n, case) in cases.iter().enumerate() {
        let mut request = case["request"].as_object().unwrap().clone();
        match request.get("audio_path").and_then(Value::as_str) {
            Some("<MISSING>") => {
                request.insert("audio_path".into(), json!(dir.join("missing.wav").display().to_string()));
            }
            _ => {
                request.insert("audio_path".into(), json!(real));
            }
        }
        let got = timestamp_graph(&request);
        match (case["return"].get("prompt"), case["return"].get("error")) {
            (Some(expected), _) => {
                let text = serde_json::to_string(&Value::Object(got.unwrap_or_else(|e| panic!("case {n}: {e}")))).unwrap();
                let escaped = serde_json::to_string(&real).unwrap();
                let text = text.replace(&escaped[1..escaped.len() - 1], "<AUDIO>");
                assert_eq!(&serde_json::from_str::<Value>(&text).unwrap(), expected, "case {n}");
            }
            (_, Some(error)) => {
                let message = got.unwrap_err().to_string();
                let message = message.replace(&dir.display().to_string(), "<TMP>");
                assert_eq!(message.trim_matches('\''), error.as_str().unwrap().trim_matches('\''), "case {n}");
            }
            _ => panic!("case {n}"),
        }
    }
    std::fs::remove_dir_all(&dir).ok();
}
