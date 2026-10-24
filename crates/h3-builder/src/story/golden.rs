//! The Story Mode steps against what the pack's own steps asked and returned (`tools/make_story_book_fixtures.py`, run with a recorder in
//! place of the LLM).

use h3_core::pyval::Obj;

use super::beat::build_scene_beat;
use super::book::{convert_story_arc, draft_story_book, remove_story_people, rewrite_story_chapter, scan_story_cast, split_pasted_story};
use super::places::build_story_locations;
use super::plan::{build_story_acts, build_story_scene_plan, rewrite_act_summary};
use super::revise::revise_story_from_scene;
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
