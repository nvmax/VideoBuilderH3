//! Scene Defaults, the Theatrical presets and the fresh prompt context against `fixtures/prompt/scene_defaults.json`, recorded from the pack.

mod common;

use common::load;
use h3_core::fresh_context::{fresh_prompt_context_text, fresh_prompt_scene_context};
use h3_core::scene_defaults::normalize_scene_defaults;
use h3_core::theatrical::{direction_lines, project_defaults};
use serde_json::{json, Value};

fn obj(v: &Value) -> &serde_json::Map<String, Value> {
    v.as_object().unwrap()
}

#[test]
fn scene_defaults_are_cleaned_like_the_pack() {
    let data = load("prompt/scene_defaults.json");
    for case in data["normalize"].as_array().unwrap() {
        let got = normalize_scene_defaults(&case["value"], case["strict"].as_bool().unwrap());
        match (case.get("error"), got) {
            (Some(error), Err(got)) => assert_eq!(json!(got), *error, "{case}"),
            (None, Ok(got)) => assert_eq!(Value::Object(got.clone()), case["return"], "{case}\n key order {:?}", got.keys().collect::<Vec<_>>()),
            (want, got) => panic!("{case}: wanted {want:?}, got {got:?}"),
        }
    }
}

#[test]
fn theatrical_directions_match_the_pack() {
    let data = load("prompt/scene_defaults.json");
    for case in data["defaults"].as_array().unwrap() {
        assert_eq!(Value::Object(project_defaults(obj(&case["session"]))), case["return"], "{case}");
    }
    for case in data["lines"].as_array().unwrap() {
        let got = direction_lines(obj(&case["card"]), case["defaults"].as_object());
        assert_eq!(json!(got), case["return"], "{case}");
    }
}

#[test]
fn saved_prompts_stay_out_of_a_fresh_prompt_context() {
    let data = load("prompt/scene_defaults.json");
    for case in data["fresh_scene"].as_array().unwrap() {
        assert_eq!(fresh_prompt_scene_context(&case["value"]), case["return"], "{case}");
    }
    for case in data["fresh_text"].as_array().unwrap() {
        assert_eq!(fresh_prompt_context_text(case["text"].as_str().unwrap()), case["return"].as_str().unwrap(), "{case}");
    }
}
