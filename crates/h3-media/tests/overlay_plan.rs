//! The overlay filter graph against `fixtures/cases/overlay_render_cases.json` (the pack's own copy, `tests/overlay_render_cases.json`, is the same file). The expected
//! text was written by this planner and read by hand, then every graph was run in a real ffmpeg (`overlay_render.rs`). Run with
//! `H3_BLESS_OVERLAY=1` to write the expected text again after a deliberate change, and read the diff before keeping it.

use std::fs;
use std::path::PathBuf;

use h3_core::overlay_layers::normalize_overlay_layers;
use h3_media::overlay::{ffmpeg_args, plan_overlay, OverlaySource, GraphArg};
use serde_json::{json, Value};

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/cases/overlay_render_cases.json")
}

fn source(fixture: &Value) -> OverlaySource {
    let s = &fixture["source"];
    OverlaySource { width: s["width"].as_u64().unwrap() as u32, height: s["height"].as_u64().unwrap() as u32, fps: s["fps"].as_f64().unwrap(), duration: s["duration"].as_f64().unwrap() }
}

fn names(fixture: &Value, key: &str) -> Vec<String> {
    fixture[key].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect()
}

fn plan_of(fixture: &Value, clips: &Value) -> h3_media::overlay::OverlayPlan {
    let files = names(fixture, "files");
    let luts = names(fixture, "luts");
    let fonts = fixture["fonts"].as_object().cloned().unwrap_or_default();
    plan_overlay(&normalize_overlay_layers(clips), &source(fixture), &|path| files.iter().any(|f| f == path), &|name| luts.iter().any(|l| l == name), &|name| fonts.get(name).and_then(Value::as_str).map(String::from))
}

fn results(fixture: &Value) -> (Vec<Value>, Vec<Value>) {
    let plans = fixture["plan_cases"].as_array().unwrap().iter().map(|case| plan_of(fixture, &case["clips"]).to_json()).collect();
    let args = fixture["args_cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|case| {
            let plan = plan_of(fixture, &case["clips"]);
            let list = ffmpeg_args(&plan, "in.mp4", "out.mp4", &GraphArg::Slash("graph.txt".into()), &source(fixture), case["crf"].as_i64().unwrap(), case["preset"].as_str().unwrap(), case["keep_audio"].as_bool().unwrap());
            json!(list)
        })
        .collect();
    (plans, args)
}

#[test]
fn every_plan_matches_the_recorded_graph() {
    let mut fixture: Value = serde_json::from_str(&fs::read_to_string(fixture_path()).unwrap()).unwrap();
    let (plans, args) = results(&fixture);
    if std::env::var_os("H3_BLESS_OVERLAY").is_some() {
        for (case, got) in fixture["plan_cases"].as_array_mut().unwrap().iter_mut().zip(&plans) {
            case["return"] = got.clone();
        }
        for (case, got) in fixture["args_cases"].as_array_mut().unwrap().iter_mut().zip(&args) {
            case["return"] = got.clone();
        }
        fs::write(fixture_path(), format!("{}\n", serde_json::to_string_pretty(&fixture).unwrap())).unwrap();
        return;
    }
    for (case, got) in fixture["plan_cases"].as_array().unwrap().iter().zip(&plans) {
        assert_eq!(&case["return"], got, "{}", case["name"]);
    }
    for (case, got) in fixture["args_cases"].as_array().unwrap().iter().zip(&args) {
        assert_eq!(&case["return"], got, "{}", case["name"]);
    }
    assert!(fixture["plan_cases"].as_array().unwrap().len() >= 14);
}

#[test]
fn the_same_clips_always_give_the_same_graph() {
    let fixture: Value = serde_json::from_str(&fs::read_to_string(fixture_path()).unwrap()).unwrap();
    assert_eq!(results(&fixture), results(&fixture));
}
