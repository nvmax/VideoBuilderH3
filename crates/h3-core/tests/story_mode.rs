//! Story Mode data against the cases the pack checks its own Python and JavaScript twins with (`fixtures/cases/story_mode_cases.json`).

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use h3_core::story::mode::*;
use serde_json::{json, Value};

fn cases() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/cases/story_mode_cases.json");
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn strings(value: &Value) -> Vec<String> {
    value.as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect()
}

#[test]
fn book_text_parses_and_prints_like_the_pack() {
    for item in cases()["parse"].as_array().unwrap() {
        let name = &item["name"];
        let parsed = parse_story_book_text(item["text"].as_str().unwrap());
        assert_eq!(parsed.book, item["book"], "{name}: book");
        assert_eq!(Value::Object(parsed.setting_names.clone()), item["setting_names"], "{name}: setting names");
        let settings = Value::Array(sync_story_settings(&parsed.book, &parsed.setting_names, None));
        let text = story_book_to_text(&parsed.book, Some(&settings));
        assert_eq!(text, item["text_out"].as_str().unwrap(), "{name}: text");
        assert_eq!(parse_story_book_text(&text).book, parsed.book, "{name}: round trip");
    }
}

#[test]
fn pasted_stories_split_like_the_pack() {
    for item in cases()["split"].as_array().unwrap() {
        let name = &item["name"];
        let paragraphs = strings(&item["paragraphs"]);
        let (chapters, names) = chapters_from_split(&paragraphs, Some(&item["split"]));
        assert_eq!(Value::Array(chapters.clone()), item["chapters"], "{name}: chapters");
        assert_eq!(Value::Object(names), item["setting_names"], "{name}: names");
        assert!(chapters_keep_text(&paragraphs, &chapters), "{name}: keeps text");
    }
}

#[test]
fn story_layer_fields_normalize_like_the_pack() {
    for item in cases()["normalize"].as_array().unwrap() {
        let got = Value::Object(normalize_story_mode_fields(item["input"].as_object().unwrap()));
        assert_eq!(got, item["output"], "{}", item["name"]);
    }
}

#[test]
fn continuations_resolve_like_the_pack() {
    for item in cases()["continuations"].as_array().unwrap() {
        let scenes = item["scenes"].as_array().unwrap();
        let plan = resolve_story_continuations(Some(&item["plan"]), scenes, Some(&item["acts"]), Some(&item["mode"]));
        let got: BTreeMap<String, Value> = plan.iter().map(|e| (e.scene_id.clone(), json!([e.continues_previous, e.continuation_source]))).collect();
        let want: BTreeMap<String, Value> = item["expect"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        assert_eq!(got, want, "{}", item["name"]);
    }
}

#[test]
fn a_continued_scene_says_so_in_its_beat() {
    let layer = json!({"story_scene_plan": [
        {"scene_id": "s1", "story_slice": "She reads."},
        {"scene_id": "s2", "story_slice": "She cries.", "continues_previous": true, "continuation_direction": "She drops the letter."},
    ]});
    let (_, brief) = story_beat_context(&layer, "s2", Some(&json!(2))).unwrap();
    assert!(brief.contains("next moment of Scene 1 in one unbroken camera take"));
    assert!(brief.contains("Then: She drops the letter."));
    assert!(!story_beat_context(&layer, "s1", Some(&json!(1))).unwrap().1.contains("Continuation:"));
}
