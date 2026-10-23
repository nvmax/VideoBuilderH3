//! Project file operations against `fixtures/project`: every case replays what the pack's Python did on a temp folder.

mod common;

use std::collections::BTreeSet;
use std::fs;

use filetime::{set_file_mtime, FileTime};
use h3_project::assets::{prune_removed_scene_assets, renumber_after_insert, renumber_after_removal};
use h3_project::backups::{backup_session_file, list_backups, prune_session_backups, restore_backup};
use h3_project::journal::{recover_pending, TimelineJournal};
use h3_project::latents::{
    clear_dirty, continues_from_predecessor, delete_all_latents, delete_latent, latent_info, list_dirty, make_room_for_scene,
    mark_dirty, mark_successor_dirty, reindex_latents,
};
use h3_project::pyjson;
use h3_project::scaffold::new_project;
use h3_project::takes::{self, activate_for_video, archive_latent, attach_video, list_takes, rename_video, video_fingerprint};
use h3_project::{load_session, save_session, SaveInput};
use serde_json::{json, Value};

use common::{assert_tree, cases, rel_pairs, seed, seed_file, snapshot, stamp};

#[test]
fn json_text_matches_python() {
    for c in cases("json_dumps") {
        let value = pyjson::parse(c["input"].as_str().unwrap()).unwrap();
        assert_eq!(json!(pyjson::dumps_file(&value)), c["output"], "{c}");
    }
}

#[test]
fn session_saves() {
    for c in cases("save_session") {
        let name = c["input"]["name"].as_str().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("proj");
        fs::create_dir_all(&root).unwrap();
        if !c["input"]["existing"].is_null() {
            fs::write(root.join("vrgdg_builder_session.json"), pyjson::dumps_file(&c["input"]["existing"])).unwrap();
        }
        let session = c["input"]["session"].as_object().unwrap().clone();
        let outcome = save_session(&root, SaveInput { session, audio_path: String::new(), now: stamp(1) }).unwrap();
        assert_eq!(json!(outcome.stale), c["stale"], "{name}");
        assert_eq!(json!(outcome.revision), c["revision"], "{name}");

        let saved: Value = pyjson::parse(&fs::read_to_string(root.join("vrgdg_builder_session.json")).unwrap()).unwrap();
        let mut saved_map = saved.as_object().unwrap().clone();
        let order: Vec<String> = saved_map.keys().cloned().collect();
        assert_eq!(json!(order), c["saved_key_order"], "{name}: key order");
        assert_eq!(saved_map.contains_key("updated"), c["has_updated"].as_bool().unwrap(), "{name}");
        saved_map.remove("updated");
        if let Some(folder) = saved_map.get("project_folder") {
            let shown = folder.as_str().unwrap().to_string();
            assert!(std::path::Path::new(&shown).ends_with("proj"), "{name}: project_folder {shown}");
            saved_map.insert("project_folder".into(), json!("<root>"));
        }
        common_json_eq(&Value::Object(saved_map), &c["saved_session"], name);
        assert_eq!(fs::read_to_string(root.join("vrgdg_builder_session.json")).unwrap(), pyjson::dumps_file(&saved), "{name}: bytes");

        let after = snapshot(&root);
        let backups = after.keys().filter(|k| k.starts_with("session_backups/") && k.ends_with(".json")).count();
        assert_eq!(json!(backups), c["backup_count"], "{name}: backups");
        assert_eq!(after.get("builder_segments.srt").cloned().flatten().map(Value::from).unwrap_or(Value::Null), c["srt"], "{name}: srt");
        let derived: BTreeSet<&String> = after
            .iter()
            .filter(|(k, v)| v.is_some() && *k != "vrgdg_builder_session.json" && !k.starts_with("session_backups/"))
            .map(|(k, _)| k)
            .collect();
        let want: BTreeSet<&String> = c["derived"].as_object().unwrap().keys().collect();
        assert_eq!(derived, want, "{name}: derived files");
        for (path, text) in c["derived"].as_object().unwrap() {
            assert_eq!(after[path].as_deref(), text.as_str(), "{name}: {path}");
        }
        let folders: Vec<&String> = after.keys().filter(|k| k.ends_with('/') && k.matches('/').count() == 1).collect();
        let want_folders: Vec<&str> = c["folders_created"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(folders.iter().map(|s| s.as_str()).collect::<Vec<_>>(), want_folders, "{name}: folders");
    }
}

/// Numbers compare by value (`1` and `1.0`), everything else exactly.
fn common_json_eq(got: &Value, want: &Value, name: &str) {
    fn eq(a: &Value, b: &Value) -> bool {
        match (a, b) {
            (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
            (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| eq(p, q)),
            (Value::Object(x), Value::Object(y)) => x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| eq(v, w))),
            _ => a == b,
        }
    }
    assert!(eq(got, want), "{name}: session\n got {got}\nwant {want}");
}

#[test]
fn loading_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("proj");
    fs::create_dir_all(&root).unwrap();
    // A byte order mark and CRLF line endings, as older saves have.
    let text = "\u{feff}{\r\n  \"builder_save_revision\": 4,\r\n  \"segments\": [{\"timeline_note\": \"\"}, {\"timeline_note\": \"kept\"}]\r\n}\r\n";
    fs::write(root.join("vrgdg_builder_session.json"), text).unwrap();
    fs::write(root.join("SceneNotes.json"), "{\"SceneNote1\": \"from file\", \"SceneNote2\": \"ignored\"}").unwrap();
    let loaded = load_session(&root).unwrap();
    assert_eq!(loaded.revision, 4);
    assert_eq!(loaded.session["revision"], 4);
    assert_eq!(loaded.session["segments"][0]["timeline_note"], "from file");
    assert_eq!(loaded.session["segments"][1]["timeline_note"], "kept");
    assert!(load_session(&dir.path().join("missing")).is_err());
}

#[test]
fn session_backups_prune_and_undo() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("session_backups");
    fs::create_dir_all(&folder).unwrap();
    for i in 0..30 {
        let path = folder.join(format!("vrgdg_builder_session_20260101_{i:06}.json"));
        fs::write(&path, "{}").unwrap();
        set_file_mtime(&path, FileTime::from_unix_time(1000 + i, 0)).unwrap();
    }
    fs::write(folder.join("other.txt"), "x").unwrap();
    prune_session_backups(dir.path(), 25);
    let mut kept: Vec<String> = fs::read_dir(&folder).unwrap().filter_map(Result::ok).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    kept.sort();
    let want = &cases("prune_backups")[0]["kept"];
    assert_eq!(json!(kept), *want);

    // Undo: restoring a backup keeps the replaced session as a backup and raises the revision.
    let project = tempfile::tempdir().unwrap();
    let session = project.path().join("vrgdg_builder_session.json");
    fs::write(&session, pyjson::dumps_file(&json!({"revision": 3, "marker": "old"}))).unwrap();
    let old = backup_session_file(project.path(), &stamp(1)).unwrap().unwrap();
    fs::write(&session, pyjson::dumps_file(&json!({"revision": 9, "marker": "new"}))).unwrap();
    restore_backup(project.path(), &old, &stamp(2)).unwrap();
    let restored: Value = pyjson::parse(&fs::read_to_string(&session).unwrap()).unwrap();
    assert_eq!(restored["marker"], "old");
    assert_eq!(restored["revision"], 10);
    assert_eq!(list_backups(project.path()).len(), 2);
}

#[test]
fn new_project_folders() {
    let c = &cases("new_project")[0];
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Song");
    let made = new_project(&root).unwrap();
    assert_eq!(made, root);
    assert_tree(&snapshot(&root), &c["tree"], "new_project");
    // The same name again gets a numbered folder.
    assert_eq!(new_project(&root).unwrap(), dir.path().join("Song_002"));
}

#[test]
fn scene_asset_renumbering() {
    let all = cases("renumber");
    assert_eq!(all.len(), 10);
    for c in all {
        let i = &c["input"];
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("proj");
        seed(&root, &i["tree"]);
        let scene = i["scene"].as_u64().unwrap() as u32;
        let renamed = match i["op"].as_str().unwrap() {
            "insert" => renumber_after_insert(&root, scene).unwrap(),
            _ => renumber_after_removal(&root, scene, &stamp(7).compact).unwrap(),
        };
        let context = format!("{} {}", i["op"], scene);
        assert_eq!(json!(rel_pairs(&root, &renamed)), c["renamed"], "{context}: renamed");
        assert_tree(&snapshot(&root), &c["tree_after"], &context);
    }
}

#[test]
fn removed_scene_archive_pruning() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..24 {
        let folder = dir.path().join("removed_scene_assets").join(format!("scene_{i:04}_20260101_000000"));
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("f.txt"), "x").unwrap();
        set_file_mtime(&folder, FileTime::from_unix_time(2000 + i, 0)).unwrap();
    }
    prune_removed_scene_assets(dir.path(), 20);
    let mut kept: Vec<String> = fs::read_dir(dir.path().join("removed_scene_assets")).unwrap().filter_map(Result::ok).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    kept.sort();
    assert_eq!(json!(kept), cases("prune_removed")[0]["kept"]);
}

#[test]
fn latent_bookkeeping() {
    for c in cases("latent_ops") {
        let i = &c["input"];
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("proj");
        let op = i["op"].as_str().unwrap();
        let mut returned = Value::Null;
        if op == "mark_successor_dirty" {
            seed(&root, &i["files"]);
            let got = mark_successor_dirty(&root, 1, "Scene 001 re-rendered", &stamp(3));
            assert_eq!(json!(got), c["returned"], "{}", i["label"]);
            assert_eq!(json!(continues_from_predecessor(&root, 2)), c["continues"], "{}", i["label"]);
        } else {
            seed(&root, &i["tree"]);
            let p = &i["params"];
            let scene = p["scene"].as_u64().unwrap_or(0) as u32;
            match op {
                "mark_dirty" => mark_dirty(&root, scene, p["reason"].as_str().unwrap(), &stamp(4)),
                "clear_dirty" => clear_dirty(&root, scene),
                "delete_latent" => returned = json!(delete_latent(&root, scene, p["reindex"].as_bool().unwrap())),
                "delete_all" => returned = json!(delete_all_latents(&root)),
                "make_room" => make_room_for_scene(&root, scene),
                "reindex" => reindex_latents(&root, scene),
                other => panic!("{other}"),
            }
            assert_eq!(returned, c["returned"], "{op}");
        }
        assert_tree(&snapshot(&root), &c["tree_after"], op);
        assert_eq!(json!(list_dirty(&root)), c["dirty"], "{op}: dirty list");
    }
}

#[test]
fn latent_file_info() {
    for c in cases("latent_info") {
        let i = &c["input"];
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("proj");
        fs::create_dir_all(&root).unwrap();
        seed(&root, &i["files"]);
        let scene = i["scene"].as_u64().unwrap_or(1) as u32;
        let info = latent_info(&root, scene).unwrap();
        let mut got = serde_json::to_value(&info).unwrap();
        got["path"] = json!(std::path::Path::new(info.path.as_str()).strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/"));
        let mut want = c["info"].clone();
        if !i["files"].as_object().unwrap().keys().any(|k| k.ends_with(".json")) || i["name"] == "bad_json" {
            got.as_object_mut().unwrap().remove("timestamp"); // file time, not stable
        }
        // The pack reports the fps and timestamp of an existing latent only.
        want.as_object_mut().unwrap().retain(|_, v| !v.is_null() || false);
        assert_eq!(got, want, "{}", i["name"]);
    }
}

fn run_take_steps(steps: &Value) -> (Vec<Value>, common::Tree) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("proj");
    fs::create_dir_all(&root).unwrap();
    let mut results = Vec::new();
    for step in steps.as_array().unwrap() {
        let now = stamp(step["t"].as_u64().unwrap_or(0));
        let scene = step["scene"].as_u64().unwrap_or(0) as u32;
        let path_of = |key: &str| root.join(step[key].as_str().unwrap());
        let result = match step["op"].as_str().unwrap() {
            "write" => {
                seed_file(&root, step["path"].as_str().unwrap(), step["content"].as_str().unwrap());
                Value::Null
            }
            "archive" => {
                let latents = root.join("latents");
                json!(archive_latent(&latents, scene, &latents.join(format!("scene_{scene:03}.latent")), &now).unwrap())
            }
            "attach" => json!(attach_video(&root, scene, &path_of("video"), &now).unwrap()),
            "activate" => {
                let video = step["video"].as_str().filter(|v| !v.is_empty()).map(|v| root.join(v));
                activate_for_video(&root, scene, video.as_deref()).unwrap()
            }
            "list" => json!(list_takes(&root, scene)),
            "rename_video" => {
                rename_video(&root, scene, &video_fingerprint(&path_of("old")), &video_fingerprint(&path_of("new"))).unwrap();
                Value::Null
            }
            "fingerprint" => json!(video_fingerprint(&path_of("video"))),
            other => panic!("{other}"),
        };
        results.push(result);
    }
    (results, snapshot(&root))
}

#[test]
fn per_take_latents() {
    for c in cases("latent_takes") {
        let name = c["input"]["name"].as_str().unwrap();
        let (results, tree) = run_take_steps(&c["input"]["steps"]);
        assert_eq!(json!(results), c["results"], "{name}: results");
        assert_tree(&tree, &c["tree_after"], name);
    }
}

#[test]
fn take_folder_helpers() {
    for c in cases("take_folders") {
        let i = &c["input"];
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("proj");
        seed(&root, &i["tree"]);
        let dst = dir.path().join("dst");
        fs::create_dir_all(dst.join("latents")).unwrap();
        let latents = root.join("latents");
        let p = &i["params"];
        match i["op"].as_str().unwrap() {
            "shift" => takes::shift_takes(&latents, p["first"].as_u64().unwrap() as u32, p["delta"].as_i64().unwrap() as i32),
            "delete_scene" => takes::delete_scene_takes(&latents, p["scene"].as_u64().unwrap() as u32),
            "delete_all" => takes::delete_all_takes(&latents),
            _ => h3_project::latents::copy_latents_folder(&root, &dst),
        }
        assert_tree(&snapshot(&root), &c["tree_after"], i["op"].as_str().unwrap());
        assert_tree(&snapshot(&dst), &c["dst_after"], "dst");
    }
}

#[test]
fn timeline_journal() {
    for c in cases("journal") {
        let name = c["input"]["name"].as_str().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("proj");
        fs::create_dir_all(&root).unwrap();
        let mut journal: Option<TimelineJournal> = None;
        let mut results = Vec::new();
        for step in c["input"]["steps"].as_array().unwrap() {
            let now = stamp(step["t"].as_u64().unwrap_or(0));
            match step["op"].as_str().unwrap() {
                "write" | "overwrite_session" => {
                    let path = step["path"].as_str().unwrap_or("vrgdg_builder_session.json");
                    seed_file(&root, path, step["content"].as_str().unwrap());
                }
                "start" => {
                    let j = TimelineJournal::new(&root, step["name"].as_str().unwrap(), now.millis());
                    j.start(now.epoch).unwrap();
                    journal = Some(j);
                }
                "rename" => {
                    let (from, to) = (root.join(step["from"].as_str().unwrap()), root.join(step["to"].as_str().unwrap()));
                    fs::rename(&from, &to).unwrap();
                    journal.as_mut().unwrap().record_renames(&[(from, to)], now.epoch).unwrap();
                }
                "commit" => journal.as_ref().unwrap().commit(),
                "rollback" => journal.as_ref().unwrap().rollback(),
                "recover" => {
                    results.push(json!(recover_pending(&root)));
                    continue;
                }
                other => panic!("{other}"),
            }
            results.push(Value::Null);
        }
        assert_eq!(json!(results), c["results"], "{name}: results");
        assert_tree(&snapshot(&root), &c["tree_after"], name);
    }
}
