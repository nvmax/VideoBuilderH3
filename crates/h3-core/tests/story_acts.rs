//! Story Mode act rules against the cases the pack checks its own twins with (`fixtures/cases/story_acts_cases.json`).

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use h3_core::story::acts::*;
use h3_core::story::mode::{apply_scene_cast, fill_scene_cast};
use serde_json::Value;

fn cases() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/cases/story_acts_cases.json");
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn list(value: &Value) -> Vec<Value> {
    value.as_array().unwrap().clone()
}

fn strings(value: &Value) -> Vec<String> {
    value.as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect()
}

fn field(acts: &[Value], key: &str) -> Value {
    Value::Array(acts.iter().map(|a| a[key].clone()).collect())
}

#[test]
fn edited_acts_are_validated_like_the_pack() {
    let data = cases();
    for item in data["validate"].as_array().unwrap() {
        let acts = if item["acts"] == "base" { list(&data["acts"]) } else { list(&item["acts"]) };
        let problems = validate_story_acts(&acts, &list(&data["chapters"]), &strings(&data["scene_ids"]));
        assert_eq!(problems, strings(&item["problems"]), "{}", item["name"]);
    }
}

#[test]
fn act_edits_match_the_pack() {
    let data = cases();
    let (acts, scene_ids) = (list(&data["acts"]), strings(&data["scene_ids"]));
    for item in data["move"].as_array().unwrap() {
        let moved = move_act_border(&acts, item["act_index"].as_i64().unwrap(), item["scene_id"].as_str().unwrap(), &scene_ids);
        assert_eq!(field(&moved, "scene_ids"), item["scene_ids_after"], "{}", item["name"]);
    }
    for item in data["merge"].as_array().unwrap() {
        let merged = merge_acts(&acts, item["act_index"].as_i64().unwrap());
        assert_eq!(field(&merged, "id"), item["ids_after"], "{}", item["name"]);
        assert_eq!(field(&merged, "scene_ids"), item["scene_ids_after"], "{}", item["name"]);
        assert_eq!(field(&merged, "cast_ids"), item["cast_after"], "{}", item["name"]);
        assert_eq!(field(&merged, "summary"), item["summary_after"], "{}", item["name"]);
    }
    for item in data["split"].as_array().unwrap() {
        let split = split_act(&acts, item["act_index"].as_i64().unwrap(), item["scene_id"].as_str().unwrap());
        assert_eq!(field(&split, "id"), item["ids_after"], "{}", item["name"]);
        assert_eq!(field(&split, "scene_ids"), item["scene_ids_after"], "{}", item["name"]);
        assert_eq!(field(&split, "title"), item["titles_after"], "{}", item["name"]);
    }
}

#[test]
fn cast_is_filled_and_applied_like_the_pack() {
    let data = cases();
    let item = &data["fill_cast"];
    let (map, changed) = fill_scene_cast(item["subject_scene_map"].as_object().unwrap(), Some(&item["acts"]), &strings(&item["subject_ids"]));
    assert_eq!(Value::Object(map), item["map_after"]);
    assert_eq!(changed, strings(&item["changed"]));
    let item = &data["apply_cast"];
    let (map, changed) = apply_scene_cast(item["subject_scene_map"].as_object().unwrap(), Some(&item["plan"]), Some(&item["acts"]), &strings(&item["subject_ids"]));
    assert_eq!(Value::Object(map), item["map_after"]);
    assert_eq!(changed, strings(&item["changed"]));
}

#[test]
fn scenes_are_shared_out_and_ranges_repaired() {
    assert_eq!(allocate_chapter_scenes(&[3, 1, 2], 20), vec![9, 4, 7]);
    assert_eq!(allocate_chapter_scenes(&[1; 5], 3), vec![1, 1, 1, 0, 0]);
    assert_eq!(chapter_ranges(&[9, 0, 3]), vec![Some((1, 9)), None, Some((10, 12))]);
    assert_eq!(repair_act_ranges(&[Some(5), Some(1), Some(30), None], 1, 9), vec![(1, 4), (5, 9)]);
    assert_eq!(repair_act_ranges(&[], 3, 7), vec![(3, 7)]);
    assert_eq!(keep_notes_inside_acts(&[(1, 4), (5, 9)], &[(3, 6)]), vec![(1, 2), (3, 9)]);
    assert_eq!(keep_notes_inside_acts(&[(1, 4), (5, 9)], &[(4, 8)]), vec![(1, 3), (4, 9)]);
    assert_eq!(fallback_act_ranges(4, 10, 19), vec![(10, 12), (13, 15), (16, 17), (18, 19)]);
}

#[test]
fn spots_keep_their_minimum_run() {
    let spots: Vec<String> = ["Kitchen", "Kitchen", "Hall", "Kitchen", "Porch", "Porch", "Porch", ""].iter().map(|s| s.to_string()).collect();
    let expected: Vec<String> = ["Kitchen"; 4].iter().chain(["Porch"; 4].iter()).map(|s| s.to_string()).collect();
    assert_eq!(enforce_spot_runs(&spots, 3, &HashSet::new()), expected);
    let locked: HashSet<usize> = [0].into_iter().collect();
    let kept: Vec<String> = ["A", "B", "B", "B"].iter().map(|s| s.to_string()).collect();
    assert_eq!(enforce_spot_runs(&kept, 3, &locked), kept);
    assert!(enforce_spot_runs(&[], 3, &HashSet::new()).is_empty());
}
