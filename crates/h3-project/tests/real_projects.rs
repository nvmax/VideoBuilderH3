//! Round trip of real Video Builder projects. Local only: set `H3_REAL_PROJECTS` to a folder that holds project
//! folders (for example ComfyUI's `output`). Nothing from those projects is ever committed. Without the variable the
//! tests do nothing, so they pass on any machine.

use std::fs;
use std::path::PathBuf;

use h3_project::{pyjson, save_session, SaveInput, Stamp};
use serde_json::Value;

fn sessions() -> Vec<(String, PathBuf)> {
    let Ok(root) = std::env::var("H3_REAL_PROJECTS") else { return vec![] };
    let mut found: Vec<(String, PathBuf)> = fs::read_dir(root)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.join("vrgdg_builder_session.json").is_file())
        .map(|p| (p.file_name().unwrap().to_string_lossy().into_owned(), p.join("vrgdg_builder_session.json")))
        .collect();
    found.sort();
    found
}

/// Older saves have a byte order mark, CRLF line endings or no final newline. The text is the same.
fn normalized(text: &str) -> String {
    text.trim_start_matches('\u{feff}').replace("\r\n", "\n").trim_end().to_string()
}

#[test]
fn real_sessions_write_back_the_same_text() {
    let list = sessions();
    let mut identical = 0;
    for (index, (_, path)) in list.iter().enumerate() {
        let original = fs::read_to_string(path).unwrap();
        let value = pyjson::parse(&original).unwrap_or_else(|e| panic!("project #{index}: {e}"));
        let written = pyjson::dumps_file(&value);
        // Never print session text: it holds private lyrics, prompts and keys. Report where it differs, nothing more.
        let (a, b) = (normalized(&written), normalized(&original));
        if a != b {
            let at = a.chars().zip(b.chars()).position(|(x, y)| x != y).unwrap_or(a.len().min(b.len()));
            // Letters are masked so only the shape of the difference is shown (numbers, punctuation, escapes).
            let window = |t: &str| -> String {
                t.chars().skip(at.saturating_sub(40)).take(80).map(|c| if c.is_alphabetic() { 'x' } else { c }).collect()
            };
            panic!(
                "project #{index}: session text changed on a round trip at character {at} (lengths {} and {})
 written: {}
 original: {}",
                a.len(),
                b.len(),
                window(&a),
                window(&b)
            );
        }
        if written == original {
            identical += 1;
        }
    }
    eprintln!("{} real sessions, {} byte identical, the rest differ only in line endings or the final newline", list.len(), identical);
}

#[test]
fn real_sessions_save_with_only_the_revision_changed() {
    let list = sessions();
    for (index, (_, path)) in list.iter().enumerate() {
        let original: Value = pyjson::parse(&fs::read_to_string(path).unwrap()).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("copy");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("vrgdg_builder_session.json"), pyjson::dumps_file(&original)).unwrap();
        let session = original.as_object().unwrap().clone();
        let audio = session.get("audio_path").and_then(Value::as_str).unwrap_or("").to_string();
        let outcome = save_session(&root, SaveInput { session, audio_path: audio, now: Stamp::now() }).unwrap();
        assert!(!outcome.stale);
        let saved: Value = pyjson::parse(&fs::read_to_string(root.join("vrgdg_builder_session.json")).unwrap()).unwrap();
        let (a, b) = (original.as_object().unwrap(), saved.as_object().unwrap());
        let mut changed: Vec<&String> = b.keys().filter(|k| a.get(*k) != b.get(*k)).collect();
        changed.retain(|k| !matches!(k.as_str(), "revision" | "updated" | "project_folder"));
        assert!(changed.is_empty(), "project #{index}: keys changed by a save: {changed:?}");
        assert!(a.keys().all(|k| b.contains_key(k)), "project #{index}: a key was lost");
        let order_a: Vec<&String> = a.keys().collect();
        let order_b: Vec<&String> = b.keys().filter(|k| a.contains_key(*k)).collect();
        assert!(order_a == order_b, "project #{index}: key order changed");
    }
}
