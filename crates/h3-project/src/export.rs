//! Project export and import as a portable zip. Ports of `_prepare_builder_project_export` and
//! `_import_builder_project_zip` in `builder/project.py`.
//!
//! The package holds the whole project folder plus `vrgdg_project_package.json`. Files the session points at outside the
//! project are copied into the project's `portable_assets/` folder first and the session in the zip points at the copies.
//! API keys are blanked in the zip's session.
//!
//! Two things differ from the pack on purpose. The pack also writes this session over the project's own session file, and
//! when the session already names this folder its rebase blanks every path inside the project, which damages the project
//! it exports. Here the project's session file is left alone, and paths are rebased only when the session names another
//! folder (a moved or copied project). Files of the old folder that are also in this one are not packed twice.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use h3_core::pyval::{text_or_empty, to_int, truthy, Obj};
use serde_json::{json, Value};
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

use crate::atomic::atomic_write_json;
use crate::layout::{session_path, Stamp, SESSION_FILE};
use crate::project_copy::{adopt_existing_project_files, copy_file_if_exists, copy_session_assets_to_project, rebase_project_owned_paths};
use crate::rehydrate::{absolute, is_inside_folder};
use crate::scaffold::unique_folder_path;
use crate::session::read_session_file;
use crate::session_ops::safe_project_name;

const PORTABLE: &[&str] = &[
    "png", "jpg", "jpeg", "webp", "gif", "bmp", "mp4", "mov", "mkv", "webm", "avi", "wav", "mp3", "m4a", "flac", "ogg", "srt", "txt", "json", "csv",
];
const ALREADY_COMPRESSED: &[&str] = &["mp4", "mov", "mkv", "webm", "avi", "mp3", "m4a", "flac", "ogg", "png", "jpg", "jpeg", "webp", "gif", "zip"];
const SECRET_KEYS: &[&str] = &["lm_studio_api_key", "llm_api_key_project", "own_server_api_key_project", "api_key", "own_server_key", "own_server_api_key"];
const PACKAGE_FILE: &str = "vrgdg_project_package.json";
const MAX_UNPACKED_BYTES: u64 = 500 * 1024 * 1024 * 1024;

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

/// `re.sub(r"[^A-Za-z0-9_.-]+", "_", text).strip("._")`, keeping the last `limit` characters.
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

fn normcase(path: &Path) -> String {
    let text = path.display().to_string();
    if cfg!(windows) { text.replace('/', "\\").to_lowercase() } else { text }
}

fn same_folder(a: &Path, b: &Path) -> bool {
    normcase(&absolute(a)) == normcase(&absolute(b))
}

/// Copies files the session names outside the project into `portable_assets/` and rewrites the names.
struct Localizer<'a> {
    project: &'a Path,
    /// Normalized source path to the copy.
    copied: HashMap<String, String>,
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
        let abs = absolute(&source);
        if is_inside_folder(&abs, self.project) {
            return Some(abs.display().to_string());
        }
        let ext = source.extension()?.to_string_lossy().to_lowercase();
        if !PORTABLE.contains(&ext.as_str()) {
            return None;
        }
        let key = normcase(&abs);
        if let Some(done) = self.copied.get(&key) {
            return Some(done.clone());
        }
        let key_part = Some(clean(key_path, 80)).filter(|k| !k.is_empty()).unwrap_or_else(|| "asset".into());
        let base = Some(clean(&source.file_name()?.to_string_lossy(), usize::MAX)).filter(|b| !b.is_empty()).unwrap_or_else(|| format!("file.{ext}"));
        let destination = self.project.join("portable_assets").join(format!("{:04}_{key_part}_{base}", self.copied.len() + 1));
        let copied = copy_file_if_exists(&source.display().to_string(), &destination).ok().filter(|c| !c.is_empty())?;
        self.copied.insert(key, copied.clone());
        Some(copied)
    }
}

fn walk_files(dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    let mut entries: Vec<_> = fs::read_dir(dir)?.flatten().collect();
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
///
/// Files the session names outside the project are copied into the project first, the zip's session is written with every
/// path pointing into the project and with the API keys blanked, and then the whole folder is zipped.
pub fn export_project(project: &Path, out_dir: &Path, now: &Stamp) -> Result<(PathBuf, String), String> {
    let project = absolute(project);
    let session_file = session_path(&project);
    if !project.is_dir() || !session_file.is_file() {
        return Err("The Builder project or its session file was not found.".into());
    }
    let session = read_session_file(&session_file).map_err(|_| "The Builder project session is invalid.".to_string())?;
    let recorded = text_or_empty(session.get("project_folder"));
    let old_project = if recorded.is_empty() { project.display().to_string() } else { recorded };
    let mut session = Value::Object(session);
    let moved = !same_folder(Path::new(&old_project), &project);
    // A session that names another folder (the project was moved or copied): files that are here too are pointed at the
    // copies in this folder first, so they are not packed a second time as if they were outside.
    if moved {
        adopt_existing_project_files(&mut session, &old_project, &project.display().to_string());
    }
    // Pull any still-external scene assets into the project before packaging it.
    let mut scene_session = session.as_object().cloned().unwrap_or_default();
    copy_session_assets_to_project(&project, &mut scene_session, now).map_err(|e| e.to_string())?;
    let mut session = Value::Object(scene_session);
    let mut localizer = Localizer { project: &project, copied: HashMap::new() };
    localizer.walk(&mut session, "session");
    if moved {
        rebase_project_owned_paths(&project, &old_project, &mut session);
    }
    if let Some(map) = session.as_object_mut() {
        map.insert("project_folder".into(), json!(project.display().to_string()));
        map.insert("updated".into(), json!(now.epoch));
    }
    redact_session_secrets(&mut session);
    let session_bytes = crate::pyjson::dumps_file(&session).into_bytes();

    let project_name = safe_project_name(&project.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    let file_name = format!("{project_name}.vrgdg.zip");
    fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let target = out_dir.join(&file_name);
    let temp = out_dir.join(format!("{file_name}.tmp"));
    let result = (|| -> Result<(), String> {
        let mut zip = zip::ZipWriter::new(File::create(&temp).map_err(|e| e.to_string())?);
        let plain = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        zip.start_file(PACKAGE_FILE, plain).map_err(|e| e.to_string())?;
        let manifest = json!({"format": "vrgdg_builder_project", "version": 1, "project_name": project_name, "created": now.epoch});
        zip.write_all(serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?.as_bytes()).map_err(|e| e.to_string())?;
        let mut files = Vec::new();
        walk_files(&project, &mut files).map_err(|e| e.to_string())?;
        for file in files {
            if file == session_file {
                zip.start_file(relative(&project, &file), plain).map_err(|e| e.to_string())?;
                zip.write_all(&session_bytes).map_err(|e| e.to_string())?;
            } else {
                add_file(&mut zip, &relative(&project, &file), &file)?;
            }
        }
        zip.finish().map_err(|e| e.to_string())?;
        Ok(())
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(&temp);
        return Err(e);
    }
    fs::rename(&temp, &target).map_err(|e| e.to_string())?;
    Ok((target, file_name))
}

// -- import ---------------------------------------------------------------------------------------------------------------

/// `_safe_builder_zip_members`: refuse a package that could write outside the new project or that is not a project.
fn check_members<R: io::Read + io::Seek>(archive: &mut zip::ZipArchive<R>) -> Result<(), String> {
    if archive.is_empty() {
        return Err("The selected ZIP file is empty.".into());
    }
    let mut total: u64 = 0;
    let mut has_session = false;
    for index in 0..archive.len() {
        let member = archive.by_index_raw(index).map_err(|e| e.to_string())?;
        let name = member.name().to_string();
        let normalized = name.replace('\\', "/");
        let bytes = normalized.as_bytes();
        let drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
        if normalized.starts_with('/') || drive || normalized.split('/').any(|part| part == "..") {
            return Err(format!("Unsafe path in project ZIP: {name}"));
        }
        if member.unix_mode().is_some_and(|mode| mode & 0o170000 == 0o120000) {
            return Err(format!("Symbolic links are not allowed in project ZIPs: {name}"));
        }
        total = total.saturating_add(member.size());
        if member.size() > 1024 * 1024 * 1024 && member.compressed_size() > 0 && member.size() > member.compressed_size() * 1000 {
            return Err(format!("Suspicious compression ratio in project ZIP: {name}"));
        }
        has_session |= normalized.trim_matches('/') == SESSION_FILE;
    }
    if total > MAX_UNPACKED_BYTES {
        return Err("The uncompressed project is larger than the 500 GB safety limit.".into());
    }
    if !has_session {
        return Err(format!("This ZIP is not a portable Video Builder project ({SESSION_FILE} is missing)."));
    }
    Ok(())
}

fn extract(archive: &mut zip::ZipArchive<File>, target: &Path) -> Result<(), String> {
    let target_real = fs::canonicalize(target).map_err(|e| e.to_string())?;
    for index in 0..archive.len() {
        let mut member = archive.by_index(index).map_err(|e| e.to_string())?;
        let name = member.name().replace('\\', "/").trim_matches('/').to_string();
        if name.is_empty() || name == PACKAGE_FILE {
            continue;
        }
        let mut destination = target_real.clone();
        destination.extend(name.split('/').filter(|p| !p.is_empty()));
        if !destination.starts_with(&target_real) {
            return Err(format!("Unsafe path in project ZIP: {}", member.name()));
        }
        if member.is_dir() {
            fs::create_dir_all(&destination).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut output = File::create(&destination).map_err(|e| format!("{}: {e}", destination.display()))?;
        io::copy(&mut member, &mut output).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Unpack a project package into a new folder under `output_dir`. Returns the folder and the project name used.
pub fn import_project_zip(zip_path: &Path, output_dir: &Path, requested_name: &str, now: &Stamp) -> Result<(PathBuf, String), String> {
    let mut archive = zip::ZipArchive::new(File::open(zip_path).map_err(|e| e.to_string())?).map_err(|_| "The selected file is not a valid ZIP project package.".to_string())?;
    check_members(&mut archive)?;
    let manifest: Value = archive
        .by_name(PACKAGE_FILE)
        .ok()
        .and_then(|mut f| {
            let mut text = String::new();
            io::Read::read_to_string(&mut f, &mut text).ok()?;
            serde_json::from_str(&text).ok()
        })
        .unwrap_or(Value::Null);
    let file_name = zip_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let default_name = Some(text_or_empty(manifest.get("project_name"))).filter(|n| !n.is_empty()).unwrap_or_else(|| file_name.replace(".vrgdg.zip", "").replace(".zip", ""));
    let project_name = safe_project_name(if requested_name.is_empty() { &default_name } else { requested_name });
    fs::create_dir_all(output_dir).map_err(|e| e.to_string())?;
    let target = unique_folder_path(&absolute(&output_dir.join(&project_name))).map_err(|e| e.to_string())?;
    fs::create_dir_all(&target).map_err(|e| e.to_string())?;
    let mut finish = || -> Result<(), String> {
        extract(&mut archive, &target)?;
        let file = session_path(&target);
        let mut session: Obj = if file.is_file() { read_session_file(&file).unwrap_or_default() } else { Obj::new() };
        let old_folder = text_or_empty(session.get("project_folder")).trim().trim_matches('"').to_string();
        let mut value = Value::Object(session);
        rebase_project_owned_paths(&target, &old_folder, &mut value);
        session = value.as_object().cloned().unwrap_or_default();
        session.insert("project_folder".into(), json!(target.display().to_string()));
        session.insert("updated".into(), json!(now.epoch));
        let revision = session.get("revision").filter(|v| truthy(v)).and_then(to_int).unwrap_or(1);
        session.insert("revision".into(), json!(revision));
        atomic_write_json(&file, &Value::Object(session)).map_err(|e| e.to_string())
    };
    if let Err(e) = finish() {
        let _ = fs::remove_dir_all(&target);
        return Err(e);
    }
    Ok((target, project_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp() -> Stamp {
        Stamp { compact: "20260101_000000".into(), iso: "2026-01-01T00:00:00".into(), epoch: 1_767_225_600.0 }
    }

    fn write(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    #[test]
    fn secrets_are_blanked() {
        let mut v = json!({"api_key": "k", "x_api_key": "k", "keep": "v", "llm_settings": {"own_server_api_key": "k", "model": "m"}});
        redact_session_secrets(&mut v);
        assert_eq!(v, json!({"api_key": "", "x_api_key": "", "keep": "v", "llm_settings": {"own_server_api_key": "", "model": "m"}}));
    }

    #[test]
    fn a_project_survives_export_and_import() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("My Song");
        let outside = dir.path().join("outside").join("voice over.wav");
        write(&outside, "audio");
        write(&project.join("project_audio/song.wav"), "song");
        let session = json!({
            "project_folder": project.display().to_string(),
            "audio_path": project.join("project_audio/song.wav").display().to_string(),
            "api_key": "secret",
            "segments": [{"custom_audio_path": outside.display().to_string()}],
            "revision": 4
        });
        write(&session_path(&project), &serde_json::to_string(&session).unwrap());
        let (zip_path, name) = export_project(&project, &dir.path().join("exports"), &stamp()).unwrap();
        assert_eq!(name, "My Song.vrgdg.zip");
        let on_disk: Value = serde_json::from_str(&fs::read_to_string(session_path(&project)).unwrap()).unwrap();
        assert_eq!(on_disk["api_key"], "secret", "the project's own session is left alone");
        assert_eq!(on_disk["audio_path"], project.join("project_audio/song.wav").display().to_string());
        assert!(project.join("scene_audio").join("audio_0001.wav").is_file(), "the outside audio was pulled in");

        let output = dir.path().join("output");
        let (target, project_name) = import_project_zip(&zip_path, &output, "", &stamp()).unwrap();
        assert_eq!(project_name, "My Song");
        assert_eq!(target, absolute(&output.join("My Song")));
        let imported: Value = serde_json::from_str(&fs::read_to_string(session_path(&target)).unwrap()).unwrap();
        assert_eq!(imported["project_folder"], target.display().to_string());
        assert_eq!(imported["revision"], 4);
        assert_eq!(imported["api_key"], "", "the package carries no key");
        assert!(imported["segments"][0]["custom_audio_path"].as_str().unwrap().ends_with("audio_0001.wav"));
        assert_eq!(imported["audio_path"], target.join("project_audio").join("song.wav").display().to_string());
        assert!(target.join("project_audio/song.wav").is_file());
        assert!(!target.join(PACKAGE_FILE).exists());
        let (second, _) = import_project_zip(&zip_path, &output, "", &stamp()).unwrap();
        assert!(second.display().to_string().ends_with("My Song_002"));
    }

    #[test]
    fn a_moved_project_is_exported_without_copying_its_own_files_again() {
        let dir = tempfile::tempdir().unwrap();
        let (old, moved) = (dir.path().join("old"), dir.path().join("moved"));
        for folder in [&old, &moved] {
            write(&folder.join("project_audio/song.wav"), "song");
            write(&folder.join("rendered_scene_videos/video_0001-audio.mp4"), "v");
        }
        let session = json!({
            "project_folder": old.display().to_string(),
            "audio_path": old.join("project_audio/song.wav").display().to_string(),
            "segments": [{"video_path": old.join("rendered_scene_videos/video_0001-audio.mp4").display().to_string()}]
        });
        write(&session_path(&moved), &serde_json::to_string(&session).unwrap());
        let (zip_path, _) = export_project(&moved, &dir.path().join("exports"), &stamp()).unwrap();
        assert!(!moved.join("portable_assets").exists(), "files of the old folder were copied as if they were outside");
        let mut archive = zip::ZipArchive::new(File::open(&zip_path).unwrap()).unwrap();
        let mut text = String::new();
        io::Read::read_to_string(&mut archive.by_name(SESSION_FILE).unwrap(), &mut text).unwrap();
        let packed: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(packed["audio_path"], moved.join("project_audio").join("song.wav").display().to_string());
        assert_eq!(packed["project_folder"], moved.display().to_string());
    }

    #[test]
    fn unsafe_packages_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let make = |name: &str, entries: &[(&str, &str)]| {
            let path = dir.path().join(name);
            let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
            for (entry, content) in entries {
                zip.start_file(*entry, SimpleFileOptions::default()).unwrap();
                zip.write_all(content.as_bytes()).unwrap();
            }
            zip.finish().unwrap();
            path
        };
        let output = dir.path().join("output");
        let escape = make("escape.zip", &[(SESSION_FILE, "{}"), ("../evil.txt", "x")]);
        assert_eq!(import_project_zip(&escape, &output, "", &stamp()).unwrap_err(), "Unsafe path in project ZIP: ../evil.txt");
        let drive = make("drive.zip", &[(SESSION_FILE, "{}"), ("C:/evil.txt", "x")]);
        assert!(import_project_zip(&drive, &output, "", &stamp()).unwrap_err().starts_with("Unsafe path"));
        let not_project = make("other.zip", &[("notes.txt", "x")]);
        assert_eq!(import_project_zip(&not_project, &output, "", &stamp()).unwrap_err(), "This ZIP is not a portable Video Builder project (vrgdg_builder_session.json is missing).");
        let empty = make("empty.zip", &[]);
        assert_eq!(import_project_zip(&empty, &output, "", &stamp()).unwrap_err(), "The selected ZIP file is empty.");
        let plain = dir.path().join("plain.zip");
        fs::write(&plain, "not a zip").unwrap();
        assert_eq!(import_project_zip(&plain, &output, "", &stamp()).unwrap_err(), "The selected file is not a valid ZIP project package.");
        assert!(!dir.path().join("evil.txt").exists());
        assert!(!output.join("escape").exists(), "a refused package leaves no folder behind");
    }
}
