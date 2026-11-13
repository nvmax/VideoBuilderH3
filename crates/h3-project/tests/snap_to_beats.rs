//! `snap_to_beats` with `scope: "all"` (what the song pipeline sends) and `"all_starts"`. The pack's `timeline_snap` has the same tests in
//! `tests/test_agent_api_mutations.py`. Before the fix `all` did nothing and still answered `snapped: true`, and `all_starts` put every start
//! back where it was, because the scenes were settled once after all the snaps instead of one by one.

mod common;

use h3_project::scene_ops::snap_to_beats;
use h3_project::session::persist_session;
use serde_json::{json, Value};

use common::stamp;

/// Two scenes that meet at `boundary`, with beats at 0, 2, 4, 6 and 8 seconds.
fn project(boundary: f64, second: Value) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let mut second = second.as_object().cloned().unwrap();
    second.insert("id".into(), json!("b"));
    second.insert("start".into(), json!(boundary));
    second.insert("end".into(), json!(8.0));
    let session = json!({
        "project_name": "Song",
        "beat_markers": [0.0, 2.0, 4.0, 6.0, 8.0],
        "segments": [{"id": "a", "start": 0.0, "end": boundary}, Value::Object(second)],
    });
    persist_session(dir.path(), session.as_object().cloned().unwrap(), stamp(1)).unwrap();
    dir
}

fn boundary(dir: &std::path::Path) -> (f64, f64) {
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("vrgdg_builder_session.json")).unwrap()).unwrap();
    (saved["segments"][0]["end"].as_f64().unwrap(), saved["segments"][1]["start"].as_f64().unwrap())
}

#[test]
fn all_moves_every_scene_boundary_to_its_beat() {
    for scope in ["all", "all_starts"] {
        let dir = project(4.1, json!({}));
        let reply = snap_to_beats(dir.path(), scope, None, "start", &stamp(2)).unwrap();
        assert_eq!(reply["snapped"], true, "{scope}");
        assert_eq!(reply["moved"], 2, "{scope}");
        // The end of scene a and the start of scene b are one boundary and move together.
        assert_eq!(boundary(dir.path()), (4.0, 4.0), "{scope}");
    }
}

#[test]
fn all_leaves_a_boundary_with_no_beat_nearby() {
    let dir = project(4.4, json!({}));
    let reply = snap_to_beats(dir.path(), "all", None, "start", &stamp(2)).unwrap();
    assert_eq!(reply["moved"], 0);
    assert_eq!(boundary(dir.path()), (4.4, 4.4));
}

#[test]
fn all_keeps_a_scene_with_a_locked_video_where_it_is() {
    let dir = project(4.1, json!({"video_path": "video.mp4"}));
    let reply = snap_to_beats(dir.path(), "all", None, "start", &stamp(2)).unwrap();
    assert_eq!(reply["moved"], 0);
    assert_eq!(boundary(dir.path()), (4.1, 4.1));
}

#[test]
fn an_unknown_scope_is_refused_instead_of_doing_nothing() {
    let dir = project(4.1, json!({}));
    let refused = snap_to_beats(dir.path(), "everything", None, "start", &stamp(2));
    let message = format!("{:?}", refused.expect_err("an unknown scope is an error"));
    assert!(message.contains("Unknown snap scope"), "{message}");
    assert_eq!(boundary(dir.path()), (4.1, 4.1), "nothing was saved");
}

#[test]
fn edge_still_snaps_one_edge() {
    let dir = project(4.1, json!({}));
    snap_to_beats(dir.path(), "edge", Some("a"), "end", &stamp(2)).unwrap();
    assert_eq!(boundary(dir.path()).0, 4.0);
}
