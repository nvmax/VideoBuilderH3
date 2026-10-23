//! Timing and token math against `fixtures/core`.

mod common;

use h3_core::timing::{align_h3_frame_count, calculate_timing, frames_covering_duration, TimingInput};
use h3_core::tokens::{frames_to_tokens, plan_masked_context, tokens_to_frames};
use serde_json::{json, Value};

fn num(v: &Value) -> Option<f64> {
    v.as_f64()
}

#[test]
fn token_and_frame_tables() {
    for c in common::cases("core/frames_to_tokens.json") {
        assert_eq!(frames_to_tokens(c["input"]["frames"].as_i64().unwrap()), c["output"].as_i64().unwrap(), "{c}");
    }
    for c in common::cases("core/tokens_to_frames.json") {
        assert_eq!(tokens_to_frames(c["input"]["tokens"].as_i64().unwrap()), c["output"].as_i64().unwrap(), "{c}");
    }
}

#[test]
fn frame_alignment_and_cover() {
    for c in common::cases("core/align_frame_count.json") {
        let got = align_h3_frame_count(num(&c["input"]["frame_count"]).unwrap());
        match (got, c.get("error")) {
            (Ok(v), None) => assert_eq!(v, c["output"].as_i64().unwrap(), "{c}"),
            (Err(e), Some(err)) => assert_eq!(json!(e.message), err["message"], "{c}"),
            (got, _) => panic!("mismatch {got:?} vs {c}"),
        }
    }
    for c in common::cases("core/frames_covering_duration.json") {
        let i = &c["input"];
        let got = frames_covering_duration(num(&i["duration"]).unwrap(), num(&i["fps"]).unwrap()).unwrap();
        assert_eq!(got, c["output"].as_i64().unwrap(), "{c}");
    }
}

#[test]
fn masked_context_plans() {
    let cases = common::cases("core/masked_context.json");
    assert!(cases.len() > 100);
    for c in cases {
        let i = &c["input"];
        let got = plan_masked_context(
            i["total_tokens"].as_i64().unwrap(),
            i["tail_padding_frames"].as_i64(),
            num(&i["context_frames"]).unwrap(),
        );
        match (got, c.get("error")) {
            (Ok(plan), None) => assert_eq!(serde_json::to_value(plan).unwrap(), c["output"], "{c}"),
            (Err(e), Some(err)) => assert_eq!(json!(e.message), err["message"], "{c}"),
            (got, _) => panic!("mismatch {got:?} vs {c}"),
        }
    }
}

#[test]
fn timing_plans() {
    let cases = common::cases("core/timing.json");
    assert!(cases.len() > 200);
    for c in cases {
        let i = &c["input"];
        let get = |k: &str| i.get(k).filter(|v| !v.is_null());
        // A non-numeric value is rejected before the plan is built (the pack raises on `Decimal("abc")`).
        if get("timeline_start_seconds").is_some_and(|v| num(v).is_none()) {
            assert_eq!(c["error"]["message"], "timeline_start_seconds must be a finite number.");
            continue;
        }
        let input = TimingInput {
            timeline_start_seconds: num(&i["timeline_start_seconds"]).unwrap(),
            timeline_end_seconds: num(&i["timeline_end_seconds"]).unwrap(),
            warmup_frames: num(&i["warmup_frames"]).unwrap_or(0.0),
            cooldown_frames: num(&i["cooldown_frames"]).unwrap_or(0.0),
            source_start_seconds: get("source_start_seconds").and_then(num),
            source_duration_seconds: get("source_duration_seconds").and_then(num),
            max_frame_count: None,
            pad_warmup: i.get("pad_warmup").and_then(Value::as_bool).unwrap_or(false),
        };
        match (calculate_timing(&input), c.get("error")) {
            (Ok(plan), None) => assert_eq!(serde_json::to_value(plan).unwrap(), c["output"], "{c}"),
            (Err(e), Some(err)) => assert_eq!(json!(e.message), err["message"], "{c}"),
            (got, _) => panic!("mismatch {got:?} vs {c}"),
        }
    }
}
