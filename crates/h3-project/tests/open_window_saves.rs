//! An open Video Builder window numbers its next save one above its last. After one edit through the API (an agent, the MCP server) that save has to be refused
//! as stale. If it were taken for the next save, the window's older snapshot would overwrite the edit.

mod common;

use h3_project::session::{persist_session, save_session, SaveInput};
use serde_json::{json, Value};

use common::stamp;

fn window_save(folder: &std::path::Path, number: i64, scenes: usize) -> h3_project::SaveOutcome {
    let segments: Vec<Value> = (0..scenes).map(|i| json!({"id": format!("s{i}"), "start": i as f64, "end": i as f64 + 1.0})).collect();
    let session = json!({"project_name": "Song", "builder_save_revision": number, "segments": segments}).as_object().cloned().unwrap();
    save_session(folder, SaveInput { session, audio_path: String::new(), now: stamp(1) }).unwrap()
}

fn read(folder: &std::path::Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(folder.join("vrgdg_builder_session.json")).unwrap()).unwrap()
}

#[test]
fn the_windows_next_save_after_one_api_edit_is_stale_and_the_edit_survives() {
    let dir = tempfile::tempdir().unwrap();
    // The window loaded the project and saved it as save 7. Its next save is 8.
    assert!(!window_save(dir.path(), 7, 2).stale);

    // One edit through the API adds a scene.
    let mut session = read(dir.path()).as_object().cloned().unwrap();
    session.get_mut("segments").unwrap().as_array_mut().unwrap().push(json!({"id": "agent", "start": 2.0, "end": 3.0}));
    persist_session(dir.path(), session, stamp(2)).unwrap();
    assert_eq!(read(dir.path())["segments"].as_array().unwrap().len(), 3);

    let refused = window_save(dir.path(), 8, 2);
    assert!(refused.stale, "the window's next save is older than the edit");
    assert_eq!(read(dir.path())["segments"].as_array().unwrap().len(), 3, "the edit survives the window's autosave");
}

#[test]
fn the_window_saves_again_once_it_has_reloaded_and_a_second_edit_is_refused_too() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!window_save(dir.path(), 7, 1).stale);
    for _ in 0..2 {
        let session = read(dir.path()).as_object().cloned().unwrap();
        persist_session(dir.path(), session, stamp(2)).unwrap();
    }
    assert!(window_save(dir.path(), 8, 1).stale);
    assert!(window_save(dir.path(), 9, 1).stale);
    // A window that reloaded takes the saved number as its own, and numbers its next save one above it.
    let saved = read(dir.path())["builder_save_revision"].as_i64().unwrap();
    assert!(!window_save(dir.path(), saved + 1, 1).stale);
}
