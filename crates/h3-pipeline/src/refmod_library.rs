//! The saved RefMods on disk, listed with their metadata without loading any tensors. Port of `minimax/refmod_library.py`.
//!
//! The folder a RefMod lives in is its type (`refmods/identity/darrel.safetensors` is an `identity` RefMod), so a mod's name
//! is its path below the RefMod folder without the extension (`identity/darrel`). A preview is `<name>.preview.png`.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use h3_core::pyval::{py_str, to_int, truthy};
use h3_graph::refmod::RefmodEntry;
use serde_json::{json, Value};

pub const PREVIEW_SUFFIX: &str = ".preview.png";
const SKIP_DIRS: [&str; 3] = ["graph_presets", ".git", "__pycache__"];

/// One readable visual RefMod.
#[derive(Debug, Clone)]
pub struct LibraryEntry {
    pub name: String,
    pub folder: String,
    pub kind_of_concept: String,
    pub kind: String,
    pub frames: i64,
    pub canvas: [i64; 2],
    pub tokens: i64,
    pub description: String,
    pub tags: Vec<String>,
    pub mode: String,
    pub has_preview: bool,
    pub path: PathBuf,
    pub size: u64,
    pub directory: PathBuf,
}

impl LibraryEntry {
    /// The entry as the API shows it: no file paths.
    pub fn public(&self) -> Value {
        json!({
            "name": self.name, "folder": self.folder, "type": self.kind_of_concept, "kind": self.kind, "frames": self.frames,
            "canvas": self.canvas, "tokens": self.tokens, "description": self.description, "tags": self.tags, "mode": self.mode,
            "has_preview": self.has_preview, "size": self.size,
        })
    }

    pub fn as_graph_entry(&self) -> RefmodEntry {
        RefmodEntry { name: self.name.clone(), kind: self.kind.clone(), tokens: self.tokens }
    }
}

/// The JSON header of a safetensors file. Reads only the header, never the tensors.
pub fn read_safetensors_header(path: &Path) -> Option<Value> {
    let mut file = fs::File::open(path).ok()?;
    let mut length = [0u8; 8];
    file.read_exact(&mut length).ok()?;
    let length = u64::from_le_bytes(length);
    if length > 64 * 1024 * 1024 {
        return None;
    }
    let mut header = vec![0u8; length as usize];
    file.read_exact(&mut header).ok()?;
    serde_json::from_slice(&header).ok()
}

fn meta_from_header(header: &Value) -> Option<Value> {
    let raw = header.get("__metadata__").filter(|v| truthy(v));
    for key in ["refmod_meta", "audio_refmod_meta"] {
        if let Some(text) = raw.and_then(|r| r.get(key)) {
            return text.as_str().and_then(|t| serde_json::from_str(t).ok());
        }
    }
    None
}

fn text_of(value: Option<&Value>) -> String {
    value.filter(|v| truthy(v)).map(|v| py_str(Some(v))).unwrap_or_default()
}

fn entry(name: &str, path: &Path, directory: &Path) -> Option<LibraryEntry> {
    let header = read_safetensors_header(path)?;
    let meta = meta_from_header(&header)?;
    let meta = meta.as_object()?;
    let kind = meta.get("kind").and_then(Value::as_str).filter(|k| matches!(*k, "image" | "video"))?.to_string();
    let int_of = |key: &str, default: i64| -> Option<i64> {
        match meta.get(key) {
            Some(v) if truthy(v) => to_int(v),
            _ => Some(default),
        }
    };
    let frames = int_of("latent_t", 1)?;
    let (height, width) = (int_of("latent_h", 0)?, int_of("latent_w", 0)?);
    let folder = name.split_once('/').map_or(String::new(), |(head, _)| head.to_string());
    let preview = path.with_file_name(format!("{}{PREVIEW_SUFFIX}", path.file_stem()?.to_string_lossy()));
    let concept = text_of(meta.get("concept_type"));
    Some(LibraryEntry {
        name: name.to_string(),
        folder: folder.clone(),
        kind_of_concept: if !concept.is_empty() { concept } else if !folder.is_empty() { folder } else { "generic".into() },
        kind,
        frames,
        canvas: [width * 16, height * 16],
        tokens: frames * height.div_euclid(2) * width.div_euclid(2),
        description: text_of(meta.get("description")),
        tags: meta.get("tags").filter(|v| truthy(v)).and_then(Value::as_array).map(|a| a.iter().map(|t| py_str(Some(t))).collect()).unwrap_or_default(),
        mode: text_of(meta.get("mode")),
        has_preview: preview.is_file(),
        path: path.to_path_buf(),
        size: fs::metadata(path).map_or(0, |m| m.len()),
        directory: directory.to_path_buf(),
    })
}

fn walk(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(read) = fs::read_dir(dir) else { return };
    let mut files: Vec<PathBuf> = Vec::new();
    let mut subdirs: Vec<PathBuf> = Vec::new();
    for item in read.flatten() {
        let path = item.path();
        if path.is_dir() {
            if !SKIP_DIRS.contains(&item.file_name().to_string_lossy().as_ref()) {
                subdirs.push(path);
            }
        } else {
            files.push(path);
        }
    }
    files.sort();
    subdirs.sort();
    found.extend(files.into_iter().filter(|f| f.extension().is_some_and(|e| e == "safetensors")));
    for sub in subdirs {
        walk(&sub, found);
    }
}

fn name_of(directory: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(directory).unwrap_or(path).with_extension("");
    relative.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/")
}

/// Every readable visual RefMod, optionally only those in one folder (type). Sorted by name.
pub fn list_refmods(dirs: &[PathBuf], folder: &str) -> Vec<LibraryEntry> {
    let mut seen: Vec<String> = Vec::new();
    let mut entries: Vec<LibraryEntry> = Vec::new();
    for directory in dirs {
        let mut files = Vec::new();
        walk(directory, &mut files);
        for path in files {
            let name = name_of(directory, &path);
            if seen.contains(&name) || (!folder.is_empty() && name.split('/').next() != Some(folder)) {
                continue;
            }
            if let Some(found) = entry(&name, &path, directory) {
                seen.push(name);
                entries.push(found);
            }
        }
    }
    entries.sort_by_key(|e| e.name.to_lowercase());
    entries
}

/// One RefMod by name. A name that could leave the RefMod folders finds nothing.
pub fn find_refmod(dirs: &[PathBuf], name: &str) -> Option<LibraryEntry> {
    let text = name.replace('\\', "/").trim().to_string();
    if text.is_empty() || text.starts_with('/') || text.contains(':') || text.split('/').any(|p| p == "..") {
        return None;
    }
    for directory in dirs {
        let path = directory.join(format!("{text}.safetensors"));
        if path.is_file() {
            return entry(&text, &path, directory);
        }
    }
    None
}
