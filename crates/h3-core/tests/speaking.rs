//! The Speaking mode checks and scene audio timing against what the pack's own functions returned for the same inputs (`fixtures/speaking/pure.json`,
//! made by `tools/make_speaking_fixtures.py`).

use std::fs;
use std::path::PathBuf;

use h3_core::speaking::*;
use serde_json::{json, Value};

fn cases() -> Vec<Value> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/speaking/pure.json");
    serde_json::from_str::<Value>(&fs::read_to_string(path).unwrap()).unwrap()["cases"].as_array().cloned().unwrap()
}

fn optional(value: &Value) -> Option<&Value> {
    Some(value).filter(|v| !v.is_null())
}

fn run(name: &str, args: &Value) -> Result<Value, String> {
    let a = args.as_array().unwrap();
    Ok(match name {
        "validate_api_key" => json!(validate_api_key(&a[0], a[1].as_bool().unwrap())?),
        "normalize_voice" => normalize_voice(&a[0]),
        "validate_voice" => validate_voice(&a[0])?,
        "normalize_dialogue" => normalize_dialogue(&a[0]),
        "validate_dialogue" => validate_dialogue(&a[0], a[1].as_bool().unwrap())?,
        "dialogue_speaker" => dialogue_speaker(Some(&a[0]), a[1].as_str().unwrap())?,
        "required_text" => json!(required_text(&a[0], a[1].as_str().unwrap(), a[2].as_u64().unwrap() as usize, a[3].as_u64().unwrap() as usize)?),
        "normalize_design_draft" => normalize_design_draft(&a[0]),
        "validate_design_draft" => validate_design_draft(&a[0])?,
        "spoken_words" => json!(spoken_words(a[0].as_str().unwrap())),
        "_voice_request_error" => {
            // The transport hands over at most the first 8193 bytes of an error body.
            let body = a[1].as_str().unwrap().as_bytes();
            let body = &body[..body.len().min(8193)];
            json!(voice_request_error(a[0].as_u64().unwrap() as u16, error_status(body).as_deref(), a[2].as_bool().unwrap()))
        }
        "validate_settings" => Value::Object(validate_settings(&a[0], a[1].as_bool().unwrap())?),
        "require_speaking" => {
            require_speaking(&a[0])?;
            Value::Null
        }
        "effective_settings" => {
            let settings = effective_settings(a[0].get("speaking_audio_defaults"), &a[1])?;
            json!({"silence_before": settings.silence_before, "silence_after": settings.silence_after, "fit_duration": settings.fit_duration})
        }
        "update_audio_settings" => update_audio_settings(&a[0], &a[1], a[2].as_str(), optional(&a[3]), optional(&a[4]))?,
        other => panic!("unknown function {other}"),
    })
}

#[test]
fn the_speaking_checks_match_the_pack() {
    let all = cases();
    assert!(all.len() > 200, "{} cases", all.len());
    let mut failures = Vec::new();
    for (number, case) in all.iter().enumerate() {
        let name = case["fn"].as_str().unwrap();
        let got = run(name, &case["args"]);
        let ok = match (&got, case.get("error")) {
            (Ok(value), None) => value == &case["return"],
            (Err(message), Some(want)) => want.as_str() == Some(message.as_str()),
            _ => false,
        };
        if !ok {
            failures.push(format!("case {number} {name}\n  args {}\n  got  {got:?}\n  want {}", case["args"], case.get("error").unwrap_or(&case["return"])));
        }
    }
    assert!(failures.is_empty(), "{} of {} cases differ\n{}", failures.len(), all.len(), failures.iter().take(4).cloned().collect::<Vec<_>>().join("\n"));
}
