//! "New seeds" for a scene: fresh values in the project's settings, or in the scene's when they are locked.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use h3_comfy::ComfyDirs;
use h3_media::Tools;
use h3_pipeline::{randomize_seeds, LiveEnv, SessionLock, SEED_FIELDS};
use h3_project::load_session;
use serde_json::{json, Value};

fn env() -> LiveEnv {
    let dirs = ComfyDirs { output: String::new(), input: String::new(), temp: String::new() };
    LiveEnv::new(Value::Null, dirs, Tools { ffmpeg: "ffmpeg".into(), ffprobe: "ffprobe".into() })
}

fn project(session: Value) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("vrgdg_builder_session.json"), serde_json::to_string(&session).unwrap()).unwrap();
    dir
}

fn assert_fresh(settings: &Value) {
    for field in SEED_FIELDS {
        let seed = settings[field].as_i64().unwrap_or_else(|| panic!("{field} missing"));
        assert!((1..=2_147_483_647).contains(&seed), "{field} = {seed}");
    }
}

#[test]
fn project_settings_get_new_seeds_and_the_save_bumps_the_revision() {
    let dir = project(json!({"revision": 2, "segments": [{"id": "a"}, {"id": "b"}], "minimax_h3_settings": {"render_pass": "single", "seed": 5}}));
    randomize_seeds(&env(), dir.path(), "b").unwrap();
    let loaded = load_session(dir.path()).unwrap();
    assert_eq!(loaded.revision, 3);
    assert_fresh(&loaded.session["minimax_h3_settings"]);
    assert_eq!(loaded.session["minimax_h3_settings"]["render_pass"], "single");
}

#[test]
fn a_scene_with_locked_settings_keeps_the_projects_seeds() {
    let dir = project(json!({
        "segments": [{"id": "a", "use_scene_minimax_h3_settings": true, "minimax_h3_settings": {"seed": 1}}],
        "minimax_h3_settings": {"seed": 5}
    }));
    randomize_seeds(&env(), dir.path(), "1").unwrap();
    let loaded = load_session(dir.path()).unwrap();
    assert_fresh(&loaded.session["segments"][0]["minimax_h3_settings"]);
    assert_eq!(loaded.session["minimax_h3_settings"]["seed"], 5);
}

/// A stand-in for the project edit lock: `taken` is true while somebody holds it.
fn spin_lock(taken: &Arc<AtomicBool>) -> SessionLock {
    struct Held(Arc<AtomicBool>);
    impl Drop for Held {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }
    let taken = Arc::clone(taken);
    Arc::new(move || {
        while taken.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
            std::thread::sleep(Duration::from_millis(5));
        }
        Box::new(Held(Arc::clone(&taken)))
    })
}

#[test]
fn new_seeds_wait_for_the_edit_lock_and_give_it_back() {
    let dir = project(json!({"revision": 2, "segments": [{"id": "a"}], "minimax_h3_settings": {"seed": 5}}));
    let taken = Arc::new(AtomicBool::new(false));
    let mut locked = env();
    locked.session_lock = Some(spin_lock(&taken));

    // Somebody else is editing the project.
    taken.store(true, Ordering::SeqCst);
    let path = dir.path().to_path_buf();
    let worker = std::thread::spawn(move || randomize_seeds(&locked, &path, "a"));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(load_session(dir.path()).unwrap().revision, 2, "nothing is saved while another edit holds the lock");
    assert!(!worker.is_finished());

    // The other edit is done.
    taken.store(false, Ordering::SeqCst);
    worker.join().unwrap().unwrap();
    assert_eq!(load_session(dir.path()).unwrap().revision, 3);
    assert!(!taken.load(Ordering::SeqCst), "the lock is given back after the save");
}

#[test]
fn an_unknown_scene_is_an_error() {
    let dir = project(json!({"segments": [{"id": "a"}]}));
    assert!(randomize_seeds(&env(), dir.path(), "9").is_err());
}
