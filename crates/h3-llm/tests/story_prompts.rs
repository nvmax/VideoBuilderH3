//! The Story Mode instruction builders against the pack's own text for the same inputs (`fixtures/story/prompts.json`, made by
//! `tools/make_story_prompt_fixtures.py`).

use std::fs;
use std::path::PathBuf;

use h3_llm::story_prompts::*;
use serde_json::{json, Value};

fn cases() -> Vec<Value> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/story/prompts.json");
    serde_json::from_str::<Value>(&fs::read_to_string(path).unwrap()).unwrap()["cases"].as_array().cloned().unwrap()
}

fn s<'a>(args: &'a Value, key: &str) -> &'a str {
    args.get(key).and_then(Value::as_str).unwrap_or("")
}

fn n(args: &Value, key: &str) -> i64 {
    args.get(key).and_then(Value::as_i64).unwrap_or(0)
}

fn b(args: &Value, key: &str) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn template(args: &Value) -> Option<&str> {
    args.get("template").and_then(Value::as_str)
}

fn run(name: &str, a: &Value) -> Value {
    match name {
        "story_mode_lyric_guidance" => json!(story_mode_lyric_guidance(a.get("strength"))),
        "story_book_size" => {
            let (acts, paragraphs) = story_book_size(n(a, "scene_count"), n(a, "act_count"));
            json!([acts, paragraphs])
        }
        "story_cast_rule" => json!(story_cast_rule(s(a, "cast_text"), b(a, "allow_new_cast"))),
        "story_medium_rule" => json!(story_medium_rule(s(a, "video_style"), s(a, "lead_name"))),
        "user_places_block" => json!(user_places_block(s(a, "places_text"))),
        "made_places_block" => json!(made_places_block(s(a, "made_places_text"))),
        "story_book_instruction" => json!(story_book_instruction(&BookInput {
            premise: s(a, "premise"),
            cast_text: s(a, "cast_text"),
            tone_notes: s(a, "tone_notes"),
            scene_count: n(a, "scene_count"),
            song_seconds: a["song_seconds"].as_f64().unwrap_or(0.0),
            lyric_guidance: s(a, "lyric_guidance"),
            lyrics: s(a, "lyrics"),
            previous_book: s(a, "previous_book"),
            seed: s(a, "seed"),
            allow_new_cast: b(a, "allow_new_cast"),
            video_style: if a.get("video_style").is_some() { s(a, "video_style") } else { "music_video" },
            lead_name: s(a, "lead_name"),
            template: template(a),
            act_count: n(a, "act_count"),
            places_text: s(a, "places_text"),
        })),
        "story_chapter_rewrite_instruction" => json!(story_chapter_rewrite_instruction(&ChapterInput {
            book_text: s(a, "book_text"),
            chapter_number: n(a, "chapter_number"),
            chapter_text: s(a, "chapter_text"),
            direction: s(a, "direction"),
            cast_text: s(a, "cast_text"),
            lyric_guidance: s(a, "lyric_guidance"),
            allow_new_cast: b(a, "allow_new_cast"),
            video_style: if a.get("video_style").is_some() { s(a, "video_style") } else { "music_video" },
            lead_name: s(a, "lead_name"),
            template: template(a),
            places_text: s(a, "places_text"),
        })),
        "story_split_instruction" => {
            let starts: Vec<i64> = a["fixed_starts"].as_array().unwrap().iter().map(|v| v.as_i64().unwrap()).collect();
            json!(story_split_instruction(s(a, "numbered_paragraphs"), &starts, n(a, "scene_count")))
        }
        "story_remove_people_instruction" => {
            let names: Vec<String> = a["names"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
            json!(story_remove_people_instruction(s(a, "story_text"), s(a, "cast_text"), &names))
        }
        "story_cast_scan_instruction" => json!(story_cast_scan_instruction(s(a, "book_text"), s(a, "known_names"))),
        "story_acts_instruction" => json!(story_acts_instruction(&ActsInput {
            chapter_number: n(a, "chapter_number"),
            chapter_title: s(a, "chapter_title"),
            setting_name: s(a, "setting_name"),
            chapter_text: s(a, "chapter_text"),
            scenes_text: s(a, "scenes_text"),
            cast_text: s(a, "cast_text"),
            notes_text: s(a, "notes_text"),
            target_acts: n(a, "target_acts"),
            template: template(a),
            places_text: s(a, "places_text"),
            made_places_text: s(a, "made_places_text"),
        })),
        "story_scene_plan_instruction" => json!(story_scene_plan_instruction(&PlanInput {
            act_title: s(a, "act_title"),
            act_summary: s(a, "act_summary"),
            setting_name: s(a, "setting_name"),
            chapter_text: s(a, "chapter_text"),
            scenes_text: s(a, "scenes_text"),
            cast_text: s(a, "cast_text"),
            spot_min_scenes: n(a, "spot_min_scenes"),
            previous_spot: s(a, "previous_spot"),
            lyric_guidance: s(a, "lyric_guidance"),
            known_spots: s(a, "known_spots"),
            video_style: if a.get("video_style").is_some() { s(a, "video_style") } else { "music_video" },
            act_cast: s(a, "act_cast"),
            main_name: s(a, "main_name"),
            template: template(a),
            places_text: s(a, "places_text"),
            made_places_text: s(a, "made_places_text"),
        })),
        "story_act_summary_instruction" => json!(story_act_summary_instruction(s(a, "act_title"), s(a, "setting_name"), s(a, "scenes_text"), s(a, "user_text"), s(a, "cast_text"), s(a, "old_summary"))),
        "story_setting_looks_instruction" => json!(story_setting_looks_instruction(s(a, "settings_text"), s(a, "book_summary"), s(a, "style_theme"), s(a, "cast_text"), template(a), s(a, "places_text"))),
        "story_spots_instruction" => json!(story_spots_instruction(s(a, "setting_name"), s(a, "setting_look"), s(a, "spots_text"), s(a, "style_theme"), template(a))),
        "story_place_list_instruction" => json!(story_place_list_instruction(s(a, "settings_text"), s(a, "user_places_text"), s(a, "style_theme"))),
        "story_revise_scene_instruction" => json!(story_revise_scene_instruction(&ReviseInput {
            book_text: s(a, "book_text"),
            chapter_number: n(a, "chapter_number"),
            chapter_title: s(a, "chapter_title"),
            setting_name: s(a, "setting_name"),
            chapter_text: s(a, "chapter_text"),
            act_title: s(a, "act_title"),
            act_summary: s(a, "act_summary"),
            act_scenes_text: s(a, "act_scenes_text"),
            scene_number: n(a, "scene_number"),
            scene_cast: s(a, "scene_cast"),
            user_note: s(a, "user_note"),
            cast_text: s(a, "cast_text"),
            lyric_guidance: s(a, "lyric_guidance"),
            template: template(a),
        })),
        other => panic!("unknown builder {other}"),
    }
}

#[test]
fn the_story_instructions_match_the_pack() {
    let all = cases();
    assert!(all.len() > 70, "{} cases", all.len());
    for (number, case) in all.iter().enumerate() {
        let got = run(case["fn"].as_str().unwrap(), &case["args"]);
        assert_eq!(got, case["return"], "case {number} {} {}", case["fn"], case["args"]);
    }
}
