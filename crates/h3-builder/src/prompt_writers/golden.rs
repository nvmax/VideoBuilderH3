//! The prompt writers against what the pack's own writers asked and returned for the same requests
//! (`tools/make_image_writer_fixtures.py`, run with a recorder in place of the LLM).

use std::path::PathBuf;

use serde_json::{json, Value};

use super::concepts::generate_concept_prompts;
use super::image_prompts::{analyze_story_references, edit_image_prompt, generate_flux_klein_prompt, generate_nb_image_prompt, generate_t2i};
use super::testing::{Asked, Canned};
use h3_core::pyval::Obj;

fn fixture(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/llm").join(name);
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Replace `from` by `to` in every string of a value.
fn swap(value: &mut Value, from: &str, to: &str) {
    match value {
        Value::String(s) => *s = s.replace(from, to),
        Value::Array(items) => items.iter_mut().for_each(|v| swap(v, from, to)),
        Value::Object(map) => map.values_mut().for_each(|v| swap(v, from, to)),
        _ => {}
    }
}

fn asked_as_json(asked: &[Asked]) -> Value {
    Value::Array(
        asked
            .iter()
            .map(|call| match call {
                Asked::Text(c) => json!({"kind": "text", "instruction": c.instruction, "temperature": c.temperature, "top_p": c.top_p, "max_new_tokens": c.max_new_tokens, "label": c.label, "preserve_paragraphs": c.preserve_paragraphs, "json_schema": c.json_schema.is_some()}),
                Asked::Vision { instruction, sizes, temperature, top_p, max_new_tokens } => json!({"kind": "vision", "instruction": instruction, "temperature": temperature, "top_p": top_p, "max_new_tokens": max_new_tokens, "images": sizes.iter().map(|(w, h)| json!([w, h])).collect::<Vec<_>>()}),
            })
            .collect(),
    )
}

#[tokio::test]
async fn the_image_writers_match_the_pack() {
    let data = fixture("image_writers.json");
    let dir = tempfile::tempdir().unwrap();
    for (name, body) in [("theme.txt", "Noir, neon, rain.\n"), ("story.txt", "A woman walks home.\n"), ("subject.txt", "The singer: Ann.\n")] {
        std::fs::write(dir.path().join(name), body).unwrap();
    }
    let folder = dir.path().display().to_string();
    let cases = data["cases"].as_array().unwrap();
    assert!(cases.len() >= 50, "{} cases", cases.len());
    for (number, case) in cases.iter().enumerate() {
        let mut request = case["request"].clone();
        swap(&mut request, "<TMP>", &folder);
        let mut payload: Obj = json!({"text_runner": "lm_studio", "lmstudio_model": "test-model"}).as_object().cloned().unwrap();
        payload.extend(request.as_object().cloned().unwrap());
        let replies: Vec<String> = case["replies"].as_array().unwrap().iter().map(|r| r.as_str().unwrap().to_string()).collect();
        let replies: Vec<&str> = replies.iter().map(String::as_str).collect();
        let llm = Canned::new(&replies);
        let result = match case["fn"].as_str().unwrap() {
            "t2i" => generate_t2i(&llm, &payload).await,
            "edit" => edit_image_prompt(&llm, &payload).await,
            "flux" => generate_flux_klein_prompt(&llm, &payload).await,
            "nano" => generate_nb_image_prompt(&llm, &payload).await,
            "stories" => analyze_story_references(&llm, &payload).await,
            "concepts" => generate_concept_prompts(&llm, &payload).await,
            other => panic!("unknown function {other}"),
        };
        let mut asked = asked_as_json(&llm.asked.lock().unwrap());
        swap(&mut asked, &folder, "<TMP>");
        let what = format!("case {number} ({}) {}", case["fn"], case["request"]);
        // The pack's recorder keeps no label for picture calls, and text calls without a schema carry `json_schema: false`.
        let mut expected_calls = case["calls"].clone();
        for call in expected_calls.as_array_mut().unwrap() {
            if call["kind"] == "vision" {
                call.as_object_mut().unwrap().remove("label");
            }
        }
        assert_eq!(asked, expected_calls, "{what}: what the LLM was asked");
        match (result, case.get("error")) {
            (Err(message), Some(expected)) => {
                let mut message = Value::String(message);
                swap(&mut message, &folder, "<TMP>");
                assert_eq!(message, *expected, "{what}: error");
            }
            (Ok(mut value), None) => {
                swap(&mut value, &folder, "<TMP>");
                assert_eq!(value, case["return"], "{what}: result");
            }
            (Ok(value), Some(expected)) => panic!("{what}: returned {value} but the pack raised {expected}"),
            (Err(message), None) => panic!("{what}: raised {message} but the pack returned {}", case["return"]),
        }
    }
}
