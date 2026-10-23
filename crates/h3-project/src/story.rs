//! Storyboard scene defaults and the story idea, saved where the Video Builder keeps them. Port of `set_story_settings`
//! in `agent_api/orchestrator/storyboard_orchestrator.py`. The story steps that need an LLM, and Story Mode, belong to the
//! Storyboard Builder, which this app does not have, so only the settings are here.

use std::path::Path;

use h3_core::pyval::{text_or_empty, to_float, truthy, Obj};
use serde_json::{json, Value};

use crate::layout::Stamp;
use crate::scene_ops::{open, OpError, OpResult};

pub const STORY_LAYER_KEYS: [&str; 7] =
    ["enabled", "overall_story_idea", "user_story_arc", "song_story_brief", "lyric_story_strength", "image_world_style", "image_custom_style_direction"];
/// Fields of the Storyboard's Story Mode. They are known so the error text matches the pack, but not supported here.
pub const STORY_MODE_EDIT_KEYS: [&str; 9] =
    ["story_book_text", "story_settings", "story_acts", "story_scene_plan", "story_spot_min_scenes", "story_cast_ids", "story_allow_new_cast", "story_video_style", "story_continuation_mode"];
const IMAGE_WORLD_STYLES: [&str; 6] = ["natural", "surreal_subject", "balanced_surreal", "full_surreal", "abstract", "custom"];
pub const DEFAULT_KEYS: [&str; 26] = [
    "global_consistency_phrase", "camera_motion_speed", "character_motion_speed", "minimax_h3_cut_frequency", "minimax_h3_cut_style", "defaults_speed_preset", "defaults_music_preset", "camera_guidance",
    "character_guidance", "performance_style", "short_film_planning_mode", "camera_flow", "custom_camera_flow_sequence", "image_shot_flow", "image_aesthetic", "video_style", "video_style_custom",
    "temporal_world_effect", "temporal_world_effect_custom", "temporal_allow_background_extras", "temporal_background_intensity", "temporal_environment_time_passage", "temporal_protected_characters",
    "temporal_protected_custom", "fx_preset", "fx_custom_json",
];
const SPEED_KEYS: [&str; 3] = ["camera_motion_speed", "character_motion_speed", "temporal_background_intensity"];
const BOOLEAN_DEFAULT_KEYS: [&str; 2] = ["temporal_allow_background_extras", "temporal_environment_time_passage"];
const TEMPORAL_PROTECTED_CHARACTERS: [&str; 3] = ["all_referenced", "lead_only", "custom"];
const SHORT_FILM_PLANNING_MODES: [&str; 2] = ["guided_film", "fully_custom"];
const STORY_ARC_DETAILS: [&str; 4] = ["compact", "standard", "detailed", "rich"];

fn invalid(message: impl Into<String>) -> OpError {
    OpError::Validation(message.into())
}

fn text(value: Option<&Value>) -> String {
    text_or_empty(value).trim().to_string()
}

/// The saved story layer with the Builder's defaults filled in.
pub fn story_layer(session: &Obj) -> Obj {
    let stored = session.get("builder_story_layer").filter(|v| truthy(v)).or_else(|| session.get("builderStoryLayer"));
    let mut layer: Obj = stored.and_then(Value::as_object).cloned().unwrap_or_default();
    layer.entry("enabled").or_insert(json!(true));
    for key in ["overall_story_idea", "user_story_arc", "song_story_brief", "image_custom_style_direction"] {
        let value = text(layer.get(key));
        layer.insert(key.into(), json!(value));
    }
    let strength = layer.get("lyric_story_strength").cloned().unwrap_or(json!(7));
    layer.insert("lyric_story_strength".into(), strength);
    let style = layer.get("image_world_style").filter(|v| truthy(v)).cloned().unwrap_or(json!("natural"));
    layer.insert("image_world_style".into(), style);
    layer
}

fn story_defaults(session: &Obj) -> Obj {
    session.get("builder_storyboard_defaults").and_then(Value::as_object).cloned().unwrap_or_default()
}

/// A 0 to 10 slider value. Out of range, or not a number, is an error.
fn number_0_to_10(value: &Value, key: &str) -> Result<f64, OpError> {
    let number = to_float(value).unwrap_or(f64::NAN);
    if (0.0..=10.0).contains(&number) { Ok(number) } else { Err(invalid(format!("{key} must be a number from 0 to 10."))) }
}

fn one_of(value: Option<&Value>, allowed: &[&str]) -> bool {
    match value {
        None | Some(Value::Null) => true,
        Some(Value::String(s)) => allowed.contains(&s.as_str()),
        Some(_) => false,
    }
}

/// `re.sub(r"[\s-]+", "_", text)`.
fn underscored(text: &str) -> String {
    let mut out = String::new();
    let mut in_run = false;
    for c in text.chars() {
        if c.is_whitespace() || c == '-' {
            if !in_run {
                out.push('_');
            }
            in_run = true;
        } else {
            out.push(c);
            in_run = false;
        }
    }
    out
}

fn revision_of(session: &Obj) -> i64 {
    let pick = |key: &str| session.get(key).filter(|v| truthy(v)).and_then(to_float).map(|f| f as i64);
    pick("revision").or_else(|| pick("builder_save_revision")).unwrap_or(0)
}

/// The saved story layer as it is, whatever it holds: `builder_story_layer`, else the camel case key, else `story`.
pub fn saved_story(session: &Obj) -> Value {
    ["builder_story_layer", "builderStoryLayer", "story"].iter().filter_map(|k| session.get(*k)).find(|v| truthy(v)).cloned().unwrap_or_else(|| json!({}))
}

/// Replace the story layer with `story`. `if_match` must equal the saved revision when given.
pub fn put_story(project: &Path, story: &Value, if_match: Option<i64>, now: &Stamp) -> OpResult {
    let mut edit = open(project, None)?;
    let current = revision_of(&edit.session);
    if let Some(incoming) = if_match.filter(|i| *i != current) {
        return Err(OpError::Conflict { current, incoming });
    }
    edit.session.insert("builder_story_layer".into(), story.clone());
    edit.session.shift_remove("builderStoryLayer");
    let revision = edit.save(now)?;
    Ok(json!({"story": story, "revision": revision}))
}

/// Save scene defaults (video style, camera flow, motion speeds) and story fields. Both groups are optional.
/// `if_match` must equal the saved revision when given.
pub fn set_story_settings(project: &Path, params: &Obj, if_match: Option<i64>, now: &Stamp) -> OpResult {
    let mut defaults_in: Obj = params.get("defaults").and_then(Value::as_object).cloned().unwrap_or_default();
    let mut story_in: Obj = params.get("story").and_then(Value::as_object).cloned().unwrap_or_default();
    let mut unknown: Vec<&String> = defaults_in.keys().filter(|k| !DEFAULT_KEYS.contains(&k.as_str()) && k.as_str() != "story_arc_detail").collect();
    unknown.sort();
    if !unknown.is_empty() {
        return Err(invalid(format!("Unknown scene default(s): {}. Allowed: {}, story_arc_detail.", unknown.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "), DEFAULT_KEYS.join(", "))));
    }
    let mut unknown_story: Vec<&String> = story_in.keys().filter(|k| !STORY_LAYER_KEYS.contains(&k.as_str()) && !STORY_MODE_EDIT_KEYS.contains(&k.as_str())).collect();
    unknown_story.sort();
    if !unknown_story.is_empty() {
        let allowed: Vec<&str> = STORY_LAYER_KEYS.iter().chain(STORY_MODE_EDIT_KEYS.iter()).copied().collect();
        return Err(invalid(format!("Unknown story field(s): {}. Allowed: {}.", unknown_story.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "), allowed.join(", "))));
    }
    let mut story_mode: Vec<&String> = story_in.keys().filter(|k| STORY_MODE_EDIT_KEYS.contains(&k.as_str())).collect();
    story_mode.sort();
    if !story_mode.is_empty() {
        return Err(invalid(format!("Story Mode fields are not supported by this app: {}.", story_mode.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "))));
    }
    for key in SPEED_KEYS {
        if let Some(value) = defaults_in.get(key) {
            let number = number_0_to_10(value, key)?;
            defaults_in.insert(key.into(), json!(number));
        }
    }
    for key in BOOLEAN_DEFAULT_KEYS {
        if defaults_in.get(key).is_some_and(|v| !v.is_boolean()) {
            return Err(invalid(format!("{key} must be true or false.")));
        }
    }
    if story_in.get("enabled").is_some_and(|v| !v.is_boolean()) {
        return Err(invalid("enabled must be true or false."));
    }
    if !one_of(defaults_in.get("temporal_protected_characters"), &TEMPORAL_PROTECTED_CHARACTERS) {
        return Err(invalid(format!("temporal_protected_characters must be one of: {}.", TEMPORAL_PROTECTED_CHARACTERS.join(", "))));
    }
    if let Some(value) = defaults_in.get("short_film_planning_mode") {
        let spelled = underscored(&text(Some(value)).to_lowercase());
        let mode = if spelled == "custom" { "fully_custom".to_string() } else { spelled };
        if !SHORT_FILM_PLANNING_MODES.contains(&mode.as_str()) {
            return Err(invalid(format!("short_film_planning_mode must be one of: {}.", SHORT_FILM_PLANNING_MODES.join(", "))));
        }
        defaults_in.insert("short_film_planning_mode".into(), json!(mode));
    }
    if !one_of(defaults_in.get("story_arc_detail"), &STORY_ARC_DETAILS) {
        return Err(invalid(format!("story_arc_detail must be one of: {}.", STORY_ARC_DETAILS.join(", "))));
    }
    if !one_of(story_in.get("image_world_style"), &IMAGE_WORLD_STYLES) {
        return Err(invalid(format!("image_world_style must be one of: {}.", IMAGE_WORLD_STYLES.join(", "))));
    }
    if let Some(value) = story_in.get("lyric_story_strength") {
        let number = number_0_to_10(value, "lyric_story_strength")?;
        story_in.insert("lyric_story_strength".into(), json!(number));
    }

    let mut edit = open(project, None)?;
    let current = revision_of(&edit.session);
    if let Some(incoming) = if_match.filter(|i| *i != current) {
        return Err(OpError::Conflict { current, incoming });
    }
    let mut defaults = story_defaults(&edit.session);
    for (key, value) in defaults_in {
        defaults.insert(key, value);
    }
    edit.session.insert("builder_storyboard_defaults".into(), Value::Object(defaults.clone()));
    let mut layer = story_layer(&edit.session);
    for (key, value) in story_in {
        layer.insert(key, value);
    }
    edit.session.insert("builder_story_layer".into(), Value::Object(layer.clone()));
    edit.session.shift_remove("builderStoryLayer");
    let revision = edit.save(now)?;
    Ok(json!({"defaults": defaults, "story": layer, "revision": revision}))
}
