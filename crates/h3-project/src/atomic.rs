//! Crash-safe writes. Rust twin of the pack's `core/atomic_write.py`.
//!
//! Content goes to a temporary sibling file, is flushed to disk, then replaces the target in one rename.
//! A crash leaves either the old file or the new file, never a partial one.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

use serde::Serialize;

/// Write `content` to `path` atomically, creating parent folders as needed.
pub fn atomic_write_text(path: &Path, content: &str) -> io::Result<()> {
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    fs::create_dir_all(parent)?;
    let mut tmp = tempfile::Builder::new()
        .prefix(".h3-")
        .suffix(".tmp")
        .tempfile_in(parent)?;
    tmp.write_all(content.as_bytes())?;
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

/// Serialize `value` as pretty JSON (2-space indent, UTF-8) and write it atomically.
pub fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let text = serde_json::to_string_pretty(value).map_err(io::Error::other)?;
    atomic_write_text(path, &text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("state.json");
        atomic_write_json(&path, &serde_json::json!({"a": 1})).unwrap();
        atomic_write_json(&path, &serde_json::json!({"a": 2})).unwrap();
        let back: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(back["a"], 2);
        let leftovers: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }
}
