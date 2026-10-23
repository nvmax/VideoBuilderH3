//! Helpers shared by the project fixture tests. Fixtures come from `tools/make_project_fixtures.py`.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use h3_project::Stamp;
use serde_json::Value;

pub fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

pub fn cases(name: &str) -> Vec<Value> {
    let path = fixtures_dir().join("project").join(format!("{name}.json"));
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let value: Value = serde_json::from_str(&text).unwrap();
    value["cases"].as_array().cloned().unwrap_or_default()
}

/// The frozen clock of step `t` in the generator: `BASE + t + 0.123` seconds.
pub fn stamp(t: u64) -> Stamp {
    Stamp {
        compact: format!("20260101_{:02}{:02}{:02}", t / 3600 % 24, t / 60 % 60, t % 60),
        iso: format!("2026-01-01T{:02}:{:02}:{:02}", t / 3600 % 24, t / 60 % 60, t % 60),
        epoch: (1_700_000_000.0 + t as f64) + 0.123,
    }
}

/// Create files from a path to text map. A path ending in `/` is an empty folder. `None` also means a folder.
pub fn seed(root: &Path, files: &Value) {
    for (rel, content) in files.as_object().unwrap() {
        let target = root.join(rel.trim_end_matches('/'));
        match content {
            Value::String(text) if !rel.ends_with('/') => {
                fs::create_dir_all(target.parent().unwrap()).unwrap();
                fs::write(&target, text).unwrap();
            }
            _ => fs::create_dir_all(&target).unwrap(),
        }
    }
}

pub fn seed_file(root: &Path, rel: &str, content: &str) {
    let target = root.join(rel);
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(target, content).unwrap();
}

pub type Tree = BTreeMap<String, Option<String>>;

/// Relative path (folders end in `/`) to text.
pub fn snapshot(root: &Path) -> Tree {
    fn walk(base: &Path, root: &Path, out: &mut Tree) {
        for entry in fs::read_dir(base).unwrap().filter_map(Result::ok) {
            let path = entry.path();
            let rel = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
            if path.is_dir() {
                out.insert(format!("{rel}/"), None);
                walk(&path, root, out);
            } else {
                out.insert(rel, Some(String::from_utf8(fs::read(&path).unwrap()).unwrap()));
            }
        }
    }
    let mut out = Tree::new();
    walk(root, root, &mut out);
    out
}

fn zero_stamps(mut value: Value) -> Value {
    if let Some(map) = value.as_object_mut() {
        for key in ["timestamp", "created_at", "updated_at"] {
            if map.contains_key(key) {
                map.insert(key.into(), Value::from(0));
            }
        }
    }
    value
}

/// Compare a snapshot with a fixture tree (`{path: text | null}`). Dirty flags and journals compare as JSON with
/// their time stamps zeroed.
pub fn assert_tree(got: &Tree, want: &Value, context: &str) {
    let want = want.as_object().unwrap();
    let got_keys: Vec<&String> = got.keys().collect();
    let want_keys: Vec<&String> = want.keys().collect();
    assert_eq!(got_keys, want_keys, "{context}: file list");
    for (path, expected) in want {
        let actual = &got[path];
        match (actual, expected) {
            (None, Value::Null) => {}
            (Some(text), Value::String(expected)) => {
                if path.ends_with(".dirty") || path.ends_with(".timeline_journal.json") {
                    let a = zero_stamps(serde_json::from_str(text).unwrap());
                    let b = zero_stamps(serde_json::from_str(expected).unwrap());
                    assert_eq!(a, b, "{context}: {path}");
                } else {
                    assert_eq!(text, expected, "{context}: {path}");
                }
            }
            other => panic!("{context}: {path}: {other:?}"),
        }
    }
}

pub fn rel_pairs(root: &Path, pairs: &[(PathBuf, PathBuf)]) -> Vec<Vec<String>> {
    let rel = |p: &Path| p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
    pairs.iter().map(|(a, b)| vec![rel(a), rel(b)]).collect()
}
