//! Per-scene Scene Defaults, sharing the project's setting names. Port of `storyboard/scene_defaults.py`.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::pyval::Obj;

pub const SCENE_DEFAULT_KEYS: &[&str] = &[
    "camera_flow",
    "custom_camera_flow_sequence",
    "camera_guidance",
    "character_guidance",
    "camera_motion_speed",
    "character_motion_speed",
    "minimax_h3_cut_frequency",
    "minimax_h3_cut_style",
    "performance_style",
    "facial_performance",
    "facial_performance_custom",
    "global_consistency_phrase",
    "image_aesthetic",
    "image_shot_flow",
    "video_style",
    "video_style_custom",
    "temporal_world_effect",
    "temporal_world_effect_custom",
    "temporal_allow_background_extras",
    "temporal_background_intensity",
    "temporal_environment_time_passage",
    "temporal_protected_characters",
    "temporal_protected_custom",
    "fx_preset",
    "fx_custom_json",
];
const NUMBER_KEYS: &[&str] = &["camera_motion_speed", "character_motion_speed", "minimax_h3_cut_frequency", "temporal_background_intensity"];
const BOOL_KEYS: &[&str] = &["temporal_allow_background_extras", "temporal_environment_time_passage"];

fn valid(key: &str, item: &Value) -> bool {
    if NUMBER_KEYS.contains(&key) {
        return item.as_f64().is_some_and(|n| n.is_finite() && (0.0..=10.0).contains(&n));
    }
    if BOOL_KEYS.contains(&key) {
        return item.is_boolean();
    }
    if key == "custom_camera_flow_sequence" {
        return item.as_array().is_some_and(|rows| rows.len() <= 1000 && rows.iter().all(|row| row.is_string() || row.is_object()));
    }
    item.as_str().is_some_and(|text| text.chars().count() <= 20000)
}

/// Keep only scene level settings. False, zero and empty text are explicit overrides. `strict` refuses what `strict = false` drops.
pub fn normalize_scene_defaults(value: &Value, strict: bool) -> Result<Obj, String> {
    let Some(given) = value.as_object() else {
        return if strict { Err("scene_defaults must be an object.".into()) } else { Ok(Obj::new()) };
    };
    if strict {
        let unknown: BTreeSet<&str> = given.keys().map(String::as_str).filter(|key| !SCENE_DEFAULT_KEYS.contains(key)).collect();
        if !unknown.is_empty() {
            return Err(format!("Unknown scene_defaults fields: {}", unknown.into_iter().collect::<Vec<_>>().join(", ")));
        }
    }
    let mut result = Obj::new();
    for key in SCENE_DEFAULT_KEYS {
        let Some(item) = given.get(*key) else { continue };
        if !valid(key, item) {
            if strict {
                return Err(format!("Invalid scene_defaults value for {key}"));
            }
            continue;
        }
        result.insert((*key).to_string(), item.clone());
    }
    Ok(result)
}

/// The project's saved defaults with the scene's own overrides on top, as stored (`{**defaults, **segment["scene_defaults"]}`).
pub fn with_scene_overrides(defaults: &Obj, segment: &Obj) -> Obj {
    let mut merged = defaults.clone();
    if let Some(scoped) = segment.get("scene_defaults").and_then(Value::as_object) {
        merged.extend(scoped.clone());
    }
    merged
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn keeps_known_settings_in_the_pack_order_and_explicit_overrides() {
        let got = normalize_scene_defaults(&json!({"camera_motion_speed": 0, "camera_flow": "", "temporal_allow_background_extras": false}), true).unwrap();
        assert_eq!(serde_json::to_string(&got).unwrap(), r#"{"camera_flow":"","camera_motion_speed":0,"temporal_allow_background_extras":false}"#);
    }

    #[test]
    fn strict_refuses_what_loose_drops() {
        let bad = json!({"camera_motion_speed": 11, "unknown": 1});
        assert_eq!(normalize_scene_defaults(&bad, true).unwrap_err(), "Unknown scene_defaults fields: unknown");
        assert_eq!(normalize_scene_defaults(&json!({"camera_motion_speed": true}), true).unwrap_err(), "Invalid scene_defaults value for camera_motion_speed");
        assert!(normalize_scene_defaults(&bad, false).unwrap().is_empty());
        assert_eq!(normalize_scene_defaults(&json!([]), true).unwrap_err(), "scene_defaults must be an object.");
        assert!(normalize_scene_defaults(&json!("x"), false).unwrap().is_empty());
    }
}
