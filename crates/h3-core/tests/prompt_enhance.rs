//! MiniMax H3 Enhance Prompts against what the pack's own functions returned for the same prompts and replies (`fixtures/llm/enhance.json`, made by
//! `tools/make_enhance_fixtures.py`).

use std::fs;
use std::path::PathBuf;

use h3_core::prompt::enhance::{merge_enhanced_prompt, merge_reference_enhanced_prompt, split_prompt};
use serde_json::{json, Value};

fn cases() -> Vec<Value> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/llm/enhance.json");
    serde_json::from_str::<Value>(&fs::read_to_string(path).unwrap()).unwrap()["cases"].as_array().cloned().unwrap()
}

#[test]
fn enhanced_prompts_merge_like_the_pack() {
    let all = cases();
    assert!(all.len() > 150, "{} cases", all.len());
    let mut failures = Vec::new();
    for (number, case) in all.iter().enumerate() {
        let args = case["args"].as_array().unwrap();
        let got: Result<Value, String> = match case["fn"].as_str().unwrap() {
            "merge" => merge_enhanced_prompt(args[0].as_str().unwrap(), args[1].as_str().unwrap()).map(Value::String).map_err(|e| e.0),
            "reference" => merge_reference_enhanced_prompt(args[0].as_str().unwrap(), &args[1], args[2].as_array().unwrap()).map(Value::String).map_err(|e| e.0),
            "split" => split_prompt(args[0].as_str().unwrap()).map(|(a, b, c)| json!([a, b, c])).map_err(|e| e.0),
            other => panic!("unknown function {other}"),
        };
        let ok = match (&got, case.get("error")) {
            (Ok(value), None) => value == &case["return"],
            (Err(message), Some(want)) => want.as_str() == Some(message.as_str()),
            _ => false,
        };
        if !ok {
            failures.push(format!("case {number} {}\n  got  {got:?}\n  want {}", case["fn"], case.get("error").unwrap_or(&case["return"])));
        }
    }
    assert!(failures.is_empty(), "{} of {} cases differ\n{}", failures.len(), all.len(), failures.iter().take(5).cloned().collect::<Vec<_>>().join("\n"));
}
