//! Scene assignment against `fixtures/timeline/assign.json`, recorded from the pack's `plan_scene_assignment`.

use std::path::Path;

use h3_project::assignment::{plan_scene_assignment, AssignParams};
use serde_json::{json, Value};

fn params_from(value: &Value) -> AssignParams {
    let mut p = AssignParams::default();
    let text = |k: &str| value.get(k).and_then(Value::as_str).map(str::to_string);
    let int = |k: &str| value.get(k).and_then(Value::as_i64);
    if let Some(v) = text("scope") { p.scope = v; }
    if let Some(v) = text("character_pattern") { p.character_pattern = v; }
    if let Some(v) = text("location_pattern") { p.location_pattern = v; }
    if let Some(v) = int("range_start") { p.range_start = v; }
    p.range_end = int("range_end");
    if let Some(v) = int("character_block_size") { p.character_block_size = v; }
    if let Some(v) = int("location_block_size") { p.location_block_size = v; }
    p.replace_existing = value.get("replace_existing").and_then(Value::as_bool).unwrap_or(false);
    p.scene_ids = value.get("scene_ids").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
    p
}

#[test]
fn plans_match_the_pack() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/timeline/assign.json");
    let cases: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert!(cases.len() > 100);
    for (n, case) in cases.iter().enumerate() {
        let session = case["session"].as_object().unwrap();
        let got = plan_scene_assignment(session, &params_from(&case["params"]));
        match (&case["return"]["plan"], &case["return"]["error"]) {
            (Value::Array(expected), _) => {
                let plan: Vec<Value> = got.unwrap_or_else(|e| panic!("case {n}: {e}")).iter().map(|i| i.to_json()).collect();
                assert_eq!(Value::Array(plan), Value::Array(expected.clone()), "case {n}: {}", case["params"]);
            }
            (_, Value::String(message)) => assert_eq!(got.unwrap_err().to_string(), *message, "case {n}"),
            _ => panic!("case {n} has neither plan nor error"),
        }
    }
}

#[test]
fn random_is_reproducible_with_a_seed_and_avoids_repeats() {
    let session = json!({
        "segments": (1..=12).map(|i| json!({"id": format!("scene_{i:03}")})).collect::<Vec<_>>(),
        "flux_reference_builder": {"subjects": [{"id": "a"}, {"id": "b"}], "locations": [{"id": "x"}, {"id": "y"}, {"id": "z"}]},
    });
    let session = session.as_object().unwrap();
    let mut p = AssignParams { character_pattern: "random".into(), location_pattern: "random".into(), seed: Some(7), ..AssignParams::default() };
    let first = plan_scene_assignment(session, &p).unwrap();
    assert_eq!(first, plan_scene_assignment(session, &p).unwrap());
    for pair in first.windows(2) {
        assert_ne!(pair[0].location_id, pair[1].location_id);
    }
    assert!(first.iter().all(|i| i.subject_ids.len() == 1));
    p.seed = Some(8);
    assert_ne!(first, plan_scene_assignment(session, &p).unwrap());
}
