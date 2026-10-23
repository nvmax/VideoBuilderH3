//! The omit-lyrics policy against `fixtures/prompt/lyric_free.json`, recorded from the pack.

mod common;

use common::load;
use h3_core::prompt::lyric_free::{apply_shots, clean_description, directions, facial_text, merge_contract, prompt_context, scene_cut_plan, scene_enabled, scene_performers, Labels};
use serde_json::Value;

fn obj(v: &Value) -> &serde_json::Map<String, Value> {
    v.as_object().unwrap()
}

fn labels(v: &Value) -> Labels {
    Labels(obj(v).iter().map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string())).collect())
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().unwrap().iter().map(|s| s.as_str().unwrap().to_string()).collect()
}

#[test]
fn contracts_and_descriptions_match_the_pack() {
    let data = load("prompt/lyric_free.json");
    for case in data["merge"].as_array().unwrap() {
        let (description, contract) = merge_contract(case["description"].as_str().unwrap(), case["contract"].as_str().unwrap());
        assert_eq!(serde_json::json!([description, contract]), case["return"], "merge {} / {}", case["description"], case["contract"]);
    }
    for case in data["clean"].as_array().unwrap() {
        let got = clean_description(case["text"].as_str().unwrap(), &strings(&case["lyrics"]), case["preserve"].as_bool().unwrap());
        assert_eq!(got, case["return"].as_str().unwrap(), "clean {} {} {}", case["text"], case["lyrics"], case["preserve"]);
    }
    for case in data["facial"].as_array().unwrap() {
        assert_eq!(facial_text(obj(&case["segment"]), obj(&case["session"])), case["return"].as_str().unwrap(), "facial {}", case["segment"]);
    }
}

#[test]
fn directions_apply_and_context_match_the_pack() {
    let data = load("prompt/lyric_free.json");
    for case in data["directions"].as_array().unwrap() {
        let got = directions(obj(&case["segment"]), obj(&case["plan"]), case["performer"].as_str().unwrap(), case["index"].as_i64().unwrap(), &labels(&case["labels"]));
        assert_eq!(serde_json::json!(got), case["return"], "directions {} {} {}", case["segment"], case["index"], case["labels"]);
    }
    for case in data["apply"].as_array().unwrap() {
        let got = apply_shots(&strings(&case["descriptions"]), obj(&case["segment"]), obj(&case["plan"]), case["performer"].as_str().unwrap(), &labels(&case["labels"]), obj(&case["session"]));
        assert_eq!(serde_json::json!(got), case["return"], "apply {} {}", case["segment"], case["descriptions"]);
    }
    for case in data["context"].as_array().unwrap() {
        let got = prompt_context(case["text"].as_str().unwrap(), obj(&case["segment"]), obj(&case["plan"]), case["performer"].as_str().unwrap(), &labels(&case["labels"]), obj(&case["session"]));
        assert_eq!(got, case["return"].as_str().unwrap(), "context {}", case["segment"]);
    }
}

#[test]
fn scene_plans_performers_and_the_switch_match_the_pack() {
    let data = load("prompt/lyric_free.json");
    for (plan, enabled) in data["cut_plan"].as_array().unwrap().iter().zip(data["enabled"].as_array().unwrap()) {
        let session = obj(&data["sessions"][plan["session"].as_str().unwrap()]);
        assert_eq!(Value::Object(scene_cut_plan(obj(&plan["segment"]), session)), plan["return"], "plan {} {}", plan["session"], plan["segment"]);
        assert_eq!(scene_enabled(session, obj(&enabled["segment"])), enabled["return"].as_bool().unwrap(), "enabled {} {}", enabled["session"], enabled["segment"]);
    }
    for case in data["performers"].as_array().unwrap() {
        let session = obj(&case["session"]);
        let (performer, found) = scene_performers(session, obj(&session["segments"][0]), "reference_to_video");
        assert_eq!(performer, case["return"]["performer"].as_str().unwrap(), "performer {}", session["segments"][0]);
        let expected: Vec<(String, String)> = obj(&case["return"]["labels"]).iter().map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string())).collect();
        assert_eq!(found.0, expected);
    }
}
