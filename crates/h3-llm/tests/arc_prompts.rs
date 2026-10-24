//! The Story Brief and Story Arc instruction builders against the pack's own text for the same inputs (`fixtures/story/arc_prompts.json`, made by
//! `tools/make_story_arc_fixtures.py`).

use std::fs;
use std::path::PathBuf;

use h3_llm::arc_prompts::*;
use serde_json::{json, Value};

fn cases() -> Vec<Value> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/story/arc_prompts.json");
    serde_json::from_str::<Value>(&fs::read_to_string(path).unwrap()).unwrap()["cases"].as_array().cloned().unwrap()
}

fn s<'a>(args: &'a Value, key: &str) -> &'a str {
    args.get(key).and_then(Value::as_str).unwrap_or("")
}

fn n(args: &Value, key: &str) -> i64 {
    args.get(key).and_then(Value::as_i64).unwrap_or(0)
}

fn list(args: &Value, key: &str) -> Vec<String> {
    args.get(key).and_then(Value::as_array).map(|items| items.iter().map(|i| i.as_str().unwrap_or("").to_string()).collect()).unwrap_or_default()
}

fn run(name: &str, a: &Value) -> Value {
    match name {
        "_storyboard_script_story_instruction" => json!(script_story_instruction(s(a, "purpose"), s(a, "story_idea"), s(a, "subjects_json"), s(a, "locations_json"), s(a, "script_text"), s(a, "compact_plan_json"))),
        "_storyboard_story_brief_instruction" => json!(story_brief_instruction(s(a, "lyric_story_strength_guidance_text"), s(a, "user_story_arc"), s(a, "lyrics"), s(a, "compact_scenes_json"))),
        "_storyboard_story_arc_structure_instruction" => json!(arc_structure_instruction(&list(a, "required_section_labels"))),
        "_storyboard_story_arc_motion_guidance" => json!(arc_motion_guidance(n(a, "character_motion_speed"))),
        "_storyboard_timeline_note_story_instruction" => json!(timeline_note_story_instruction(s(a, "notes_json"))),
        "_storyboard_story_arc_format_retry_instruction" => json!(arc_format_retry_instruction(&list(a, "required_section_labels"), s(a, "original_instruction"))),
        "_storyboard_story_arc_schema" => arc_schema(&list(a, "required_section_labels")),
        "_storyboard_story_arc_json_retry_instruction" => json!(arc_json_retry_instruction(s(a, "original_instruction"))),
        "_storyboard_story_arc_instruction" => json!(story_arc_instruction(&ArcInput {
            required_section_labels: &list(a, "required_section_labels"),
            section_word_limit: n(a, "section_word_limit"),
            story_arc_seed: s(a, "story_arc_seed"),
            camera_flow: s(a, "camera_flow"),
            camera_motion_speed: n(a, "camera_motion_speed"),
            character_motion_speed: n(a, "character_motion_speed"),
            performance_style: s(a, "performance_style"),
            facial_performance: s(a, "facial_performance"),
            lyric_story_strength_guidance_text: s(a, "lyric_story_strength_guidance_text"),
            story_idea: s(a, "story_idea"),
            previous_story_arc: s(a, "previous_story_arc"),
            style_theme: s(a, "style_theme"),
            lyrics_source: s(a, "lyrics_source"),
            lyrics: s(a, "lyrics"),
            compact_scenes_json: s(a, "compact_scenes_json"),
            subjects_json: s(a, "subjects_json"),
            locations_json: s(a, "locations_json"),
            song_story_brief: s(a, "song_story_brief"),
        })),
        "_storyboard_story_arc_scene_entry_instruction" => json!(scene_entry_instruction(&EntryInput {
            section_label: s(a, "section_label"),
            section_text: s(a, "section_text"),
            scene_number: n(a, "scene_number"),
            location_name: s(a, "location_name"),
            location_description: s(a, "location_description"),
            cast_wall: s(a, "cast_wall"),
            cast_names: &list(a, "cast_names"),
            lyric: s(a, "lyric"),
            previous_entry: s(a, "previous_entry"),
            camera_flow: s(a, "camera_flow"),
            camera_motion_speed: n(a, "camera_motion_speed"),
            character_motion_speed: n(a, "character_motion_speed"),
            word_limit: n(a, "word_limit"),
            story_arc_seed: s(a, "story_arc_seed"),
        })),
        other => panic!("unknown builder {other}"),
    }
}

#[test]
fn the_arc_instructions_match_the_pack() {
    let all = cases();
    assert!(all.len() > 30, "{} cases", all.len());
    let mut failures = Vec::new();
    for (number, case) in all.iter().enumerate() {
        let name = case["fn"].as_str().unwrap();
        let got = run(name, &case["args"]);
        if got != case["return"] {
            failures.push(format!("case {number} {name}\n  got  {got}\n  want {}", case["return"]));
        }
    }
    assert!(failures.is_empty(), "{} of {} cases differ\n{}", failures.len(), all.len(), failures.iter().take(3).cloned().collect::<Vec<_>>().join("\n"));
}
