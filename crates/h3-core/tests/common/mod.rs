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

/// Equality where `1` and `1.0` match, since Python writes whole floats either way.
pub fn json_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| json_eq(p, q)),
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| json_eq(v, w)))
        }
        _ => a == b,
    }
}

/// Like `assert_eq!` on two JSON values, with number tolerance and a readable diff of the differing keys.
pub fn assert_json(got: &Value, want: &Value, context: &Value) {
    if json_eq(got, want) {
        return;
    }
    if let (Value::Object(g), Value::Object(w)) = (got, want) {
        for key in g.keys().chain(w.keys()) {
            let (a, b) = (g.get(key).unwrap_or(&Value::Null), w.get(key).unwrap_or(&Value::Null));
            if !json_eq(a, b) {
                eprintln!("key {key}: got {a} want {b}");
            }
        }
    }
    panic!("mismatch for {}", serde_json::to_string(context).unwrap_or_default().chars().take(600).collect::<String>());
}
