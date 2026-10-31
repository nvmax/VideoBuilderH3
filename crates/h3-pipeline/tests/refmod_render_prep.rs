//! What a render prepares for a scene of the RefMod pipeline. The Builder skips the Reference Builder picture check for it
//! (`video_render.mjs`) and so does the pack's agent render (`video_orchestrator.py`), so a render started through the API or MCP must too.

use h3_core::timing::TimingPlan;
use h3_graph::env::{GraphEnv, LatentFacts};
use h3_pipeline::{prepare_scene_render, Prepared, RenderError, RenderParams};
use h3_project::session::LoadedSession;
use serde_json::{json, Value};

/// prepare_scene_render only asks the environment about latents, and only for a continued scene.
struct Env;
impl GraphEnv for Env {
    fn is_file(&self, _: &str) -> bool { false }
    fn is_dir(&self, _: &str) -> bool { true }
    fn ensure_dir(&self, _: &str) {}
    fn node_info(&self, _: &str) -> Option<Value> { None }
    fn model_choices(&self, _: &str) -> Vec<String> { vec![] }
    fn output_dir(&self) -> String { String::new() }
    fn probe_duration(&self, _: &str) -> Result<f64, String> { Ok(10.0) }
    fn trim_audio(&self, _: &str, _: &str, _: u32, _: &TimingPlan) -> Result<Value, String> { Err("not used".into()) }
    fn latent_exists(&self, _: &str, _: u32) -> bool { false }
    fn latent_path(&self, _: &str, _: u32) -> String { String::new() }
    fn latent_facts(&self, _: &str, _: u32) -> LatentFacts { unimplemented!("not used") }
    fn pass1_latent_matches(&self, _: &str, _: u32) -> bool { false }
    fn random_seed(&self) -> u64 { 1 }
}

/// A saved RefMod on a Reference Builder card, with or without a picture on the card (a video RefMod has none).
fn card(picture: Option<&str>) -> Value {
    let mut card = json!({"id": "s1", "name": "Brad", "reference_type": "character", "source": "refmod",
        "refmod": {"name": "mods/s1", "kind": "video", "tokens": 100, "strength": 1.0}});
    if let Some(path) = picture {
        card["image"] = json!({"path": path});
    }
    card
}

/// A one-scene project with the card mapped to the scene.
fn project(pipeline: &str, card: Value) -> LoadedSession {
    let map = json!({"a": "s1"});
    let session = json!({
        "audio_path": "",
        "minimax_h3_settings": {"pipeline": pipeline, "video_mode": "reference_to_video"},
        "segments": [{"id": "a", "start": 0.0, "end": 4.0, "minimax_h3_prompt": "Brad walks."}],
        "flux_reference_builder": {"use_subject_reference": true, "subject_count": 1, "subjects": [card], "subject_scene_map": map},
    });
    LoadedSession {
        project_folder: std::env::temp_dir(),
        session_path: std::env::temp_dir().join("vrgdg_builder_session.json"),
        revision: 1,
        session: session.as_object().unwrap().clone(),
    }
}

fn prepare(loaded: &LoadedSession) -> Result<Prepared, RenderError> {
    prepare_scene_render(&Env, loaded, "1", &RenderParams { overrides: serde_json::Map::new(), timeout_hours: None })
}

#[test]
fn a_refmod_scene_whose_card_has_no_picture_prepares() {
    let prepared = prepare(&project("refmod", card(None))).expect("a RefMod scene needs no Reference Builder picture");
    assert_eq!(prepared.payload["pipeline"], "refmod");
    assert_eq!(prepared.payload["refmod_references"].as_array().map(Vec::len), Some(1));
    assert_eq!(prepared.payload["image_paths"], json!([]));
    assert_eq!(prepared.payload["video_references"], json!([]));
}

#[test]
fn a_refmod_scene_sends_no_images_and_does_not_check_the_card_picture() {
    // The card has a picture file that is gone. The RefMod pipeline sends no images, so that is not a reason to refuse.
    let prepared = prepare(&project("refmod", card(Some("C:/gone/brad.png")))).expect("the card picture is not used");
    assert_eq!(prepared.payload["image_paths"], json!([]));
}

#[test]
fn a_refmod_scene_with_only_a_muted_refmod_says_so() {
    let mut muted = card(None);
    muted["refmod"]["strength"] = json!(0.0);
    let error = prepare(&project("refmod", muted)).err().expect("a RefMod at strength 0 does not count").to_string();
    assert_eq!(error, "Scene 1 needs at least one RefMod. Pick a RefMod on a Reference Builder card and map it to the scene.");
}

#[test]
fn the_standard_pipeline_still_needs_a_reference_builder_picture() {
    let error = prepare(&project("standard", card(None))).err().expect("the standard pipeline needs a picture").to_string();
    assert_eq!(error, "Scene 1: This scene needs at least one ordered Reference Builder image.");
}
