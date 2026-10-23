//! Transactional rename journal for timeline changes. Port of `TimelineJournal` and `recover_pending_journal` in
//! `builder/timeline.py`.
//!
//! A change that renames scene files first copies the session to a backup and writes a journal. Each rename is recorded
//! as it happens. If the change fails, `rollback` reverses the renames and restores the session. If the app dies
//! first, `recover_pending` does the same on the next start.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::atomic::atomic_write_json;
use crate::layout::session_path;
use crate::pyjson;

pub const JOURNAL_FILENAME: &str = ".timeline_journal.json";

pub struct TimelineJournal {
    operation: String,
    journal_path: PathBuf,
    session_path: PathBuf,
    backup_path: PathBuf,
    renames: Vec<(PathBuf, PathBuf)>,
}

impl TimelineJournal {
    /// `now_ms` is the millisecond clock the backup file name carries.
    pub fn new(project: &Path, operation: &str, now_ms: u64) -> Self {
        Self {
            operation: operation.to_string(),
            journal_path: project.join(JOURNAL_FILENAME),
            session_path: session_path(project),
            backup_path: project.join(format!(".timeline_session_backup_{now_ms}.json")),
            renames: Vec::new(),
        }
    }

    fn write(&self, renames: &[Value], stamp_key: &str, now: f64) -> io::Result<()> {
        let value = json!({
            "status": "in_progress",
            "operation": self.operation,
            "backup_session": self.backup_path.display().to_string(),
            "renames": renames,
            stamp_key: now,
        });
        atomic_write_json(&self.journal_path, &value)
    }

    /// Back up the session and write the journal.
    pub fn start(&self, now: f64) -> io::Result<()> {
        if self.session_path.is_file() {
            fs::copy(&self.session_path, &self.backup_path)?;
        }
        self.write(&[], "created_at", now)
    }

    /// Record renames that were just done.
    pub fn record_renames(&mut self, renames: &[(PathBuf, PathBuf)], now: f64) -> io::Result<()> {
        self.renames.extend(renames.iter().cloned());
        if self.journal_path.is_file() {
            let list: Vec<Value> = self
                .renames
                .iter()
                .map(|(s, t)| json!([s.display().to_string(), t.display().to_string()]))
                .collect();
            self.write(&list, "updated_at", now)?;
        }
        Ok(())
    }

    /// The change succeeded: drop the journal and the backup.
    pub fn commit(&self) {
        let _ = fs::remove_file(&self.journal_path);
        let _ = fs::remove_file(&self.backup_path);
    }

    /// Reverse every recorded rename and restore the session backup.
    pub fn rollback(&self) {
        undo_renames(self.renames.iter().map(|(s, t)| (s.as_path(), t.as_path())).rev());
        if self.backup_path.is_file() && fs::copy(&self.backup_path, &self.session_path).is_ok() {
            let _ = fs::remove_file(&self.backup_path);
        }
        let _ = fs::remove_file(&self.journal_path);
    }
}

fn undo_renames<'a>(pairs: impl Iterator<Item = (&'a Path, &'a Path)>) {
    for (source, target) in pairs {
        if target.exists() && !source.exists() {
            let _ = fs::rename(target, source);
        }
    }
}

/// Roll back an interrupted change found at start up. Returns `true` when a journal was found and handled.
pub fn recover_pending(project: &Path) -> bool {
    let journal_path = project.join(JOURNAL_FILENAME);
    if !journal_path.is_file() {
        return false;
    }
    let run = || -> Option<()> {
        let data = pyjson::parse(&fs::read_to_string(&journal_path).ok()?).ok()?;
        let renames: Vec<(PathBuf, PathBuf)> = data
            .get("renames")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|item| {
                        let pair = item.as_array()?;
                        (pair.len() == 2).then(|| (PathBuf::from(pair[0].as_str().unwrap_or("")), PathBuf::from(pair[1].as_str().unwrap_or(""))))
                    })
                    .collect()
            })
            .unwrap_or_default();
        undo_renames(renames.iter().map(|(s, t)| (s.as_path(), t.as_path())).rev());
        if let Some(backup) = data.get("backup_session").and_then(Value::as_str).map(PathBuf::from) {
            if backup.is_file() {
                fs::copy(&backup, session_path(project)).ok()?;
                let _ = fs::remove_file(&backup);
            }
        }
        fs::remove_file(&journal_path).ok()
    };
    run().is_some()
}
