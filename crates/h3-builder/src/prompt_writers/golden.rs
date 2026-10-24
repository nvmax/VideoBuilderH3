//! The prompt writers against what the pack's own writers asked and returned for the same requests
//! (`tools/make_image_writer_fixtures.py` and `tools/make_reference_writer_fixtures.py`, run with a recorder in place of the LLM).

use serde_json::json;

use super::concepts::generate_concept_prompts;
use super::image_prompts::{analyze_story_references, edit_image_prompt, generate_flux_klein_prompt, generate_nb_image_prompt, generate_t2i};
use super::references::{
    describe_reference_image, flux_reference_extract_locations, flux_reference_extract_subjects, flux_reference_location_map, flux_reference_zimage_prompt, lm_scout_locations, wizard_locations_from_lyrics,
};
use super::testing::Canned;
use crate::golden_support::{check_cases, Answer};
use h3_core::pyval::Obj;

/// The Rust writer a fixture's `fn` names.
fn dispatch<'a>(name: &'a str, llm: &'a Canned, payload: &'a Obj) -> Answer<'a> {
    Box::pin(async move {
        match name {
            "t2i" => generate_t2i(llm, payload).await,
            "edit" => edit_image_prompt(llm, payload).await,
            "flux" => generate_flux_klein_prompt(llm, payload).await,
            "nano" => generate_nb_image_prompt(llm, payload).await,
            "stories" => analyze_story_references(llm, payload).await,
            "concepts" => generate_concept_prompts(llm, payload).await,
            "describe" => describe_reference_image(llm, payload).await,
            "map" => flux_reference_location_map(llm, payload).await,
            "locs" => flux_reference_extract_locations(llm, payload).await,
            "wizard" => wizard_locations_from_lyrics(llm, payload).await,
            "scout" => lm_scout_locations(llm, payload).await,
            "subjects" => flux_reference_extract_subjects(llm, payload).await,
            "zimage" => flux_reference_zimage_prompt(llm, payload).await,
            other => panic!("unknown function {other}"),
        }
    })
}

#[tokio::test]
async fn the_image_writers_match_the_pack() {
    let dir = tempfile::tempdir().unwrap();
    for (name, body) in [("theme.txt", "Noir, neon, rain.\n"), ("story.txt", "A woman walks home.\n"), ("subject.txt", "The singer: Ann.\n")] {
        std::fs::write(dir.path().join(name), body).unwrap();
    }
    check_cases("llm/image_writers.json", 50, &dir.path().display().to_string(), dispatch).await;
}

#[tokio::test]
async fn the_reference_writers_match_the_pack() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("SceneNotes.json"), json!({"SceneNote1": " rain on the window ", "SceneNote3": "empty street", "Other": "x"}).to_string()).unwrap();
    check_cases("llm/reference_writers.json", 70, &dir.path().display().to_string(), dispatch).await;
}
