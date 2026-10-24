//! Story location cards against the cases the pack checks its own twins with (`fixtures/cases/story_locations_cases.json`).

use std::fs;
use std::path::PathBuf;

use h3_core::story::locations::{apply_story_locations, card_name};
use serde_json::{json, Value};

fn cases() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/cases/story_locations_cases.json");
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn card_names_match_the_pack() {
    for item in cases()["card_names"].as_array().unwrap() {
        assert_eq!(card_name(item["setting"].as_str().unwrap(), item["spot"].as_str().unwrap()), item["name"].as_str().unwrap(), "{item}");
    }
}

#[test]
fn location_cards_merge_like_the_pack() {
    for item in cases()["apply"].as_array().unwrap() {
        let mut count = 0;
        let mut new_id = |taken: &mut std::collections::HashSet<String>| {
            count += 1;
            let id = format!("loc_new_{count}");
            taken.insert(id.clone());
            id
        };
        let before = item["refs"].clone();
        let locations = item["locations"].as_array().unwrap();
        let (refs, counts) = apply_story_locations(&item["refs"], locations, &mut new_id, item["replace_descriptions"].as_bool().unwrap());
        assert_eq!(refs, item["refs_after"], "{}", item["name"]);
        assert_eq!(json!({"added": counts.added, "updated": counts.updated}), json!({"added": item["added"], "updated": item["updated"]}), "{}", item["name"]);
        assert_eq!(item["refs"], before);
    }
}
