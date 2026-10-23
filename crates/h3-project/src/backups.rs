//! Session backups and undo. Every save copies the previous session to `session_backups/`, keeping the newest 25, which
//! is the pack's own history. Undo restores one of them. The restore first backs up the current session, so an undo can
//! itself be undone.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::layout::{session_path, Stamp};

pub const MAX_SESSION_BACKUPS: usize = 25;

pub fn backups_folder(project: &Path) -> PathBuf {
    project.join("session_backups")
}

fn modified(path: &Path) -> SystemTime {
    fs::metadata(path).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH)
}

fn is_backup_name(name: &str) -> bool {
    name.starts_with("vrgdg_builder_session_") && name.ends_with(".json")
}

/// Backup files, oldest first.
pub fn list_backups(project: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(backups_folder(project)) else { return vec![] };
    let mut found: Vec<(SystemTime, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter(|e| is_backup_name(&e.file_name().to_string_lossy()) && e.path().is_file())
        .map(|e| (modified(&e.path()), e.path()))
        .collect();
    found.sort_by_key(|(time, _)| *time);
    found.into_iter().map(|(_, path)| path).collect()
}

/// Keep the newest `max_keep` backups.
pub fn prune_session_backups(project: &Path, max_keep: usize) {
    let mut backups = list_backups(project);
    while backups.len() > max_keep {
        let _ = fs::remove_file(backups.remove(0));
    }
}

/// Copy the current session into `session_backups/`. Returns the copy, or `None` when there is no session yet.
pub fn backup_session_file(project: &Path, now: &Stamp) -> io::Result<Option<PathBuf>> {
    let source = session_path(project);
    if !source.is_file() {
        return Ok(None);
    }
    let folder = backups_folder(project);
    fs::create_dir_all(&folder)?;
    let mut target = folder.join(format!("vrgdg_builder_session_{}.json", now.compact));
    let mut index = 2;
    while target.exists() {
        target = folder.join(format!("vrgdg_builder_session_{}_{index:02}.json", now.compact));
        index += 1;
    }
    fs::copy(&source, &target)?;
    prune_session_backups(project, MAX_SESSION_BACKUPS);
    Ok(Some(target))
}

/// Put a backup back as the current session. The session it replaces is backed up first. The revision is raised so
/// open copies of the project see the restore as a newer save, not a stale one.
pub fn restore_backup(project: &Path, backup: &Path, now: &Stamp) -> io::Result<()> {
    let text = fs::read_to_string(backup)?;
    let mut session = crate::pyjson::parse(&text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let Some(map) = session.as_object_mut() else {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "backup is not a JSON object"));
    };
    let current = fs::read_to_string(session_path(project))
        .ok()
        .and_then(|t| crate::pyjson::parse(&t).ok())
        .and_then(|v| v.get("revision").and_then(serde_json::Value::as_i64))
        .unwrap_or(0);
    let restored = map.get("revision").and_then(serde_json::Value::as_i64).unwrap_or(0);
    backup_session_file(project, now)?;
    map.insert("revision".into(), serde_json::json!(current.max(restored) + 1));
    crate::atomic::atomic_write_json(&session_path(project), &session)
}
