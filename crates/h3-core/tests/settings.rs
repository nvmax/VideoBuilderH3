//! Settings, continuity modes and render payload against `fixtures/core` and `fixtures/scene`.

mod common;

use h3_core::continuity::{canonical_continuity_mode, normalize_continuity_mode};
use h3_core::settings::{build_render_payload, normalize_settings, render_pass, settings_for_scene, validate_patch, workflow_key};
use serde_json::{json, Value};

#[test]
fn continuity_spellings() {
    for c in common::cases("scene/continuity_modes.json") {
        let v = c["input"]["value"].clone();
        assert_eq!(json!(canonical_continuity_mode(Some(&v))), c["canonical"], "{c}");
        assert_eq!(json!(normalize_continuity_mode(Some(&v))), c["normalized"], "{c}");
    }
}

#[test]
fn settings_normalize() {
    let cases = common::cases("core/settings_normalize.json");
    assert!(cases.len() > 40);
    for c in cases {
        let got = Value::Object(normalize_settings(Some(&c["input"]["raw"])));
        common::assert_json(&got, &c["output"], &c);
    }
}

#[test]
fn settings_patch_validation() {
    for c in common::cases("core/settings_validate_patch.json") {
        let patch = c["input"]["patch"].as_object().unwrap();
        let got = Value::Object(validate_patch(patch));
        common::assert_json(&got, &c["output"], &c);
    }
}

#[test]
fn settings_for_a_scene() {
    for c in common::cases("core/settings_for_scene.json") {
        let segment = c["input"]["segment"].clone();
        let got = Value::Object(settings_for_scene(&c["input"]["session"], Some(&segment).filter(|s| !s.is_null())));
        common::assert_json(&got, &c["output"], &c);
    }
}

#[test]
fn render_payloads() {
    let cases = common::cases("core/settings_to_payload.json");
    assert!(cases.len() >= 40);
    for c in cases {
        let settings = normalize_settings(Some(&c["input"]["raw"]));
        match (build_render_payload(&settings, None), c.get("error")) {
            (Ok(payload), None) => {
                common::assert_json(&Value::Object(payload), &c["output"]["payload"], &c);
                assert_eq!(json!(render_pass(&settings)), c["output"]["render_pass"], "{c}");
                assert_eq!(json!(workflow_key(&settings)), c["output"]["workflow_key"], "{c}");
            }
            (Err(e), Some(err)) => assert_eq!(json!(e.message), err["message"], "{c}"),
            (got, _) => panic!("ok={} vs {c}", got.is_ok()),
        }
    }
}
