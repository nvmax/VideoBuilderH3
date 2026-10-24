//! Shared by the golden tests of the LLM writers: run every case of a fixture file against the Rust writer and compare what the writer asked
//! the runner and what it returned with what the pack's own writer did (`tools/llm_harness.py`).

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use h3_core::pyval::Obj;
use serde_json::{json, Value};

use crate::prompt_writers::testing::{Asked, Canned};

pub type Answer<'a> = Pin<Box<dyn Future<Output = Result<Value, String>> + 'a>>;

pub fn fixture(file: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures").join(file);
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Replace `from` by `to` in every string of a value.
pub fn swap(value: &mut Value, from: &str, to: &str) {
    if from.is_empty() {
        return;
    }
    match value {
        Value::String(s) => *s = s.replace(from, to),
        Value::Array(items) => items.iter_mut().for_each(|v| swap(v, from, to)),
        Value::Object(map) => map.values_mut().for_each(|v| swap(v, from, to)),
        _ => {}
    }
}

pub fn asked_as_json(asked: &[Asked]) -> Value {
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

/// Run every case of a fixture file (`dir/name.json`) and compare. `folder` stands for `<TMP>` in the requests.
pub async fn check_cases(file: &str, minimum: usize, folder: &str, dispatch: for<'a> fn(&'a str, &'a Canned, &'a Obj) -> Answer<'a>) {
    let data = fixture(file);
    let cases = data["cases"].as_array().unwrap();
    assert!(cases.len() >= minimum, "{} cases", cases.len());
    for (number, case) in cases.iter().enumerate() {
        let mut request = case["request"].clone();
        swap(&mut request, "<TMP>", folder);
        let mut payload: Obj = json!({"text_runner": "lm_studio", "lmstudio_model": "test-model"}).as_object().cloned().unwrap();
        payload.extend(request.as_object().cloned().unwrap());
        let replies: Vec<String> = case["replies"].as_array().unwrap().iter().map(|r| r.as_str().unwrap().to_string()).collect();
        let replies: Vec<&str> = replies.iter().map(String::as_str).collect();
        let llm = Canned::new(&replies);
        let result = dispatch(case["fn"].as_str().unwrap(), &llm, &payload).await;
        let mut asked = asked_as_json(&llm.asked.lock().unwrap());
        swap(&mut asked, folder, "<TMP>");
        let what = format!("{file} case {number} ({}) {}", case["fn"], case["request"]);
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
                swap(&mut message, folder, "<TMP>");
                assert_eq!(message, *expected, "{what}: error");
            }
            (Ok(mut value), None) => {
                swap(&mut value, folder, "<TMP>");
                assert_eq!(value, case["return"], "{what}: result");
            }
            (Ok(value), Some(expected)) => panic!("{what}: returned {value} but the pack raised {expected}"),
            (Err(message), None) => panic!("{what}: raised {message} but the pack returned {}", case["return"]),
        }
    }
}
