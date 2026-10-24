//! The Storyboard file against what the pack's own functions did in the same flows (`fixtures/story/disk.json`, made by
//! `tools/make_storyboard_disk_fixtures.py`): the replies and the files each flow leaves in the project.

use std::fs;
use std::path::{Path, PathBuf};

use h3_core::pyval::Obj;
use h3_project::storyboard::{export_prompts, import_reference_image, load_storyboard, save_storyboard, StoryboardError};
use h3_project::Stamp;
use serde_json::{json, Value};

fn fixture() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/story/disk.json");
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn is_iso(text: &str) -> bool {
    let b = text.as_bytes();
    b.len() == 19 && b.iter().enumerate().all(|(i, c)| match i {
        4 | 7 => *c == b'-',
        10 => *c == b'T',
        13 | 16 => *c == b':',
        _ => c.is_ascii_digit(),
    })
}

/// Paths in one form (slashes, the folder as `<TMP>`) and timestamps as `<T>`.
fn normalize(value: &Value, root: &str) -> Value {
    match value {
        Value::String(s) => {
            let text = s.replace('\\', "/").replace(root, "<TMP>");
            Value::String(if is_iso(&text) { "<T>".to_string() } else { text })
        }
        Value::Array(items) => Value::Array(items.iter().map(|v| normalize(v, root)).collect()),
        Value::Object(map) => Value::Object(map.iter().map(|(k, v)| (k.replace('\\', "/").replace(root, "<TMP>"), normalize(v, root))).collect()),
        other => other.clone(),
    }
}

fn snapshot(root: &Path) -> Value {
    fn walk(dir: &Path, base: &Path, out: &mut Obj) {
        let mut entries: Vec<_> = fs::read_dir(dir).unwrap().flatten().collect();
        entries.sort_by_key(|e| e.path());
        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, base, out);
                continue;
            }
            let relative = path.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
            let bytes = fs::read(&path).unwrap();
            let value = match path.extension().and_then(|e| e.to_str()) {
                Some("json") => serde_json::from_slice(&bytes).unwrap(),
                Some("txt") => json!(String::from_utf8(bytes).unwrap()),
                _ => json!({"bytes": bytes.len()}),
            };
            out.insert(relative, value);
        }
    }
    let mut files = Obj::new();
    walk(root, root, &mut files);
    Value::Object(files)
}

#[test]
fn the_storyboard_file_behaves_like_the_packs() {
    let stamp = Stamp { compact: "<T14>".into(), iso: "<T>".into(), epoch: 0.0 };
    for flow in fixture()["flows"].as_array().unwrap() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_string_lossy().replace('\\', "/");
        let name = &flow["name"];
        let steps = flow["steps"].as_array().unwrap();
        let results = flow["results"].as_array().unwrap();
        for (number, (step, want)) in steps.iter().zip(results).enumerate() {
            let request = serde_json::from_str::<Value>(&step["request"].to_string().replace("<TMP>", &root)).unwrap();
            let payload = request.as_object().unwrap();
            let got = match step["fn"].as_str().unwrap() {
                "load" => load_storyboard(payload, &stamp).map(|v| json!({"return": v})).map_err(StoryboardError::Failed),
                "save" => save_storyboard(payload, &stamp).map(|v| json!({"return": v})),
                "export" => export_prompts(payload, &stamp).map(|v| json!({"return": v})),
                "import_image" => import_reference_image(payload, &stamp).map(|v| json!({"return": v})).map_err(StoryboardError::Failed),
                other => panic!("unknown step {other}"),
            };
            let got = match got {
                Ok(value) => value,
                Err(StoryboardError::Conflict { current_revision, message }) => json!({"conflict": true, "error": message, "current_revision": current_revision}),
                Err(StoryboardError::Failed(message)) => json!({"error": message}),
            };
            assert_eq!(normalize(&got, &root), *want, "{name} step {number} ({})", step["fn"]);
        }
        assert_eq!(normalize(&snapshot(dir.path()), &root), flow["files"], "{name}: files");
    }
}
