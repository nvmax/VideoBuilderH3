//! Cut plan, video style, temporal effect and FX wording against `fixtures/timeline/style.json`.

use std::fs;
use std::path::PathBuf;

use h3_core::style::*;
use serde_json::{json, Value};

fn obj(v: &Value) -> h3_core::pyval::Obj {
    v.as_object().cloned().unwrap_or_default()
}

fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => (x.as_f64().unwrap() - y.as_f64().unwrap()).abs() < 1e-9,
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q)),
        (Value::Object(x), Value::Object(y)) => x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w))),
        _ => a == b,
    }
}

#[test]
fn style_wording_matches_the_pack() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/timeline/style.json");
    let cases: Vec<Value> = serde_json::from_str::<Value>(&fs::read_to_string(path).unwrap()).unwrap()["cases"].as_array().cloned().unwrap();
    for c in &cases {
        match c["op"].as_str().unwrap() {
            "cut_plan" => {
                let got = Value::Object(storyboard_cut_plan_for_duration(c["duration"].as_f64().unwrap(), c["frequency"].as_i64().unwrap()));
                assert!(same(&got, &c["return"]), "cut plan\n got  {got}\n want {}", c["return"]);
            }
            "style" => {
                let (card, defaults) = (obj(&c["card"]), obj(&c["defaults"]));
                assert_eq!(json!(video_style_label(&card, &defaults)), c["label"], "{c}");
                assert_eq!(json!(video_style_guidance(&card, &defaults)), c["guidance"], "{c}");
                assert_eq!(json!(video_style_verbiage(&card, &defaults)), c["verbiage"], "{c}");
            }
            "temporal" => assert_eq!(json!(temporal_world_effect_verbiage(&obj(&c["card"]), &obj(&c["defaults"]))), c["return"], "{c}"),
            "temporal_append" => assert_eq!(json!(append_temporal_world_effect(c["prompt"].as_str().unwrap(), c["verbiage"].as_str().unwrap())), c["return"], "{c}"),
            "fx" => {
                let defaults = obj(&c["defaults"]);
                let accents: Vec<String> = (0..4).map(|i| fx_accent(&defaults, i)).collect();
                assert_eq!(json!(accents), c["accents"], "{c}");
                assert_eq!(json!(fx_reserve(&defaults, 3)), c["reserve"], "{c}");
                let core = "[Shot 1] At 00:00.000, one.\n\n[Shot 2] At 00:01.500, two.\n\n[Shot 3] At 00:03.000, three.";
                assert_eq!(json!(apply_fx(core, &defaults)), c["apply"], "{c}");
            }
            "articulation" => assert_eq!(json!(h3_core::prompt::performance::strip_singing_articulation(c["text"].as_str().unwrap())), c["return"], "{c}"),
            "emotion" => assert_eq!(json!(h3_core::prompt::emotion::emotion_expression_instruction(&obj(&c["payload"]))), c["return"], "{c}"),
            "emotion_input" => assert_eq!(Value::Object(h3_core::prompt::emotion::emotion_expression_input(&obj(&c["segment"]), &obj(&c["session"]))), c["return"], "{c}"),
            other => panic!("unknown op {other}"),
        }
    }
}
