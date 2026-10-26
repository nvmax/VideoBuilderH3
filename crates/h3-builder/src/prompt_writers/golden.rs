//! The prompt writers against what the pack's own writers asked and returned for the same requests
//! (`tools/make_image_writer_fixtures.py` and `tools/make_reference_writer_fixtures.py`, run with a recorder in place of the LLM).

use serde_json::json;

use super::agent::builder_agent_reply;
use super::concepts::generate_concept_prompts;
use super::enhance::enhance_minimax_h3_prompt;
use super::gemma4::generate as generate_gemma4;
use super::image_prompts::{analyze_story_references, edit_image_prompt, generate_flux_klein_prompt, generate_nb_image_prompt, generate_t2i};
use super::motion_notes::generate_motion_notes;
use super::speech::{craft_dialogue, generate_voice_description};
use super::video_prompt::generate_t2v;
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
            "enhance" => enhance_minimax_h3_prompt(llm, payload).await,
            "motion" => generate_motion_notes(llm, payload).await,
            "t2v" => generate_t2v(llm, payload).await,
            "gemma4" => generate_gemma4(llm, payload).await,
            "agent" => builder_agent_reply(llm, payload).await,
            "voice" => generate_voice_description(llm, payload).await,
            "craft" => craft_dialogue(llm, payload).await,
            other => panic!("unknown function {other}"),
        }
    })
}

// Its fixtures hold Windows paths, so it only runs on Windows.
#[cfg(windows)]
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

// Its fixtures hold Windows paths, so it only runs on Windows.
#[cfg(windows)]
#[tokio::test]
async fn the_prompt_enhancer_matches_the_pack() {
    check_cases("llm/enhance_writer.json", 20, "", dispatch).await;
}

#[tokio::test]
async fn the_motion_note_writer_matches_the_pack() {
    check_cases("llm/motion_notes.json", 20, "", dispatch).await;
}

// Its fixtures hold Windows paths, so it only runs on Windows.
#[cfg(windows)]
#[tokio::test]
async fn the_h3_prompt_writer_matches_the_pack() {
    let dir = tempfile::tempdir().unwrap();
    for (name, body) in [("theme.txt", "\u{feff}Noir, neon, rain.\n"), ("story.txt", "A woman walks home.\n"), ("subject.txt", "The singer: Ann.\n")] {
        std::fs::write(dir.path().join(name), body).unwrap();
    }
    let custom = dir.path().join("project_context").join("custom_builder_instructions");
    std::fs::create_dir_all(custom.join("scenes").join("scene_2")).unwrap();
    std::fs::write(custom.join("minimax_h3_text_to_video.txt"), "Project H3 text to video instructions.\n").unwrap();
    std::fs::write(custom.join("scenes").join("scene_2").join("minimax_h3_text_to_video.txt"), "Scene 2 H3 text to video instructions.\n").unwrap();
    // Two pictures of the sizes the fixtures name. Only their sizes matter.
    image::RgbImage::new(64, 48).save(dir.path().join("ref.png")).unwrap();
    image::RgbImage::new(800, 700).save(dir.path().join("ref2.png")).unwrap();
    check_cases("llm/video_prompt.json", 40, &dir.path().display().to_string(), dispatch).await;
}

#[tokio::test]
async fn the_gemma4_drafts_match_the_pack() {
    let dir = tempfile::tempdir().unwrap();
    let custom = dir.path().join("project_context").join("custom_llm_instructions");
    std::fs::create_dir_all(&custom).unwrap();
    for (name, body) in [("style_theme.txt", "\u{feff}Custom style instructions.\n"), ("story_idea.txt", "   \n"), ("location_description_detail.txt", "Custom location detail instructions.")] {
        std::fs::write(custom.join(name), body).unwrap();
    }
    check_cases("llm/gemma4.json", 40, &dir.path().display().to_string(), dispatch).await;
}

#[tokio::test]
async fn the_builder_agent_matches_the_pack() {
    check_cases("llm/agent.json", 150, "", dispatch).await;
}

#[tokio::test]
async fn the_speaking_helpers_match_the_pack() {
    let dir = tempfile::tempdir().unwrap();
    let custom = dir.path().join("project_context").join("custom_builder_instructions");
    std::fs::create_dir_all(custom.join("scenes").join("scene_2")).unwrap();
    std::fs::write(custom.join("elevenlabs_dialogue.txt"), "Project dialogue instructions.\n").unwrap();
    std::fs::write(custom.join("scenes").join("scene_2").join("elevenlabs_dialogue.txt"), "Scene dialogue instructions.\n").unwrap();
    check_cases("speaking/writers.json", 30, &dir.path().display().to_string(), dispatch).await;
}
