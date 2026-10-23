//! Scene operations against `fixtures/scene_ops`: each case replays what the pack's mutation did to a project folder.

mod common;

use std::fs;
use std::path::Path;

use common::{snapshot, stamp, Tree};
use h3_project::scene_ops::*;
use serde_json::{json, Value};

fn json_escaped(text: &str) -> String {
    serde_json::to_string(text).unwrap().trim_matches('"').to_string()
}

fn expand(text: &str, root: &Path, escaped: bool) -> String {
    let shown = root.display().to_string();
    text.replace("<root>", &if escaped { json_escaped(&shown) } else { shown })
}

fn collapse_text(text: &str, root: &Path) -> String {
    let raw = root.display().to_string();
    let posix = raw.replace('\\', "/");
    text.replace(&json_escaped(&raw), "<root>").replace(&raw, "<root>").replace(&posix, "<root>")
}

fn collapse(tree: &Tree, root: &Path) -> Tree {
    tree.iter().map(|(k, v)| (k.clone(), v.as_ref().map(|t| collapse_text(t, root)))).collect()
}

fn run(name: &str, project: &Path) -> Result<Value, OpError> {
    let now = stamp(7);
    let mut n = 0;
    let mut id = || {
        n += 1;
        format!("seg_new{n}")
    };
    let p = project;
    match name {
        "create_append" => create_scene(p, "append", None, 5.0, "", "tp", "ip", "nt", None, &now, &mut id),
        "create_before_s2" => create_scene(p, "before", Some("s2"), 3.0, "Intro", "", "", "", None, &now, &mut id),
        "create_after_s1_by_number" => create_scene(p, "after", Some("1"), 2.5, "", "", "", "", None, &now, &mut id),
        "create_missing_ref" => create_scene(p, "after", Some("zz"), 2.5, "", "", "", "", None, &now, &mut id),
        "create_conflict" => create_scene(p, "append", None, 4.0, "", "", "", "", Some(2), &now, &mut id),
        "delete_s2_ripple" => delete_scene(p, "s2", true, None, &now),
        "delete_s3_no_ripple" => delete_scene(p, "s3", false, None, &now),
        "delete_missing" => delete_scene(p, "nope", true, None, &now),
        "split_s1_clear" => split_scene(p, "s1", 2.5, true, None, &now, &mut id),
        "split_s3_keep_media" => split_scene(p, "s3", 10.0, false, None, &now, &mut id),
        "split_locked" => split_scene(p, "s2", 6.0, true, None, &now, &mut id),
        "split_out_of_range" => split_scene(p, "s1", 4.0, true, None, &now, &mut id),
        "merge_s1_next" => merge_scenes(p, "s1", "next", None, &now),
        "merge_s3_previous_locked" => merge_scenes(p, "s3", "previous", None, &now),
        "merge_s3_next_locked" => merge_scenes(p, "s3", "next", None, &now),
        "merge_bad_direction" => merge_scenes(p, "s1", "up", None, &now),
        "merge_no_neighbor" => merge_scenes(p, "s1", "previous", None, &now),
        "move_s1" => move_scene(p, "s1", 1.0, false, None, &now),
        "move_s3_ripple" => move_scene(p, "s3", 9.5, true, None, &now),
        "resize_s1_duration" => resize_scene(p, "s1", Some(3.0), None, false, None, &now),
        "resize_s3_end_ripple" => resize_scene(p, "s3", None, Some(15.0), true, None, &now),
        "bulk_replace_durations" => timeline_bulk(p, "3\n4 hello there\n5", "durations", "replace", 0.0, &now, &mut id),
        "bulk_replace_carry" => timeline_bulk(p, "0 - 6\n6 - 16", "ranges", "replace", 0.0, &now, &mut id),
        "bulk_append_markers" => timeline_bulk(p, "0 a\n3 b\n6", "markers", "append", 20.0, &now, &mut id),
        "bulk_append_default" => timeline_bulk(p, "2\n2", "durations", "append", 0.0, &now, &mut id),
        "bulk_bad" => timeline_bulk(p, "oops", "durations", "replace", 0.0, &now, &mut id),
        "bulk_bad_action" => timeline_bulk(p, "2", "durations", "merge", 0.0, &now, &mut id),
        "close_gaps" => close_gaps(p, &now),
        other => panic!("unknown case {other}"),
    }
}

fn is_plain_json(path: &str) -> bool {
    path.ends_with(".json") && !path.starts_with("session_backups") && !path.ends_with("index.json") && !path.ends_with(".latent.json") && !path.ends_with(".timeline_journal.json")
}

#[test]
fn scene_operations_match_the_pack() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/scene_ops/ops.json");
    let fixture: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("proj");
        fs::create_dir_all(&root).unwrap();
        for (rel, content) in case["before"].as_object().unwrap() {
            let target = root.join(rel.trim_end_matches('/'));
            match content {
                Value::String(text) if !rel.ends_with('/') => {
                    fs::create_dir_all(target.parent().unwrap()).unwrap();
                    fs::write(&target, expand(text, &root, rel.ends_with(".json"))).unwrap();
                }
                _ => fs::create_dir_all(&target).unwrap(),
            }
        }
        let result = run(name, &root);
        match (&result, case.get("error")) {
            (Err(e), Some(want)) => {
                let kind = match e {
                    OpError::SceneNotFound(_) => "SceneNotFoundError",
                    OpError::Validation(_) => "ValidationError",
                    OpError::Conflict { .. } => "RevisionConflictError",
                    OpError::Io(_) => panic!("{name}: {e}"),
                };
                if kind == "ValidationError" || kind == "RevisionConflictError" {
                    // A bad bulk line is a plain ValueError in the pack, a validation error here.
                    assert_eq!(json!(e.to_string()), want["message"], "{name}: message");
                }
                if want["type"] != "ValueError" {
                    assert_eq!(json!(kind), want["type"], "{name}");
                }
            }
            (Ok(got), None) => {
                assert_eq!(got["revision"], case["result"]["revision"], "{name}: revision");
                if let Some(renamed) = case["result"].get("renamed") {
                    let normal = |v: &Value| -> Vec<Vec<String>> {
                        v.as_array().unwrap().iter().map(|p| p.as_array().unwrap().iter().map(|s| collapse_text(s.as_str().unwrap(), &root)).collect()).collect()
                    };
                    assert_eq!(normal(&got["renamed"]), normal(renamed), "{name}: renamed");
                }
            }
            (got, want) => panic!("{name}: {got:?} vs {want:?}"),
        }
        let after = collapse(&snapshot(&root), &root);
        let want = case["after"].as_object().unwrap();
        assert_eq!(after.keys().collect::<Vec<_>>(), want.keys().collect::<Vec<_>>(), "{name}: file list");
        for (path, expected) in want {
            match (&after[path], expected) {
                (None, Value::Null) => {}
                (Some(text), Value::String(e)) if is_plain_json(path) => {
                    // JSON files compare as values, so `1` and `1.0` are alike, and then by key order.
                    let a: Value = serde_json::from_str(text).unwrap();
                    let b: Value = serde_json::from_str(e).unwrap();
                    assert_eq!(a, b, "{name}: {path}");
                    let keys = |v: &Value| -> Vec<String> { v.as_object().map(|m| m.keys().cloned().collect()).unwrap_or_default() };
                    assert_eq!(keys(&a), keys(&b), "{name}: {path}: key order");
                    if path == "vrgdg_builder_session.json" {
                        let seg_keys = |v: &Value| -> Vec<Vec<String>> { v["segments"].as_array().unwrap().iter().map(keys).collect() };
                        assert_eq!(seg_keys(&a), seg_keys(&b), "{name}: scene key order");
                    }
                }
                (Some(text), Value::String(e)) => {
                    if path.contains(".dirty") || path.ends_with(".timeline_journal.json") {
                        continue;
                    }
                    assert_eq!(text, e, "{name}: {path}");
                }
                other => panic!("{name}: {path}: {other:?}"),
            }
        }
    }
}
