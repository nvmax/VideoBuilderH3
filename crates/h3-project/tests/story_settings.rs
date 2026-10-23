//! Story settings against `fixtures/timeline/story_settings.json`, recorded from the pack's `set_story_settings`.

use std::path::Path;

use h3_project::story::set_story_settings;
use h3_project::Stamp;
use serde_json::{json, Value};

#[test]
fn story_settings_match_the_pack() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/timeline/story_settings.json");
    let data: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let cases = data["cases"].as_array().unwrap();
    assert!(cases.len() > 200);
    for (n, case) in cases.iter().enumerate() {
        let dir = tempfile::tempdir().unwrap();
        let session = &data["sessions"][case["session"].as_str().unwrap()];
        std::fs::write(dir.path().join("vrgdg_builder_session.json"), serde_json::to_string_pretty(session).unwrap()).unwrap();
        let params = case["params"].as_object().cloned().unwrap_or_default();
        let if_match = case["if_match"].as_i64();
        let got = set_story_settings(dir.path(), &params, if_match, &Stamp::now());
        let label = format!("case {n}: {} {} {:?}", case["session"], case["params"], if_match);
        match (case["return"].get("result"), case["return"].get("error")) {
            (Some(expected), _) => {
                assert_eq!(&got.unwrap_or_else(|e| panic!("{label}: {e}")), expected, "{label}");
                let saved: Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("vrgdg_builder_session.json")).unwrap()).unwrap();
                let after = json!({
                    "builder_storyboard_defaults": saved.get("builder_storyboard_defaults").cloned().unwrap_or(Value::Null),
                    "builder_story_layer": saved.get("builder_story_layer").cloned().unwrap_or(Value::Null),
                    "revision": saved.get("revision").cloned().unwrap_or(Value::Null),
                    "builder_save_revision": saved.get("builder_save_revision").cloned().unwrap_or(Value::Null),
                    "has_camel": saved.get("builderStoryLayer").is_some(),
                });
                assert_eq!(after, case["return"]["after"], "saved session of {label}");
            }
            (_, Some(error)) => {
                assert_eq!(got.unwrap_err().to_string(), error.as_str().unwrap(), "{label}");
                let saved: Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("vrgdg_builder_session.json")).unwrap()).unwrap();
                assert_eq!(&saved, session, "a refused change leaves the session alone: {label}");
            }
            _ => panic!("{label}"),
        }
    }
}
