//! Timed Timeline Notes against `fixtures/timeline/markers.json`, recorded from the pack.

mod common;

use common::load;
use h3_core::timeline_markers::{create_marker, delete_marker, normalize_markers, overlapping_scene_ids, update_marker, MarkerError};
use serde_json::{json, Value};

fn counter() -> impl FnMut() -> String {
    let mut n = 0;
    move || {
        n += 1;
        format!("mark_{n}")
    }
}

fn as_values(markers: &[serde_json::Map<String, Value>]) -> Value {
    Value::Array(markers.iter().map(|m| Value::Object(m.clone())).collect())
}

#[test]
fn timeline_notes_match_the_pack() {
    let cases = load("timeline/markers.json");
    let cases = cases.as_array().unwrap();
    assert!(cases.len() >= 45);
    for (n, case) in cases.iter().enumerate() {
        let label = format!("case {n}: {} {}", case["op"], case.get("fields").or(case.get("raw")).unwrap_or(&Value::Null));
        match case["op"].as_str().unwrap() {
            "normalize" => {
                let mut ids = counter();
                let got = normalize_markers(Some(&case["raw"]), &mut ids);
                assert_eq!(as_values(&got), case["return"], "{label}");
            }
            "create" => {
                let mut ids = counter();
                let mut markers = normalize_markers(Some(&case["markers"]), &mut ids);
                let mut ids = counter();
                match (create_marker(&mut markers, &case["fields"], &mut ids), case["return"].get("error")) {
                    (Ok(note), None) => {
                        assert_eq!(Value::Object(note), case["return"]["note"], "{label}");
                        assert_eq!(as_values(&markers), case["return"]["markers"], "{label}");
                    }
                    (Err(e), Some(error)) => assert_eq!(e.to_string(), error.as_str().unwrap(), "{label}"),
                    (got, want) => panic!("{label}: {got:?} vs {want:?}"),
                }
            }
            "update" => {
                let mut ids = counter();
                let mut markers = normalize_markers(Some(&case["markers"]), &mut ids);
                let id = case["id"].as_str().unwrap();
                match (update_marker(&mut markers, id, &case["fields"]), &case["return"]) {
                    (Ok(note), want) if want.get("note").is_some() => {
                        assert_eq!(Value::Object(note), want["note"], "{label}");
                        assert_eq!(as_values(&markers), want["markers"], "{label}");
                    }
                    (Err(MarkerError::Invalid(m)), want) => assert_eq!(&json!(m), &want["error"], "{label}"),
                    (Err(MarkerError::NotFound(m)), want) => assert_eq!(&json!(m), &want["missing"], "{label}"),
                    (got, want) => panic!("{label}: {got:?} vs {want:?}"),
                }
            }
            "delete" => {
                let mut ids = counter();
                let mut markers = normalize_markers(Some(&case["markers"]), &mut ids);
                match (delete_marker(&mut markers, case["id"].as_str().unwrap()), &case["return"]) {
                    (Ok(note), want) => {
                        assert_eq!(Value::Object(note), want["note"], "{label}");
                        assert_eq!(as_values(&markers), want["markers"], "{label}");
                    }
                    (Err(MarkerError::NotFound(m)), want) => assert_eq!(&json!(m), &want["missing"], "{label}"),
                    (got, _) => panic!("{label}: {got:?}"),
                }
            }
            "overlap" => {
                let marker = case["marker"].as_object().unwrap();
                let segments = case["segments"].as_array().unwrap();
                assert_eq!(json!(overlapping_scene_ids(marker, segments)), case["return"], "{label}");
            }
            other => panic!("unknown op {other}"),
        }
    }
}
