//! Scene card fields against `fixtures/timeline/scene_card.json`, made by the pack's own parser.

use std::fs;
use std::path::PathBuf;

use h3_core::scene_card::*;
use serde_json::{json, Value};

fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => (x.as_f64().unwrap() - y.as_f64().unwrap()).abs() < 1e-9,
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q)),
        (Value::Object(x), Value::Object(y)) => x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w))),
        _ => a == b,
    }
}

#[test]
fn scene_card_patches_match_the_pack() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/timeline/scene_card.json");
    let cases: Vec<Value> = serde_json::from_str::<Value>(&fs::read_to_string(path).unwrap()).unwrap()["cases"].as_array().cloned().unwrap();
    for case in &cases {
        if let Some(names) = case.get("names") {
            assert_eq!(json!(field_names()), *names);
            continue;
        }
        let patch = case["patch"].as_object().unwrap();
        let result = parse_scene_card_patch(patch);
        if let Some(want) = case.get("error") {
            assert_eq!(&json!(result.unwrap_err()), want, "{case}");
            continue;
        }
        let (values, rest) = result.unwrap_or_else(|e| panic!("{e}: {case}"));
        assert!(same(&Value::Object(values.clone()), &case["values"]), "values\n got  {values:?}\n want {}", case["values"]);
        assert!(same(&Value::Object(rest), &case["rest"]), "rest {case}");
        let mut segment = json!({"id": "s1", "start": 0, "end": 4, "label": "Scene 1", "lyric_text": "old"});
        apply_to_segment(&mut segment, &values, case["engine"].as_str().unwrap(), case["mode"].as_str().unwrap());
        assert!(same(&segment, &case["segment"]), "segment\n got  {segment}\n want {}", case["segment"]);
        let keys = |v: &Value| -> Vec<String> { v.as_object().unwrap().keys().cloned().collect() };
        assert_eq!(keys(&segment), keys(&case["segment"]), "key order {case}");
        assert_eq!(json!(card_only_names(&values)), case["card_only"], "{case}");
    }
}
