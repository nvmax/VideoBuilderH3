//! Timeline and lyric scene logic against `fixtures/timeline`, made by the pack's own functions.

use std::fs;
use std::path::PathBuf;

use h3_core::lyric_scenes::*;
use h3_core::timeline::*;
use serde_json::{json, Value};

fn cases(name: &str) -> Vec<Value> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/timeline").join(format!("{name}.json"));
    serde_json::from_str::<Value>(&fs::read_to_string(path).unwrap()).unwrap()["cases"].as_array().cloned().unwrap()
}

/// Numbers compare by value to 1e-9, everything else exactly.
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => (x.as_f64().unwrap() - y.as_f64().unwrap()).abs() < 1e-9,
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q)),
        (Value::Object(x), Value::Object(y)) => x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w))),
        _ => a == b,
    }
}

fn list(v: &Value) -> Vec<Value> {
    v.as_array().cloned().unwrap_or_default()
}

fn check(got: &Value, want: &Value, what: &str, case: &Value) {
    assert!(same(got, want), "{what} differs\n got  {got}\n want {want}\n case {case}");
}

#[test]
fn normalize_matches() {
    for c in cases("normalize") {
        let mut segs = list(&c["input"]);
        normalize_segments(&mut segs, c["active_index"].as_u64().unwrap() as usize);
        check(&Value::Array(segs), &c["output"], "segments", &c);
    }
}

#[test]
fn move_and_resize_match() {
    for c in cases("move_resize") {
        let mut segs = list(&c["input"]);
        let scene = c["scene"].as_str().unwrap();
        let result = if c["op"] == "move" {
            move_scene_timing(&mut segs, scene, c["start"].as_f64().unwrap(), c["ripple"].as_bool().unwrap())
        } else {
            resize_scene_timing(&mut segs, scene, c["duration"].as_f64(), c["end"].as_f64(), c["ripple"].as_bool().unwrap())
        };
        if c.get("error").is_some() {
            assert!(result.is_err(), "{c}");
            continue;
        }
        let delta = result.unwrap_or_else(|e| panic!("{e} {c}"));
        check(&Value::Array(segs), &c["segments"], "segments", &c);
        check(&json!(delta), &c["return"]["delta"], "delta", &c);
    }
}

#[test]
fn gaps_match() {
    for c in cases("gaps") {
        let mut segs = list(&c["input"]);
        let mut overlay = list(&c["overlay"]);
        let ret = if c["op"] == "close_all" {
            close_all_base_timeline_gaps(&mut segs, Some(&mut overlay))
        } else {
            close_base_timeline_gap(&mut segs, Some(&mut overlay), c["start"].as_f64().unwrap(), c["end"].as_f64().unwrap())
        };
        check(&Value::Array(segs), &c["output"], "segments", &c);
        check(&Value::Array(overlay), &c["overlay_output"], "overlay", &c);
        check(&json!(ret), &c["return"], "return", &c);
    }
}

#[test]
fn labels_snap_and_validate_match() {
    for c in cases("labels_snap_validate") {
        match c["op"].as_str().unwrap() {
            "labels" => {
                let mut segs = list(&c["input"]);
                renumber_generic_base_scene_labels(&mut segs);
                check(&Value::Array(segs), &c["output"], "labels", &c);
            }
            "validate" => {
                let mut segs = list(&c["input"]);
                let got = validate_timeline_consistency(&mut segs, c["audio_duration"].as_f64());
                check(&Value::Array(got), &c["return"], "issues", &c);
            }
            _ => {
                let got = snap_to_beat(c["t"].as_f64().unwrap(), &list(&c["beats"]), 0.14);
                check(&json!(got), &c["return"], "snap", &c);
            }
        }
    }
}

#[test]
fn bulk_text_parses_like_the_pack() {
    for c in cases("bulk_parse") {
        let got = parse_bulk_scenes(c["text"].as_str().unwrap(), c["mode"].as_str().unwrap());
        match (got, c.get("error")) {
            (Err(e), Some(want)) => assert_eq!(&json!(e), want, "{c}"),
            (Ok(v), None) => check(&Value::Array(v), &c["return"], "scenes", &c),
            (got, _) => panic!("{got:?} vs {c}"),
        }
    }
}

#[test]
fn lyric_helpers_match() {
    for c in cases("lyrics") {
        match c["op"].as_str().unwrap() {
            "instrumental" => assert_eq!(json!(is_instrumental_lyric_text(c["text"].as_str().unwrap())), c["return"], "{c}"),
            "split" => {
                let (a, b) = split_lyric_text(c["text"].as_str().unwrap(), c["fraction"].as_f64().unwrap());
                assert_eq!(json!([a, b]), c["return"], "{c}");
            }
            "merge" => assert_eq!(json!(merge_lyric_text(c["a"].as_str().unwrap(), c["b"].as_str().unwrap())), c["return"], "{c}"),
            _ => {
                let ranges: Vec<(f64, f64)> = list(&c["ranges"]).iter().map(|r| (r[0].as_f64().unwrap(), r[1].as_f64().unwrap())).collect();
                let got: Vec<Value> = carry_lyrics_over(&list(&c["old"]), &ranges).into_iter().map(Value::Object).collect();
                check(&Value::Array(got), &c["return"], "carry", &c);
            }
        }
    }
}

#[test]
fn length_fixes_match() {
    for c in cases("length_fix") {
        let locked: Vec<String> = list(&c["locked"]).iter().map(|v| v.as_str().unwrap().to_string()).collect();
        let segs = list(&c["input"]);
        let got = next_length_fix(&segs, c["min"].as_f64().unwrap(), c["max"].as_f64().unwrap(), &locked);
        if let Some(want) = c.get("error") {
            assert_eq!(&json!(got.unwrap_err()), want);
            continue;
        }
        let fix = got.unwrap();
        let want = &c["return"];
        match (&fix, want.is_null()) {
            (None, true) => continue,
            (Some(LengthFix::Split { scene_id, at_time }), false) => {
                assert_eq!((want["op"].as_str(), want["scene_id"].as_str()), (Some("split"), Some(scene_id.as_str())), "{c}");
                assert!((want["at_time"].as_f64().unwrap() - at_time).abs() < 1e-9);
            }
            (Some(LengthFix::Merge { scene_id, with }), false) => {
                assert_eq!((want["op"].as_str(), want["scene_id"].as_str(), want["with"].as_str()), (Some("merge"), Some(scene_id.as_str()), Some(*with)), "{c}");
            }
            other => panic!("{other:?} vs {c}"),
        }
        let mut applied = segs.clone();
        let mut n = 0;
        apply_length_fix(&mut applied, fix.as_ref().unwrap(), &mut || {
            n += 1;
            format!("seg_new{n}")
        })
        .unwrap();
        // The pack made new ids from one counter across all cases, so only compare ids that are not generated.
        let strip = |v: &mut Value| {
            for s in v.as_array_mut().unwrap() {
                if s["id"].as_str().is_some_and(|i| i.starts_with("seg_new")) {
                    s["id"] = json!("NEW");
                }
            }
        };
        let (mut a, mut w) = (Value::Array(applied), c["applied"].clone());
        strip(&mut a);
        strip(&mut w);
        check(&a, &w, "applied", &c);
    }
}
