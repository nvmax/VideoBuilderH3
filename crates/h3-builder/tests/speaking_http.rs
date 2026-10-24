//! The ElevenLabs calls against what the pack's own calls asked and returned for the same scripts of replies (`fixtures/speaking/http.json`, made by
//! `tools/make_speaking_fixtures.py`).

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use base64::Engine;
use h3_builder::elevenlabs::{create_designed_voice, design_voice, generate_speech, list_voices, Failure, Reply, Request, SendFuture, Transport};
use serde_json::{json, Value};

fn cases() -> Vec<Value> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/speaking/http.json");
    serde_json::from_str::<Value>(&fs::read_to_string(path).unwrap()).unwrap()["cases"].as_array().cloned().unwrap()
}

/// Answers from a script and remembers every request as the fixture records it.
struct Scripted {
    script: Mutex<Vec<Value>>,
    seen: Mutex<Vec<Value>>,
}

impl Transport for Scripted {
    fn send(&self, request: Request, limit: usize) -> SendFuture<'_> {
        Box::pin(async move {
            let headers: serde_json::Map<String, Value> = request.headers.iter().map(|(k, v)| (k.to_lowercase(), json!(v))).collect();
            let body = request.body.as_ref().map(|b| serde_json::from_slice::<Value>(b).unwrap());
            self.seen.lock().unwrap().push(json!({"method": request.method, "url": request.url, "headers": headers, "body": body, "timeout": request.timeout_seconds}));
            let step = self.script.lock().unwrap().remove(0);
            if let Some(code) = step.get("http_error").and_then(Value::as_u64) {
                let text = step["body"].as_str().unwrap_or("").as_bytes();
                return Err(Failure::Status { code: code as u16, body: text[..text.len().min(8193)].to_vec() });
            }
            if step.get("connect").is_some() {
                return Err(Failure::Connect);
            }
            let bytes = match step.get("bytes_b64").and_then(Value::as_str) {
                Some(encoded) => base64::engine::general_purpose::STANDARD.decode(encoded).unwrap(),
                None => match &step["body"] {
                    Value::String(text) => text.clone().into_bytes(),
                    other => other.to_string().into_bytes(),
                },
            };
            Ok(Reply { body: bytes[..bytes.len().min(limit + 1)].to_vec(), content_type: step.get("content_type").and_then(Value::as_str).map(str::to_string) })
        })
    }
}

#[tokio::test]
async fn the_elevenlabs_calls_match_the_pack() {
    let all = cases();
    assert!(all.len() > 100, "{} cases", all.len());
    let mut failures = Vec::new();
    for (number, case) in all.iter().enumerate() {
        let http = Scripted { script: Mutex::new(case["script"].as_array().cloned().unwrap()), seen: Mutex::new(Vec::new()) };
        let a = case["args"].as_array().unwrap();
        let got: Result<Value, String> = match case["fn"].as_str().unwrap() {
            "list_voices" => list_voices(&http, &a[0], &a[1]).await,
            "generate_speech" => generate_speech(&http, &a[0], a[1].as_str().unwrap(), &a[2]).await,
            "design_voice" => design_voice(&http, &a[0], &a[1]).await,
            "create_designed_voice" => create_designed_voice(&http, &a[0], &a[1]).await,
            other => panic!("unknown function {other}"),
        };
        let seen = Value::Array(http.seen.lock().unwrap().clone());
        let same_calls = seen == case["requests"];
        let same_result = match (&got, case.get("error")) {
            (Ok(value), None) => value == &case["return"],
            (Err(message), Some(want)) => want.as_str() == Some(message.as_str()),
            _ => false,
        };
        if !(same_calls && same_result) {
            failures.push(format!("case {number} {}\n  calls {same_calls}, result {same_result}\n  got  {got:?}\n  want {}\n  asked {seen}\n  want  {}", case["fn"], case.get("error").unwrap_or(&case["return"]), case["requests"]));
        }
    }
    assert!(failures.is_empty(), "{} of {} cases differ\n{}", failures.len(), all.len(), failures.iter().take(3).cloned().collect::<Vec<_>>().join("\n"));
}
