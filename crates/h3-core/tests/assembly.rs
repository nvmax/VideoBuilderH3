//! Prompt assembly, validation and the agent brief against `fixtures/prompt/assembly.json`, recorded from the pack.

mod common;

use common::load;
use h3_core::prompt::assembly::{assemble_minimax_h3_prompt, build_minimax_prompt_context, validate_minimax_h3_prompt};
use serde_json::Value;

fn scene_and_session(data: &Value, case: &Value) -> (serde_json::Map<String, Value>, serde_json::Map<String, Value>) {
    let session = data["sessions"][case["variant"].as_str().unwrap()].as_object().unwrap().clone();
    let index = case["index"].as_u64().unwrap() as usize;
    let scene = session["segments"][index].as_object().unwrap().clone();
    (scene, session)
}

#[test]
fn assembled_prompts_match_the_pack() {
    let data = load("prompt/assembly.json");
    let cases = data["assemble"].as_array().unwrap();
    assert!(cases.len() > 1000);
    for (n, case) in cases.iter().enumerate() {
        let (scene, session) = scene_and_session(&data, case);
        let shots: Vec<String> = case["descriptions"].as_array().unwrap().iter().map(|s| s.as_str().unwrap().to_string()).collect();
        let got = assemble_minimax_h3_prompt(&scene, &session, &shots, case["mode"].as_str()).expect("the pack answered, so this must");
        let want = &case["return"];
        let label = format!("case {n}: {} scene {} mode {} shots {}", case["variant"], case["index"], case["mode"], case["shots"]);
        assert_eq!(got["prompt"], want["prompt"], "{label}");
        assert_eq!(Value::Object(got), *want, "{label}");
    }
}

#[test]
fn validation_matches_the_pack() {
    let data = load("prompt/assembly.json");
    let prompts = data["prompts"].as_array().unwrap();
    let cases = data["validate"].as_array().unwrap();
    assert!(cases.len() > 3000);
    for (n, case) in cases.iter().enumerate() {
        let scene = case["index"].as_u64().map(|_| scene_and_session(&data, case).0);
        let prompt = prompts[case["prompt"].as_u64().unwrap() as usize].as_str().unwrap();
        let got = validate_minimax_h3_prompt(prompt, scene.as_ref(), case["mode"].as_str(), case["strict"].as_bool().unwrap());
        assert_eq!(got, case["return"], "case {n}: {} mode {} strict {} prompt {}", case["variant"], case["mode"], case["strict"], case["prompt"]);
    }
}

#[test]
fn agent_briefs_match_the_pack() {
    let data = load("prompt/assembly.json");
    let cases = data["context"].as_array().unwrap();
    assert!(cases.len() >= 30);
    for (n, case) in cases.iter().enumerate() {
        let (scene, session) = scene_and_session(&data, case);
        let got = build_minimax_prompt_context(&scene, &session, None);
        assert_eq!(Value::Object(got), case["return"], "case {n}: {} scene {}", case["variant"], case["index"]);
    }
}
