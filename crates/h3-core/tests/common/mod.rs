//! Helpers shared by the fixture tests. Fixtures are generated from the pack by `tools/make_all.py`.
#![allow(dead_code)]

use std::path::PathBuf;

use serde_json::Value;

/// `rustapp/fixtures`.
pub fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

/// Parse one fixture file, for example `load("core/timing.json")`.
pub fn load(relative: &str) -> Value {
    let path = fixtures_dir().join(relative);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The `cases` array of a table fixture (`core`, `scene` and `prompt` folders).
pub fn cases(relative: &str) -> Vec<Value> {
    load(relative)["cases"].as_array().unwrap_or_else(|| panic!("{relative} has no cases")).clone()
}

/// Every `*.json` file name in a fixtures sub folder, sorted.
pub fn files_in(folder: &str) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(fixtures_dir().join(folder))
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".json"))
        .collect();
    names.sort();
    names
}
