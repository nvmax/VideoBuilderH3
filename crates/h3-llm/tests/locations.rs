//! The location scout against `fixtures/llm/locations.json`, recorded from the pack.

use std::path::Path;

use h3_llm::locations::{
    clean_location_context_text, extract_json_object, looks_like_location_meta_text, parse_scout_reply, repair_json_like_text, scout_instruction, scout_schema, subject_context_for_locations, ScoutInput,
};
use serde_json::Value;

fn fixture() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/llm/locations.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or("").to_string()
}

fn input_from(v: &Value) -> ScoutInput {
    ScoutInput {
        lyrics_text: text(&v["lyrics_text"]),
        scenes: v["scenes"].as_array().unwrap().iter().map(|s| (text(&s["concept"]), text(&s["notes"]))).collect(),
        style_theme: text(&v["style_theme"]),
        subject_context: text(&v["subject_context"]),
        existing_locations: v["existing_locations"].as_array().unwrap().iter().map(|l| text(&l["name"])).collect(),
        ..ScoutInput::default()
    }
}

#[test]
fn prompt_and_reply_handling_match_the_pack() {
    let data = fixture();
    assert_eq!(scout_schema(), data["schema"]);
    let cases = data["scout"].as_array().unwrap();
    assert!(cases.len() >= 40);
    for (n, case) in cases.iter().enumerate() {
        let instruction = scout_instruction(&input_from(&case["input"]));
        match case["instruction"].as_str() {
            // The pack stopped before it asked the model.
            None => assert_eq!(instruction.unwrap_err(), text(&case["return"]["error"]), "case {n}"),
            Some(expected) => {
                assert_eq!(instruction.unwrap(), expected, "instruction of case {n}");
                let got = parse_scout_reply(case["reply"].as_str().unwrap());
                match (case["return"].get("locations"), case["return"].get("error")) {
                    (Some(expected), _) => {
                        let locations: Vec<Value> = got.unwrap_or_else(|e| panic!("case {n}: {e}")).into_iter().map(|l| serde_json::json!({"name": l.name, "description": l.description})).collect();
                        assert_eq!(&Value::Array(locations), expected, "locations of case {n}");
                    }
                    (_, Some(error)) => assert_eq!(got.unwrap_err(), text(error), "error of case {n}"),
                    _ => panic!("case {n}"),
                }
            }
        }
    }
}

#[test]
fn json_repair_and_extraction_match_the_pack() {
    let data = fixture();
    for case in data["repair"].as_array().unwrap() {
        assert_eq!(repair_json_like_text(case["input"].as_str().unwrap()), text(&case["return"]), "repair of {}", case["input"]);
    }
    for case in data["extract"].as_array().unwrap() {
        let got = extract_json_object(case["input"].as_str().unwrap());
        match case["return"].get("value") {
            Some(value) => assert_eq!(&got.unwrap_or_else(|e| panic!("{}: {e}", case["input"])), value, "extract {}", case["input"]),
            None => assert!(got.is_err(), "extract {} should fail", case["input"]),
        }
    }
}

#[test]
fn context_cleaning_and_subject_notes_match_the_pack() {
    let data = fixture();
    for case in data["context"].as_array().unwrap() {
        let input = case["input"].as_str().unwrap();
        assert_eq!(clean_location_context_text(input), text(&case["return"]), "clean {input:?}");
        assert_eq!(looks_like_location_meta_text(input), case["meta"].as_bool().unwrap(), "meta {input:?}");
    }
    for case in data["subject_context"].as_array().unwrap() {
        assert_eq!(subject_context_for_locations(&case["refs"]), text(&case["return"]), "refs {}", case["refs"]);
    }
}
