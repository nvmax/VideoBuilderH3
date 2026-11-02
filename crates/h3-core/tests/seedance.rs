//! The Seedance prompt conversion against `fixtures/prompt/seedance.json`. Every case is what the pack's `prepare_seedance_blocks` and
//! `assemble_seedance_prompt` answered (made by `tools/make_seedance_fixtures.py`).

mod common;

use h3_core::seedance::{assemble_seedance_prompt, plan_schema, prepare_seedance_blocks, Block, HEADINGS, VOCAL_ROLE_GUIDANCE};
use serde_json::{json, Value};

fn blocks_of(value: &Value) -> Vec<Block> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|block| Block {
            id: block["id"].as_str().unwrap().to_string(),
            heading_hint: block["heading_hint"].as_str().unwrap().to_string(),
            text: block["text"].as_str().unwrap().to_string(),
        })
        .collect()
}

#[test]
fn a_saved_prompt_is_split_and_retagged_like_the_pack() {
    let fixture = common::load("prompt/seedance.json");
    let cases = fixture["prepare"].as_array().unwrap();
    assert!(cases.len() >= 45);
    let (mut converted, mut refused) = (0, 0);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let scene = case["scene"].as_object().unwrap();
        match prepare_seedance_blocks(scene) {
            Ok(blocks) => {
                assert_eq!(serde_json::to_value(&blocks).unwrap(), case["blocks"], "{name}");
                converted += 1;
            }
            Err(message) => {
                assert_eq!(Value::String(message), case["error"], "{name}");
                refused += 1;
            }
        }
    }
    assert!(converted >= 30 && refused >= 10, "{converted} converted, {refused} refused");
}

#[test]
fn a_layout_plan_is_checked_like_the_pack() {
    let fixture = common::load("prompt/seedance.json");
    let cases = fixture["assemble"].as_array().unwrap();
    let (mut laid_out, mut refused) = (0, 0);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let blocks = blocks_of(&case["blocks"]);
        match assemble_seedance_prompt(&blocks, &case["plan"]) {
            Ok(prompt) => {
                assert_eq!(Value::String(prompt), case["prompt"], "{name}");
                laid_out += 1;
            }
            Err(message) => {
                assert_eq!(Value::String(message), case["error"], "{name}");
                refused += 1;
            }
        }
    }
    assert!(laid_out >= 4 && refused >= 12, "{laid_out} laid out, {refused} refused");
}

#[test]
fn the_schema_and_the_texts_are_the_packs() {
    let fixture = common::load("prompt/seedance.json");
    assert_eq!(plan_schema(), fixture["plan_schema"]);
    assert_eq!(json!(HEADINGS), fixture["headings"]);
    assert_eq!(json!(VOCAL_ROLE_GUIDANCE), fixture["vocal_role_guidance"]);
}

#[test]
fn the_scene_audio_is_bound_to_the_prompt_like_the_pack() {
    let fixture = common::load("prompt/seedance.json");
    let cases = fixture["audio_prompt"].as_array().unwrap();
    let (mut bound, mut refused) = (0, 0);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let payload = case["payload"].as_object().unwrap();
        match h3_core::seedance::scene_audio_prompt("Prompt text.", payload) {
            Ok(prompt) => {
                assert_eq!(Value::String(prompt), case["prompt"], "{name}");
                bound += 1;
            }
            Err(message) => {
                assert_eq!(Value::String(message), case["error"], "{name}");
                refused += 1;
            }
        }
    }
    assert!(bound >= 6 && refused >= 4, "{bound} bound, {refused} refused");
}
