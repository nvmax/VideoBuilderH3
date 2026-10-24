//! "Auto-Generate Scenes" of the Speaking (short film) mode: the LLM plans a dialogue-driven short film as scene cards the user reviews before
//! anything reaches the timeline. Port of `_build_minimax_dialogue_scenes` in `storyboard/dialogue_scenes.py`.
//!
//! An imported script from the Script Mapper locks the dialogue, the order, the durations and the times. The LLM only adds the visual side.

use h3_core::pyjson::dumps;
use h3_core::pyval::{py_type_name, round_half_even, text_or_empty, to_float, truthy, Obj};
use h3_core::story::dialogue::{apply_authoritative_script_plan, normalize_generated_scenes};
use h3_core::story::helpers::{dialogue_reference_catalog, first_truthy, normalize_story_layer};
use h3_core::story::mode::text;
use h3_core::story::scene::{normalize_script_import, normalize_storyboard_scene};
use h3_llm::dialogue_prompts::{dialogue_json_repair_instruction, dialogue_plan_schema, dialogue_planner_instruction, PlannerInput};
use h3_llm::locations::extract_json_object;
use h3_llm::runner::runner_kind;
use serde_json::{json, Value};

use crate::prompt_writers::{number, whole, Llm, TextCall};

type Reply = Result<Value, String>;

fn array(value: Option<&Value>) -> Vec<Value> {
    match value {
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    }
}

/// `max(0, min(10, int(round(float(a or b or c or default)))))`: a motion speed from the first setting that is not falsy.
fn motion_speed(candidates: [Option<&Value>; 3], default: i64) -> i64 {
    let chosen = candidates.into_iter().flatten().find(|v| truthy(v));
    chosen.map_or(Some(default as f64), to_float).filter(|f| f.is_finite()).map_or(default, round_half_even).clamp(0, 10)
}

/// `_authoritative_script_from_payload`.
fn authoritative_script(payload: &Obj) -> Option<Value> {
    let storyboard = payload.get("storyboard").and_then(Value::as_object);
    let pick = |map: &Obj| ["script_import", "scriptImport"].iter().filter_map(|k| map.get(*k)).find(|v| truthy(v)).cloned();
    let source = pick(payload).or_else(|| storyboard.and_then(pick));
    let normalized = normalize_script_import(source.as_ref());
    let usable = normalized.get("enabled").is_some_and(truthy) && normalized.get("cues").is_some_and(truthy);
    usable.then_some(normalized)
}

/// `_authoritative_script_text`.
fn authoritative_script_text(script: &Value) -> String {
    let raw = text(script.get("raw_text"), 100000);
    if !raw.is_empty() {
        return raw;
    }
    array(script.get("cues"))
        .iter()
        .filter(|cue| cue.get("text").is_some_and(truthy))
        .map(|cue| {
            let speaker = ["speaker_alias", "speaker_name"].iter().filter_map(|k| cue.get(*k)).find(|v| truthy(v)).map_or_else(|| "Speaker".to_string(), |v| text_or_empty(Some(v)));
            format!("{speaker}: {}", text_or_empty(cue.get("text")))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `POST storyboard/minimax_dialogue_scenes`.
pub async fn build_minimax_dialogue_scenes(llm: &dyn Llm, payload: &Obj) -> Reply {
    let script = authoritative_script(payload);
    let story_layer = normalize_story_layer(first_truthy(payload, &["story_layer", "storyLayer"]));
    let storyboard = payload.get("storyboard").and_then(Value::as_object).cloned().unwrap_or_default();
    let camera_flow = text(["camera_flow", "cameraFlow"].iter().filter_map(|k| payload.get(*k)).chain(storyboard.get("camera_flow")).find(|v| truthy(v)).or(Some(&json!("balanced"))), 120);
    let camera_speed = motion_speed([payload.get("camera_motion_speed"), payload.get("cameraMotionSpeed"), storyboard.get("camera_motion_speed")], 4);
    let character_speed = motion_speed([payload.get("character_motion_speed"), payload.get("characterMotionSpeed"), storyboard.get("character_motion_speed")], 4);
    let source_text = match &script {
        Some(script) => authoritative_script_text(script),
        None => {
            let own = first_truthy(payload, &["story_source", "storySource"]).or_else(|| ["user_story_arc", "song_story_brief"].iter().filter_map(|k| story_layer.get(*k)).find(|v| truthy(v)));
            text_or_empty(own)
        }
    };
    let story_source = text(Some(&json!(source_text)), if script.is_some() { 100000 } else { 12000 });
    let asked_count = first_truthy(payload, &["scene_count", "sceneCount"]).map_or(Some(6.0), to_float).filter(|f| f.is_finite()).map_or(6, |f| f.trunc() as i64);
    let planned_count = script.as_ref().map(|s| array(s.get("scene_plan").and_then(|plan| plan.get("scenes"))).len() as i64);
    let scene_count = match planned_count {
        Some(count) => (if count > 0 { count } else { asked_count }).clamp(1, 80),
        None => asked_count.clamp(1, 24),
    };
    let (subjects, locations) = dialogue_reference_catalog(payload);
    let mut compact_existing: Vec<Value> = Vec::new();
    for (index, scene) in array(payload.get("scenes")).iter().take(24).enumerate() {
        if !scene.is_object() {
            continue;
        }
        let normalized = normalize_storyboard_scene(scene, index as i64 + 1);
        compact_existing.push(json!({
            "scene_number": normalized.get("scene_number").cloned().unwrap_or_else(|| json!(index + 1)),
            "label": normalized.get("label").cloned().unwrap_or_else(|| json!("")),
            "dialogue": normalized.get("lyrics").cloned().unwrap_or_else(|| json!("")),
            "story_beat": normalized.get("story_beat").cloned().unwrap_or_else(|| json!("")),
        }));
    }
    let listed = |items: &[Value]| if items.is_empty() { "[none provided]".to_string() } else { dumps(&Value::Array(items.to_vec())) };
    let plan_json = match &script {
        Some(script) => dumps(script.get("scene_plan").filter(|v| truthy(v)).unwrap_or(&json!({}))),
        None => "[none]".to_string(),
    };
    let instruction = dialogue_planner_instruction(&PlannerInput {
        has_authoritative_script: script.is_some(),
        camera_flow: &camera_flow,
        camera_motion_speed: camera_speed,
        character_motion_speed: character_speed,
        scene_count,
        story_source: &story_source,
        script_mapper_plan_json: &plan_json,
        story_layer_json: &dumps(&story_layer),
        project_motion_settings_json: &dumps(&json!({"camera_flow": camera_flow, "camera_motion_speed": camera_speed, "character_motion_speed": character_speed})),
        subjects_json: &listed(&subjects),
        locations_json: &listed(&locations),
        compact_existing_json: &if compact_existing.is_empty() { "[none]".to_string() } else { dumps(&Value::Array(compact_existing)) },
    });
    let call = TextCall::new(instruction.clone(), number(payload, "temperature", 0.55)?, number(payload, "top_p", 0.92)?, whole(payload, "max_new_tokens", 1400.max(scene_count * 280))?, "MiniMax Short Film Dialogue Scenes LLM").paragraphs();
    let (reply, info) = llm.text(call).await?;
    let data = match extract_json_object(&reply) {
        Ok(data) => data,
        Err(parse_error) => {
            // Runners that take a schema run the plan again, held to valid JSON. The text repair prompt is the fallback. The first call stays free,
            // because schema decoding is much slower on the built-in runner.
            let (retry_instruction, schema) = if runner_kind(payload) != "llm_api" { (instruction, Some(dialogue_plan_schema())) } else { (dialogue_json_repair_instruction(&reply), None) };
            let mut repair = TextCall::new(retry_instruction, 0.1, 0.8, whole(payload, "max_new_tokens", 2200.max(scene_count * 520))?, "MiniMax Short Film Dialogue JSON Repair").paragraphs();
            repair.json_schema = schema;
            let (repaired, _) = llm.text(repair).await?;
            extract_json_object(&repaired).map_err(|_| format!("Gemma returned malformed dialogue-plan JSON and repair failed. Original parse error: {parse_error}"))?
        }
    };
    let Value::Object(data) = data else { return Err(format!("'{}' object has no attribute 'get'", py_type_name(&data))) };
    let rows = match &script {
        Some(script) => Some(Value::Array(apply_authoritative_script_plan(data.get("scenes"), script))),
        None => data.get("scenes").cloned(),
    };
    let scenes = normalize_generated_scenes(rows.as_ref(), &subjects, &locations, payload.get("minimax_h3_mode"), &camera_flow, camera_speed)?;
    let count = scenes.len();
    let unloaded = info.get("unloaded").cloned().unwrap_or_else(|| json!(true));
    Ok(json!({
        "title": text(data.get("title"), 200),
        "premise": text(first_truthy(&data, &["premise"]).or(Some(&json!(story_source))), 4000),
        "scenes": scenes,
        "scene_count": count,
        "runner": info.get("runner").cloned().unwrap_or_else(|| json!("builtin")),
        "used_model": info.get("used_model").cloned().unwrap_or_else(|| json!("")),
        "unloaded": unloaded,
        "authoritative_script_used": script.is_some(),
    }))
}
