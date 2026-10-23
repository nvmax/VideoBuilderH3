//! "New seeds" for a scene: fresh values in the project's settings, or in the scene's when they are locked.

use h3_comfy::ComfyDirs;
use h3_media::Tools;
use h3_pipeline::{randomize_seeds, LiveEnv, SEED_FIELDS};
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

#[test]
fn an_unknown_scene_is_an_error() {
    let dir = project(json!({"segments": [{"id": "a"}]}));
    assert!(randomize_seeds(&env(), dir.path(), "9").is_err());
}
