//! The chunked second pass on golden graphs: when it runs, when it leaves the graph alone, and why it says so.

mod common;

use std::path::Path;

use common::StubEnv;
use h3_graph::chunked_pass2::{apply_chunked_pass2, CHUNK_CLASS};
use h3_core::pyval::Obj;
use serde_json::{json, Value};

fn golden_prompt(name: &str) -> Obj {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/graphs").join(format!("{name}.json"));
    let record: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    record["result"]["prompt"].as_object().unwrap().clone()
}

fn asked(min_seconds: Value) -> Obj {
    json!({"pass2_chunking": "auto", "pass2_chunking_min_seconds": min_seconds}).as_object().unwrap().clone()
}

fn count(prompt: &Obj, class: &str) -> usize {
    prompt.values().filter(|node| node["class_type"] == json!(class)).count()
}

#[test]
fn a_long_clip_swaps_the_second_pass_for_the_chunk_node() {
    let mut prompt = golden_prompt("r2v_two_pass_scene1");
    let summary = apply_chunked_pass2(&StubEnv { has_chunk_node: true }, &mut prompt, &asked(json!(8)), Some(328));
    assert_eq!(summary["enabled"], json!(true), "{summary:?}");
    assert_eq!(count(&prompt, CHUNK_CLASS), 1);
    assert_eq!(count(&prompt, "VRGDG_MiniMaxH3LearnedLatentUpscale"), 0);
    assert_eq!(count(&prompt, "SamplerCustomAdvanced"), 1, "only the first pass sampler is left");
    for (id, node) in &prompt {
        for (name, value) in node["inputs"].as_object().unwrap() {
            if let Some([Value::String(target), _]) = value.as_array().map(Vec::as_slice) {
                assert!(prompt.contains_key(target), "{id}.{name} points at the removed node {target}");
            }
        }
    }
}

#[test]
fn the_threshold_decides_for_three_and_five_seconds() {
    // 124 frames is 5.17 s, 73 frames is 3.04 s, 60 frames is 2.5 s.
    let env = StubEnv { has_chunk_node: true };
    for (limit, frames, enabled) in [(3, 73, true), (3, 60, false), (5, 124, true), (5, 97, false), (8, 124, false)] {
        let mut prompt = golden_prompt("r2v_two_pass_scene1");
        let summary = apply_chunked_pass2(&env, &mut prompt, &asked(json!(limit)), Some(frames));
        assert_eq!(summary["enabled"], json!(enabled), "limit {limit} s, {frames} frames: {summary:?}");
        assert_eq!(count(&prompt, CHUNK_CLASS), usize::from(enabled));
    }
}

#[test]
fn a_comfyui_without_the_chunk_node_keeps_the_whole_clip_pass_and_says_so() {
    let mut prompt = golden_prompt("r2v_two_pass_scene1");
    let before = prompt.clone();
    let summary = apply_chunked_pass2(&StubEnv { has_chunk_node: false }, &mut prompt, &asked(json!(8)), Some(328));
    assert_eq!(summary["enabled"], json!(false));
    assert!(summary["reason"].as_str().unwrap().contains("Update the comfyui-vrgamedevgirl pack"), "{summary:?}");
    assert_eq!(prompt, before);
}

#[test]
fn a_single_pass_graph_stays_whole() {
    let mut prompt = golden_prompt("r2v_single_scene1");
    let before = prompt.clone();
    let summary = apply_chunked_pass2(&StubEnv { has_chunk_node: true }, &mut prompt, &asked(json!(8)), Some(328));
    assert_eq!(summary["enabled"], json!(false));
    assert_eq!(prompt, before);
}
