//! The masked continuation rules against `fixtures/timeline/masked_context.json`, recorded from the pack.

mod common;

use common::load;
use h3_core::prompt::masked::masked_continuation_context;
use serde_json::Value;

#[test]
fn masked_continuation_context_matches_the_pack() {
    let cases = load("timeline/masked_context.json");
    let cases = cases.as_array().unwrap();
    assert!(cases.len() > 400);
    for (n, case) in cases.iter().enumerate() {
        let segment = case["segment"].as_object().unwrap();
        let got = masked_continuation_context(segment, case["duration"].as_f64().unwrap(), case["shots"].as_u64().unwrap() as usize, case["vocals"].as_bool().unwrap());
        assert_eq!(Value::Object(got), case["return"], "case {n}: {} {} {} {}", case["segment"], case["duration"], case["shots"], case["vocals"]);
    }
}
