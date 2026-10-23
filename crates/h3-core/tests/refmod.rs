//! RefMod scene logic against the pack's own shared cases (`fixtures/cases/refmod_scene_cases.json`) and its unit tests.

mod common;

use common::load;
use h3_core::refmod::{apply_scene_item_users, assign_labels, compose_items, reference_payload, refmod_items_for_scene, scene_subject_cards, token_report, total_tokens};
use serde_json::{json, Value};

fn objs(v: &Value) -> Vec<Value> {
    v.as_array().cloned().unwrap_or_default()
}

#[test]
fn the_shared_cases_match() {
    let cases = load("cases/refmod_scene_cases.json");
    let cases = cases.as_array().unwrap();
    assert!(cases.len() >= 13);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let empty = serde_json::Map::new();
        let segment = case.get("scene").and_then(Value::as_object).unwrap_or(&empty);
        let subjects = scene_subject_cards(segment, objs(&case["subjects"]));
        let location = case.get("location").filter(|l| !l.is_null());
        let mut items = compose_items(&subjects, &objs(&case["extras"]), location, &objs(&case["all_subjects"]), case["override"].as_object());
        if case.get("scene_item_users").is_some_and(|u| u.as_object().is_some_and(|o| !o.is_empty())) {
            items = apply_scene_item_users(items, Some(&case["scene_item_users"]));
        }
        let labelled = assign_labels(&items, case["include_audio"].as_bool().unwrap_or(false));
        let actual: Vec<Value> = labelled.iter().map(|i| json!({"card_id": i["card_id"], "category": i["category"], "label": i["label"], "mod_name": i["mod_name"]})).collect();
        assert_eq!(Value::Array(actual), case["expected"], "{name}");
        assert_eq!(json!(total_tokens(&items)), case["expected_tokens"], "{name}: tokens");
    }
}

fn person(name: &str, tokens: i64, strength: f64, category: &str) -> serde_json::Map<String, Value> {
    json!({"name": name, "tokens": tokens, "strength": strength, "category": category}).as_object().unwrap().clone()
}

#[test]
fn the_token_report_names_an_unbalanced_cast() {
    let report = token_report(&[person("The man", 520, 1.0, "character"), person("Darrel", 2394, 1.0, "character"), person("Brandon", 1440, 1.0, "character")]);
    assert_eq!(report["imbalance"], json!({"weak": "The man", "strong": "Darrel", "ratio": 4.6}));
    assert_eq!((report["total"].as_i64(), report["over_limit"].as_bool()), (Some(4354), Some(false)));
    assert!(token_report(&[person("A", 520, 1.0, "character"), person("B", 1000, 0.6, "character")])["imbalance"].is_null());
    assert!(token_report(&[person("A", 1000, 1.0, "character"), person("Meadow", 5000, 1.0, "background")])["imbalance"].is_null());
    assert_eq!(token_report(&[person("A", 3500, 1.0, "character"), person("B", 3500, 1.0, "character")])["over_limit"], true);
}

fn card(id: &str, name: &str, kind: &str, strength: f64, reference_type: &str) -> Value {
    json!({"id": id, "name": name, "reference_type": reference_type, "source": "refmod", "refmod": {"name": format!("mods/{id}"), "kind": kind, "tokens": 100, "strength": strength}})
}

#[test]
fn a_scene_uses_the_standard_scene_maps() {
    let brad = card("s1", "Brad", "video", 1.0, "character");
    let darrel = card("s2", "Darrel", "video", 1.0, "character");
    let meadow = card("l1", "Meadow", "image", 1.0, "environment");
    let session = json!({"flux_reference_builder": {
        "use_subject_reference": true, "subject_count": 2, "subjects": [brad, darrel], "locations": [meadow],
        "subject_scene_map": {"scene-a": "s1,s2", "scene-b": "s2"}, "scene_map": {"scene-a": "l1"},
    }});
    let names = |items: &[serde_json::Map<String, Value>]| items.iter().map(|i| i["mod_name"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    let first = refmod_items_for_scene(&session, json!({"id": "scene-a"}).as_object().unwrap(), 0);
    assert_eq!(names(&first), ["mods/s1", "mods/s2", "mods/l1"]);
    let second = refmod_items_for_scene(&session, json!({"id": "scene-b"}).as_object().unwrap(), 1);
    assert_eq!(names(&second), ["mods/s2"]);
    let labels: Vec<String> = assign_labels(&first, true).iter().map(|i| i["label"].as_str().unwrap().to_string()).collect();
    assert_eq!(labels, ["<Video 1>", "<Video 2>", "<Picture 1>", "<Audio 1>"]);
}

#[test]
fn a_no_character_scene_uses_no_character_refmod() {
    let ayame = card("character_a", "Ayame", "video", 1.0, "character");
    let floors = card("loc_safehouse", "Safehouse floors", "video", 1.0, "environment");
    let session = json!({"flux_reference_builder": {
        "use_subject_reference": true, "subject_count": 1, "subjects": [ayame], "locations": [floors],
        "subject_scene_map": {"seg_a": ["character_a"]}, "scene_map": {"seg_5d0f59ee3de8": "loc_safehouse"},
    }});
    let segment = json!({"id": "seg_5d0f59ee3de8", "no_character_present": true});
    let items = assign_labels(&refmod_items_for_scene(&session, segment.as_object().unwrap(), 3), true);
    let seen: Vec<(String, String)> = items.iter().map(|i| (i["key"].as_str().unwrap().into(), i["label"].as_str().unwrap().into())).collect();
    assert_eq!(seen, [("location:loc_safehouse".to_string(), "<Video 1>".to_string()), ("audio:scene".to_string(), "<Audio 1>".to_string())]);
    let sung = assign_labels(&refmod_items_for_scene(&session, json!({"id": "seg_a"}).as_object().unwrap(), 0), false);
    assert_eq!(sung.iter().map(|i| i["mod_name"].as_str().unwrap()).collect::<Vec<_>>(), ["mods/character_a"]);
}

#[test]
fn the_payload_lists_visual_mods_only() {
    let brad = card("s1", "Brad", "video", 0.5, "character");
    let items = assign_labels(&compose_items(std::slice::from_ref(&brad), &[], None, std::slice::from_ref(&brad), None), true);
    let payload = reference_payload(&items);
    assert_eq!((payload.len(), payload[0]["strength"].as_f64()), (1, Some(0.5)));
}
