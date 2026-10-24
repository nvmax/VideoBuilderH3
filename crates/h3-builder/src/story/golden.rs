//! The Story Mode steps against what the pack's own steps asked and returned (`tools/make_story_book_fixtures.py`, run with a recorder in
//! place of the LLM).

use h3_core::pyval::Obj;
use serde_json::json;

use super::arc::{build_story_layer_arc, build_story_layer_brief, ArcError};
use super::beat::build_scene_beat;
use super::book::{convert_story_arc, draft_story_book, remove_story_people, rewrite_story_chapter, scan_story_cast, split_pasted_story};
use super::places::build_story_locations;
use super::plan::{build_story_acts, build_story_scene_plan, rewrite_act_summary};
use super::revise::revise_story_from_scene;
use super::scene_prompts::build_storyboard_image_prompt;
use crate::golden_support::{check_cases, Answer};
use crate::prompt_writers::testing::Canned;

fn dispatch<'a>(name: &'a str, llm: &'a Canned, payload: &'a Obj) -> Answer<'a> {
    Box::pin(async move {
        match name {
            "draft" => draft_story_book(llm, payload).await,
            "chapter" => rewrite_story_chapter(llm, payload).await,
            "split" => split_pasted_story(llm, payload).await,
            "convert" => convert_story_arc(llm, payload).await,
            "remove" => remove_story_people(llm, payload).await,
            "scan" => scan_story_cast(llm, payload).await,
            "acts" => build_story_acts(llm, payload).await,
            "plan" => build_story_scene_plan(llm, payload).await,
            "summary" => rewrite_act_summary(llm, payload).await,
            "places" => build_story_locations(llm, payload).await,
            "revise" => revise_story_from_scene(llm, payload).await,
            "beat" => build_scene_beat(llm, payload).await,
            "image" => build_storyboard_image_prompt(llm, payload).await,
            "brief" => build_story_layer_brief(llm, payload).await,
            "arc" => match build_story_layer_arc(llm, payload).await {
                Ok(value) => Ok(value),
                Err(ArcError::Failed(message)) => Err(message),
                Err(ArcError::Format(found)) => Ok(json!({"__format_error__": {
                    "message": found.message, "runner": found.runner, "expected_sections": found.expected_sections, "raw_output": found.raw_output, "cleaned_output": found.cleaned_output,
                }})),
            },
            other => panic!("unknown step {other}"),
        }
    })
}

#[tokio::test]
async fn the_book_steps_match_the_pack() {
    check_cases("story/book_steps.json", 30, "", dispatch).await;
}

#[tokio::test]
async fn the_acts_and_the_scene_plan_match_the_pack() {
    check_cases("story/plan_steps.json", 30, "", dispatch).await;
}

#[tokio::test]
async fn the_places_and_the_scene_revision_match_the_pack() {
    check_cases("story/places_steps.json", 20, "", dispatch).await;
}

#[tokio::test]
async fn the_scene_story_beat_matches_the_pack() {
    check_cases("story/beat_steps.json", 25, "", dispatch).await;
}

#[tokio::test]
async fn the_storyboard_image_prompt_matches_the_pack() {
    check_cases("story/image_prompt.json", 15, "", dispatch).await;
}

#[tokio::test]
async fn the_story_brief_and_the_story_arc_match_the_pack() {
    let dir = tempfile::tempdir().unwrap();
    let context = dir.path().join("project_context");
    std::fs::create_dir_all(&context).unwrap();
    let lyrics = "\u{feff}[Intro]\n\n[Verse 1]\nwalking home alone\nthe street is empty\nI count the lights\n\n[Chorus]\noh oh oh\nnever going back\n\n[Verse 2]\nmorning comes\n\n[Chorus]\noh oh oh\n\n[Outro]\n";
    std::fs::write(context.join("full_lyrics.txt"), lyrics).unwrap();
    check_cases("story/arc_steps.json", 60, &dir.path().display().to_string(), dispatch).await;
}
