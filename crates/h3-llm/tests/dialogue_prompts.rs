//! The MiniMax short-film planner texts against the pack's own (`fixtures/story/dialogue_prompts.json`, made by `tools/make_dialogue_scene_fixtures.py`).

use std::fs;
use std::path::PathBuf;

use h3_llm::dialogue_prompts::*;
use serde_json::{json, Value};

fn cases() -> Vec<Value> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/story/dialogue_prompts.json");
    serde_json::from_str::<Value>(&fs::read_to_string(path).unwrap()).unwrap()["cases"].as_array().cloned().unwrap()
}

fn s<'a>(args: &'a Value, key: &str) -> &'a str {
    args.get(key).and_then(Value::as_str).unwrap_or("")
}

fn n(args: &Value, key: &str) -> i64 {
    args.get(key).and_then(Value::as_i64).unwrap_or(0)
}

fn run(name: &str, a: &Value) -> Value {
    match name {
        "_storyboard_dialogue_planner_instruction" => json!(dialogue_planner_instruction(&PlannerInput {
            has_authoritative_script: a["has_authoritative_script"].as_bool().unwrap_or(false),
            camera_flow: s(a, "camera_flow"),
            camera_motion_speed: n(a, "camera_motion_speed"),
            character_motion_speed: n(a, "character_motion_speed"),
            scene_count: n(a, "scene_count"),
            story_source: s(a, "story_source"),
            script_mapper_plan_json: s(a, "script_mapper_plan_json"),
            story_layer_json: s(a, "story_layer_json"),
            project_motion_settings_json: s(a, "project_motion_settings_json"),
            subjects_json: s(a, "subjects_json"),
            locations_json: s(a, "locations_json"),
            compact_existing_json: s(a, "compact_existing_json"),
        })),
        "_storyboard_dialogue_plan_schema" => dialogue_plan_schema(),
        "_storyboard_dialogue_json_repair_instruction" => json!(dialogue_json_repair_instruction(s(a, "malformed_text"))),
        other => panic!("unknown builder {other}"),
    }
}

#[test]
fn the_dialogue_planner_texts_match_the_pack() {
    let all = cases();
    assert!(all.len() >= 15, "{} cases", all.len());
    let mut failures = Vec::new();
    for (number, case) in all.iter().enumerate() {
        let name = case["fn"].as_str().unwrap();
        let got = run(name, &case["args"]);
        if got != case["return"] {
            failures.push(format!("case {number} {name}\n  got  {got}\n  want {}", case["return"]));
        }
    }
    assert!(failures.is_empty(), "{} of {} cases differ\n{}", failures.len(), all.len(), failures.iter().take(3).cloned().collect::<Vec<_>>().join("\n"));
}
