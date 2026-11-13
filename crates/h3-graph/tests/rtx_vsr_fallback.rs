//! The `lanczos` fallback for a ComfyUI without the `nvidia-vfx` library. The whole graphs are compared with the pack's in `golden.rs` (the
//! `_no_rtx_vsr` cases), so this file checks the swap itself and that the fixtures really contain the swap.

use std::path::Path;

use h3_core::pyval::Obj;
use h3_graph::workflows::fall_back_from_unavailable_rtx_vsr;
use serde_json::Value;

fn golden_prompt(name: &str) -> Obj {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/graphs").join(format!("{name}.json"));
    let record: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    record["result"]["prompt"].as_object().unwrap().clone()
}

/// The `upscale_method` of every node that has one.
fn methods(prompt: &Obj) -> Vec<(String, String)> {
    prompt.iter().filter_map(|(id, node)| Some((id.clone(), node["inputs"]["upscale_method"].as_str()?.to_string()))).collect()
}

#[test]
fn every_nvidia_resize_becomes_lanczos_and_nothing_else_changes() {
    for name in ["r2v_two_pass_scene1", "r2v_two_pass_chunked_long", "i2v_two_pass_normal"] {
        let mut prompt = golden_prompt(name);
        let before = serde_json::to_string(&prompt).unwrap();
        assert!(methods(&prompt).iter().any(|(_, m)| m == "nvidia_rtx_vsr"), "{name} has a resize to swap");
        let changed = fall_back_from_unavailable_rtx_vsr(&mut prompt);
        assert!(!changed.is_empty(), "{name}");
        assert!(methods(&prompt).iter().all(|(_, m)| m != "nvidia_rtx_vsr"), "{name}");
        assert_eq!(serde_json::to_string(&prompt).unwrap(), before.replace("\"nvidia_rtx_vsr\"", "\"lanczos\""), "{name}");
    }
}

#[test]
fn a_graph_without_the_resize_is_left_alone() {
    let mut prompt = golden_prompt("r2v_two_pass_scene1");
    fall_back_from_unavailable_rtx_vsr(&mut prompt);
    let before = prompt.clone();
    assert!(fall_back_from_unavailable_rtx_vsr(&mut prompt).is_empty(), "nothing left to swap");
    assert_eq!(prompt, before);
}

#[test]
fn the_fixtures_made_without_the_library_resize_with_lanczos() {
    for name in ["r2v_two_pass_no_rtx_vsr", "r2v_two_pass_chunked_no_rtx_vsr", "i2v_two_pass_no_rtx_vsr"] {
        let prompt = golden_prompt(name);
        let methods = methods(&prompt);
        assert!(methods.iter().any(|(_, m)| m == "lanczos"), "{name}");
        assert!(methods.iter().all(|(_, m)| m != "nvidia_rtx_vsr"), "{name}");
    }
}
