//! Project export as a portable zip. Port of `_prepare_builder_project_export` in `builder/project.py`.
//!
//! The package holds the whole project folder plus `vrgdg_project_package.json`. Files the session points at outside the
//! project are copied in under `portable_assets/` and the session in the zip points at them. API keys are blanked in the
//! zip's session. Unlike the Video Builder, the session on disk is left as it is.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

use crate::layout::session_path;
use crate::session::load_session;
use crate::session_ops::safe_project_name;

const PORTABLE: &[&str] = &[
    "png", "jpg", "jpeg", "webp", "gif", "bmp", "mp4", "mov", "mkv", "webm", "avi", "wav", "mp3", "m4a", "flac", "ogg", "srt", "txt", "json", "csv",
];
const ALREADY_COMPRESSED: &[&str] = &["mp4", "mov", "mkv", "webm", "avi", "mp3", "m4a", "flac", "ogg", "png", "jpg", "jpeg", "webp", "gif", "zip"];
const SECRET_KEYS: &[&str] = &["lm_studio_api_key", "llm_api_key_project", "own_server_api_key_project", "api_key", "own_server_key", "own_server_api_key"];

fn is_secret(key: &str) -> bool {
    SECRET_KEYS.contains(&key) || key.ends_with("_api_key") || key.ends_with("_api_key_project")
}

/// Blank API keys at the top level and inside the settings groups that may hold them.
pub fn redact_session_secrets(session: &mut Value) {
    let Some(map) = session.as_object_mut() else { return };
    for (key, value) in map.iter_mut() {
        if is_secret(key) {
            *value = json!("");
        }
    }
    for group in ["nb_image_settings", "llm_settings", "lm_studio_settings"] {
        if let Some(inner) = map.get_mut(group).and_then(Value::as_object_mut) {
            for (key, value) in inner.iter_mut() {
                if is_secret(key) {
                    *value = json!("");
                }
            }
        }
    }
}

fn clean(text: &str, limit: usize) -> String {
    let mut out = String::new();
    let mut last_bad = false;
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
            out.push(c);
            last_bad = false;
        } else if !last_bad {
            out.push('_');
            last_bad = true;
        }
    }
    let out = out.trim_matches(['.', '_']).to_string();
    let skip = out.chars().count().saturating_sub(limit);
    out.chars().skip(skip).collect()
}

struct Localizer<'a> {
    project: &'a Path,
    /// Source file, name inside `portable_assets/`.
    copied: HashMap<String, String>,
    order: Vec<(PathBuf, String)>,
}

impl Localizer<'_> {
    fn walk(&mut self, value: &mut Value, key_path: &str) {
        match value {
            Value::Object(map) => {
                for (key, item) in map.iter_mut() {
                    self.walk(item, &format!("{key_path}_{key}"));
                }
            }
            Value::Array(list) => {
                for (i, item) in list.iter_mut().enumerate() {
                    self.walk(item, &format!("{key_path}_{}", i + 1));
                }
            }
            Value::String(text) => {
                if let Some(replacement) = self.localize(text, key_path) {
                    *text = replacement;
                }
            }
            _ => {}
        }
    }

    fn localize(&mut self, text: &str, key_path: &str) -> Option<String> {
        let source = PathBuf::from(text.trim().trim_matches('"'));
        if !source.is_absolute() || !source.is_file() {
            return None;
        }
        if source.starts_with(self.project) {
            return None;
        }
        let ext = source.extension()?.to_string_lossy().to_lowercase();
        if !PORTABLE.contains(&ext.as_str()) {
            return None;
        }
        let key = source.to_string_lossy().to_lowercase();
        let name = if let Some(name) = self.copied.get(&key) {
            name.clone()
        } else {
            let base = clean(&source.file_name()?.to_string_lossy(), usize::MAX);
            let base = if base.is_empty() { format!("file.{ext}") } else { base };
            let key_part = clean(key_path, 80);
            let key_part = if key_part.is_empty() { "asset".to_string() } else { key_part };
            let name = format!("{:04}_{key_part}_{base}", self.copied.len() + 1);
            self.copied.insert(key, name.clone());
            self.order.push((source, name.clone()));
            name
        };
        Some(self.project.join("portable_assets").join(name).display().to_string())
    }
}

fn walk_files(dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            if entry.file_name() != "__pycache__" {
                walk_files(&path, out)?;
            }
        } else if path.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

fn relative(project: &Path, file: &Path) -> String {
    file.strip_prefix(project).unwrap_or(file).to_string_lossy().replace('\\', "/")
}

fn add_file<W: Write + io::Seek>(zip: &mut zip::ZipWriter<W>, name: &str, source: &Path) -> Result<(), String> {
    let ext = source.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let method = if ALREADY_COMPRESSED.contains(&ext.as_str()) { CompressionMethod::Stored } else { CompressionMethod::Deflated };
    let options = SimpleFileOptions::default().compression_method(method).large_file(true);
    zip.start_file(name, options).map_err(|e| e.to_string())?;
    let mut file = File::open(source).map_err(|e| format!("{}: {e}", source.display()))?;
    io::copy(&mut file, zip).map_err(|e| e.to_string())?;
    Ok(())
}

/// Package the project into `<out_dir>/<name>.vrgdg.zip`. Returns the zip path and its file name.
pub fn export_project(project: &Path, out_dir: &Path, created: f64) -> Result<(PathBuf, String), String> {
    let project = std::path::absolute(project).map_err(|e| e.to_string())?;
    if !session_path(&project).is_file() {
        return Err("The Builder project or its session file was not found.".into());
    }
    let loaded = load_session(&project).map_err(|e| e.to_string())?;
    let mut session = Value::Object(loaded.session);
    let mut localizer = Localizer { project: &project, copied: HashMap::new(), order: Vec::new() };
    localizer.walk(&mut session, "session");
    if let Some(map) = session.as_object_mut() {
        map.insert("project_folder".into(), json!(project.display().to_string()));
        map.insert("updated".into(), json!(created));
    }
    redact_session_secrets(&mut session);
    let session_bytes = crate::pyjson::dumps_file(&session).into_bytes();

    let project_name = safe_project_name(&project.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    let file_name = format!("{project_name}.vrgdg.zip");
    std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let target = out_dir.join(&file_name);
    let temp = out_dir.join(format!("{file_name}.tmp"));
    let result = (|| -> Result<(), String> {
        let mut zip = zip::ZipWriter::new(File::create(&temp).map_err(|e| e.to_string())?);
        let plain = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        zip.start_file("vrgdg_project_package.json", plain).map_err(|e| e.to_string())?;
        let manifest = json!({"format": "vrgdg_builder_project", "version": 1, "project_name": project_name, "created": created});
        zip.write_all(serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?.as_bytes()).map_err(|e| e.to_string())?;
        let mut files = Vec::new();
        walk_files(&project, &mut files).map_err(|e| e.to_string())?;
        let session_file = session_path(&project);
        for file in files {
            if file == session_file {
                zip.start_file(relative(&project, &file), plain).map_err(|e| e.to_string())?;
                zip.write_all(&session_bytes).map_err(|e| e.to_string())?;
            } else {
                add_file(&mut zip, &relative(&project, &file), &file)?;
            }
        }
        for (source, name) in &localizer.order {
            add_file(&mut zip, &format!("portable_assets/{name}"), source)?;
        }
        zip.finish().map_err(|e| e.to_string())?;
        Ok(())
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&temp);
        return Err(e);
    }
    std::fs::rename(&temp, &target).map_err(|e| e.to_string())?;
    Ok((target, file_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_blanked() {
        let mut v = json!({"api_key": "k", "x_api_key": "k", "keep": "v", "llm_settings": {"own_server_api_key": "k", "model": "m"}});
        redact_session_secrets(&mut v);
        assert_eq!(v, json!({"api_key": "", "x_api_key": "", "keep": "v", "llm_settings": {"own_server_api_key": "", "model": "m"}}));
    }
}
