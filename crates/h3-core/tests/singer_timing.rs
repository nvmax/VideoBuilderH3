//! Timing a multi-shot singing scene before its prompt is written, against `fixtures/prompt/singer_timing.json`. Every case is
//! what the pack's `shot_aligned_singer_cues` and `time_scene_singer_cues` answered (made by `tools/make_singer_timing_fixtures.py`).

mod common;

use h3_core::prompt::singer_timing::{plan_singer_timing, shot_aligned_singer_cues};
use h3_core::pyval::Obj;
use serde_json::Value;

fn object(value: &Value) -> Obj {
    value.as_object().unwrap_or_else(|| panic!("not an object: {value}")).clone()
}

#[test]
fn words_are_placed_into_the_shots_like_the_pack() {
    let fixture = common::load("prompt/singer_timing.json");
    let cases = fixture["align"].as_array().unwrap();
    assert!(cases.len() >= 13);
    for case in cases {
        let cues = shot_aligned_singer_cues(&object(&case["payload"]), &object(&case["cut_plan"]), &object(&case["performer"]));
        assert_eq!(Value::Array(cues), case["cues"], "{}", case["name"]);
    }
}

#[test]
fn what_a_scene_needs_timed_matches_the_pack() {
    let fixture = common::load("prompt/singer_timing.json");
    let clip_audio = fixture["clip_audio"].as_str().unwrap();
    let cases = fixture["timing"].as_array().unwrap();
    let (mut skipped, mut failed, mut timed) = (0, 0, 0);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let session = object(&case["session"]);
        let segment = object(&session["segments"][0]);
        let index = case["index"].as_u64().unwrap() as usize;
        let planned = plan_singer_timing(&session, "Z:\\h3fixture\\proj", &segment, index, &object(&case["cut_plan"]));
        match planned {
            Ok(None) => {
                assert_eq!(case["skip"], Value::Bool(true), "{name}: the pack timed this scene");
                skipped += 1;
            }
            Err(message) => {
                assert_eq!(Value::String(message), case["error"], "{name}");
                failed += 1;
            }
            Ok(Some(plan)) => {
                // The words that come back can still leave nothing to place, which the pack also reports as an error.
                let answer = plan.cue_fields(&object(&case["reply"]));
                if let Some(expected) = case.get("error") {
                    assert_eq!(Value::String(answer.expect_err(name)), *expected, "{name}");
                    failed += 1;
                    continue;
                }
                assert_eq!(Value::Object(plan.clip.clone()), case["clip"], "{name}: audio clip");
                assert_eq!(Value::Object(plan.graph_request(clip_audio)), case["graph"], "{name}: timestamp request");
                assert_eq!(Value::Object(answer.expect(name)), case["fields"], "{name}: saved fields");
                timed += 1;
            }
        }
    }
    assert_eq!(skipped + failed + timed, cases.len());
    assert!(skipped >= 8 && failed >= 6 && timed >= 20, "{skipped} skipped, {failed} failed, {timed} timed");
}
