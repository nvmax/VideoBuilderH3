//! The Storyboard's pure functions against what the pack's own functions returned for the same inputs (`fixtures/story/helpers.json`, made by
//! `tools/make_storyboard_fixtures.py`).

use std::fs;
use std::path::PathBuf;

use h3_core::story::helpers::*;
use h3_core::story::scene::*;
use h3_core::story::scene_text::*;
use serde_json::{json, Value};

fn cases() -> Vec<Value> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/story/helpers.json");
    serde_json::from_str::<Value>(&fs::read_to_string(path).unwrap()).unwrap()["cases"].as_array().cloned().unwrap()
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or("")
}

fn run(case: &Value) -> Result<Value, String> {
    let args = case["args"].as_array().unwrap();
    let arg = |i: usize| &args[i];
    Ok(match case["fn"].as_str().unwrap() {
        "scene" => normalize_storyboard_scene(arg(0), arg(1).as_i64().unwrap()),
        "script_import" => normalize_script_import(Some(arg(0))),
        "layer" => normalize_story_layer(Some(arg(0))),
        "guidance" => json!(lyric_story_strength_guidance(arg(0))),
        "catalog" => normalize_reference_catalog(Some(arg(0))),
        "item" => normalize_reference_item(Some(arg(0)), text(arg(1)), text(arg(2))),
        "tags" => json!(normalize_tags(Some(arg(0)))),
        "stem" => json!(safe_file_stem(text(arg(0)), text(arg(1)))),
        "facial" => json!(enforce_video_facial_requirements(text(arg(0)), arg(1))),
        "motion" => json!(enforce_high_motion_language(text(arg(0)), arg(1))),
        "camera" => json!(camera_motion_for_speed(text(arg(0)), storyboard_speed(Some(arg(1)), 4.0))),
        "start_shot" => json!(ensure_starting_shot(text(arg(0)), arg(1))),
        "ref_opening" => json!(ensure_reference_opening(text(arg(0)), arg(1))),
        "timed" => json!(timed_lyric_contract(arg(0))),
        "pronouns" => json!(fix_single_subject_prompt_pronouns(text(arg(0)), arg(1))?),
        "dialogue_catalog" => {
            let (subjects, locations) = dialogue_reference_catalog(arg(0).as_object().unwrap());
            json!([subjects, locations])
        }
        other => panic!("unknown function {other}"),
    })
}

#[test]
fn the_storyboard_functions_match_the_pack() {
    let all = cases();
    assert!(all.len() > 250, "{} cases", all.len());
    let mut failures = Vec::new();
    for (number, case) in all.iter().enumerate() {
        let got = run(case);
        let ok = match (&got, case.get("error")) {
            (Ok(value), None) => value == &case["return"],
            (Err(message), Some(want)) => want.as_str() == Some(message.as_str()),
            _ => false,
        };
        if !ok {
            if let Ok(dir) = std::env::var("H3_DUMP") {
                let entry = json!({"case": number, "fn": case["fn"], "args": case["args"], "got": got.clone().unwrap_or_else(|e| json!({"error": e})), "want": case.get("error").unwrap_or(&case["return"])});
                fs::write(PathBuf::from(dir).join(format!("case_{number}.json")), entry.to_string()).unwrap();
            }
            failures.push(format!("case {number} {} {}\n  got  {got:?}\n  want {}", case["fn"], case["args"], case.get("error").unwrap_or(&case["return"])));
        }
    }
    assert!(failures.is_empty(), "{} of {} cases differ\n{}", failures.len(), all.len(), failures.iter().take(6).cloned().collect::<Vec<_>>().join("\n"));
}
