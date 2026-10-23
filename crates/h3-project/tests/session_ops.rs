//! Session edits: scene fields, prompts, lyrics, references and projects.

mod common;

use std::fs;
use std::path::Path;

use common::stamp;
use h3_project::session_ops::*;
use h3_project::{load_session, scene_ops::OpError};
use serde_json::{json, Value};

fn project() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let (folder, revision) = create_project(dir.path(), "My Song!", &stamp(1)).unwrap();
    assert_eq!(revision, 1);
    // Two scenes and two cards.
    let mut session = load_session(&folder).unwrap().session;
    session.insert("segments".into(), json!([
        {"id": "a", "start": 0.0, "end": 4.0, "label": "Scene 1"},
        {"id": "b", "start": 4.0, "end": 8.0, "label": "Scene 2", "minimax_h3_prompt": "old"}
    ]));
    session.insert("flux_reference_builder".into(), json!({
        "subjects": [{"id": "s1", "name": "Ava", "image": {}}],
        "locations": [{"id": "l1", "name": "Roof"}]
    }));
    h3_project::persist_session(&folder, session, stamp(2)).unwrap();
    (dir, folder)
}

fn obj(v: Value) -> h3_core::pyval::Obj {
    v.as_object().unwrap().clone()
}

fn validation(r: Result<Value, OpError>) -> String {
    match r {
        Err(OpError::Validation(m)) => m,
        other => panic!("expected a validation error, got {other:?}"),
    }
}

#[test]
fn a_new_project_has_the_scaffold_and_a_session() {
    let dir = tempfile::tempdir().unwrap();
    let (folder, _) = create_project(dir.path(), "My Song!", &stamp(1)).unwrap();
    assert_eq!(folder.file_name().unwrap(), "My Song");
    assert!(folder.join("project_context").is_dir() && folder.join("latents").is_dir());
    let session = load_session(&folder).unwrap().session;
    assert_eq!(session["project_name"], "My Song!");
    assert_eq!(session["video_engine"], "minimax_h3");
    // The same name again gets its own folder.
    let (second, _) = create_project(dir.path(), "My Song!", &stamp(1)).unwrap();
    assert_ne!(folder, second);
    assert!(create_project(dir.path(), "  ", &stamp(1)).is_err());
    delete_project(&folder).unwrap();
    assert!(!folder.exists());
    assert!(delete_project(dir.path()).is_err(), "only a project folder can be deleted");
}

#[test]
fn scene_fields_are_saved_with_the_builders_keys() {
    let (_dir, folder) = project();
    let patch = obj(json!({"label": " Opening ", "lyrics": "[instrumental]", "director_note": "slow", "video_prompt": "A man walks.", "image_prompt": "a still"}));
    let result = patch_scene(&folder, "a", &patch, None, &stamp(3)).unwrap();
    let scene = &result["scene"];
    assert_eq!(scene["label"], "Opening");
    assert_eq!(scene["lyric_text"], "[instrumental]");
    assert_eq!(scene["lyric_no_lip_sync"], true, "an instrumental lyric turns lip sync off");
    assert_eq!(scene["timeline_note"], "slow");
    assert_eq!(scene["minimax_h3_prompt"], "A man walks.");
    assert_eq!(scene["minimax_h3_prompt_origin"], "manual");
    assert_eq!(scene["t2i_prompt"], "a still");
    // A changed prompt marks the scene's latent dirty.
    assert!(folder.join("latents/scene_001.dirty").is_file());
    assert_eq!(load_session(&folder).unwrap().session["segments"][0]["label"], "Opening");

    // Timing edits keep the neighbours from overlapping.
    patch_scene(&folder, "a", &obj(json!({"end": 5.0})), None, &stamp(4)).unwrap();
    let segs = load_session(&folder).unwrap().session["segments"].clone();
    assert_eq!((segs[0]["end"].as_f64(), segs[1]["start"].as_f64()), (Some(5.0), Some(5.0)));
}

#[test]
fn bad_scene_patches_are_refused_without_saving() {
    let (_dir, folder) = project();
    let revision = load_session(&folder).unwrap().revision;
    assert!(validation(patch_scene(&folder, "a", &obj(json!({"bogus": 1})), None, &stamp(3))).starts_with("Unsupported scene field: bogus."));
    assert!(validation(patch_scene(&folder, "a", &obj(json!({"performance_mode": "dancing"})), None, &stamp(3))).contains("must be one of"));
    assert!(validation(patch_scene(&folder, "a", &obj(json!({"trigger_phrase": "x"})), None, &stamp(3))).contains("Storyboard"));
    assert!(validation(patch_scene(&folder, "a", &obj(json!({"minimax_h3_settings": {"nope": 1}})), None, &stamp(3))).contains("minimax_h3_settings.nope"));
    assert!(validation(patch_scene(&folder, "a", &obj(json!({"subject_ids": ["zz"]})), None, &stamp(3))).starts_with("Unknown subject id: zz. Known: s1."));
    assert!(matches!(patch_scene(&folder, "zz", &obj(json!({"label": "x"})), None, &stamp(3)), Err(OpError::SceneNotFound(_))));
    assert!(matches!(patch_scene(&folder, "a", &obj(json!({"label": "x"})), Some(revision + 5), &stamp(3)), Err(OpError::Conflict { .. })));
    assert_eq!(load_session(&folder).unwrap().revision, revision, "nothing was saved");
}

#[test]
fn references_are_mapped_to_scenes_and_cleaned_on_delete() {
    let (_dir, folder) = project();
    patch_scene(&folder, "a", &obj(json!({"subject_ids": ["s1"], "location_id": "l1"})), None, &stamp(3)).unwrap();
    let refs = load_session(&folder).unwrap().session["flux_reference_builder"].clone();
    assert_eq!(refs["subject_scene_map"]["a"], json!(["s1"]));
    assert_eq!(refs["scene_map"]["a"], "l1");
    assert_eq!((refs["use_subject_reference"].as_bool(), refs["use_location_references"].as_bool()), (Some(true), Some(true)));

    update_scene_mapping(&folder, &obj(json!({"subjects": {"b": ["s1"]}, "locations": {"b": "l1"}})), None, &stamp(4)).unwrap();
    let refs = load_session(&folder).unwrap().session["flux_reference_builder"].clone();
    assert_eq!(refs["subject_scene_map"]["b"], json!(["s1"]));

    // A card update keeps the keys it was not sent.
    upsert_reference(&folder, "subjects", "s1", &obj(json!({"description": "singer"})), None, &stamp(5)).unwrap();
    let card = load_session(&folder).unwrap().session["flux_reference_builder"]["subjects"][0].clone();
    assert_eq!((card["name"].as_str(), card["description"].as_str()), (Some("Ava"), Some("singer")));
    upsert_reference(&folder, "locations", "l2", &obj(json!({"name": "Alley"})), None, &stamp(5)).unwrap();

    let deleted = delete_reference(&folder, "subjects", "s1", None, &stamp(6)).unwrap();
    assert_eq!(deleted["deleted"], true);
    let refs = load_session(&folder).unwrap().session["flux_reference_builder"].clone();
    assert_eq!(refs["subject_scene_map"]["a"], json!([]));
    delete_reference(&folder, "locations", "l1", None, &stamp(7)).unwrap();
    let refs = load_session(&folder).unwrap().session["flux_reference_builder"].clone();
    assert!(refs["scene_map"].get("a").is_none());
    assert!(matches!(delete_reference(&folder, "things", "x", None, &stamp(8)), Err(OpError::Validation(_))));
}

#[test]
fn prompts_and_lyrics_are_saved() {
    let (_dir, folder) = project();
    let r = set_scene_prompt(&folder, "b", "minimax_h3_prompt", "New prompt", "agent", None, &stamp(3)).unwrap();
    assert_eq!(r["origin"], "agent");
    let session = load_session(&folder).unwrap().session;
    assert_eq!((session["segments"][1]["minimax_h3_prompt"].as_str(), session["segments"][1]["minimax_h3_prompt_origin"].as_str()), (Some("New prompt"), Some("agent")));
    assert!(matches!(set_scene_prompt(&folder, "b", "label", "x", "agent", None, &stamp(3)), Err(OpError::Validation(_))));

    set_lyrics(&folder, Some("line one\nline two"), None, &stamp(4)).unwrap();
    assert_eq!(fs::read_to_string(Path::new(&folder).join("project_context/full_lyrics.txt")).unwrap().trim_end(), "line one\nline two");
    assert_eq!(load_session(&folder).unwrap().session["canonical_lyrics"], "line one\nline two");
}
