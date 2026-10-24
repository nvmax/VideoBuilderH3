//! The short-film scene helpers against what the pack's own functions returned for the same inputs (`fixtures/story/dialogue_helpers.json`, made by
//! `tools/make_dialogue_scene_fixtures.py`).

use std::fs;
use std::path::PathBuf;

use h3_core::story::dialogue::{apply_authoritative_script_plan, camera_motion_family, normalize_generated_scenes, rebalance_camera_motion, structured_image_prompt};
use serde_json::{json, Value};

fn cases() -> Vec<Value> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/story/dialogue_helpers.json");
    serde_json::from_str::<Value>(&fs::read_to_string(path).unwrap()).unwrap()["cases"].as_array().cloned().unwrap()
}

fn list(value: &Value) -> Vec<Value> {
    value.as_array().cloned().unwrap_or_default()
}

fn run(name: &str, args: &Value) -> Result<Value, String> {
    let a = args.as_array().unwrap();
    Ok(match name {
        "_id_lora_structured_image_prompt" => json!(structured_image_prompt(a[0].as_object().unwrap(), Some(&a[1]).filter(|v| !v.is_null()), Some(&a[2]).filter(|v| !v.is_null()))),
        "_minimax_camera_motion_family" => json!(camera_motion_family(a[0].as_str().unwrap_or(""))),
        "_rebalance_generated_minimax_camera_motion" => match &a[0] {
            Value::Array(scenes) => {
                let mut scenes = scenes.clone();
                rebalance_camera_motion(&mut scenes, a[1].as_str().unwrap_or(""), a[2].as_i64().unwrap_or(4));
                Value::Array(scenes)
            }
            other => other.clone(),
        },
        "_normalize_generated_minimax_dialogue_scenes" => {
            let scenes = normalize_generated_scenes(Some(&a[0]), &list(&a[1]), &list(&a[2]), Some(&a[3]).filter(|v| !v.is_null()), a[4].as_str().unwrap_or(""), a[5].as_i64().unwrap_or(4))?;
            Value::Array(scenes)
        }
        "_apply_authoritative_script_plan" => Value::Array(apply_authoritative_script_plan(Some(&a[0]), &a[1])),
        other => panic!("unknown function {other}"),
    })
}

#[test]
fn the_dialogue_scene_helpers_match_the_pack() {
    let all = cases();
    assert!(all.len() > 150, "{} cases", all.len());
    let mut failures = Vec::new();
    for (number, case) in all.iter().enumerate() {
        let name = case["fn"].as_str().unwrap();
        let got = run(name, &case["args"]);
        let ok = match (&got, case.get("error")) {
            (Ok(value), None) => value == &case["return"],
            (Err(message), Some(want)) => want.as_str() == Some(message.as_str()),
            _ => false,
        };
        if !ok {
            failures.push(format!("case {number} {name}\n  args {}\n  got  {got:?}\n  want {}", case["args"], case.get("error").unwrap_or(&case["return"])));
        }
    }
    assert!(failures.is_empty(), "{} of {} cases differ\n{}", failures.len(), all.len(), failures.iter().take(3).cloned().collect::<Vec<_>>().join("\n"));
}
