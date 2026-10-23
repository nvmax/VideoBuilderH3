//! SQLite persistence for jobs and their logs.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection};

use crate::model::{Job, LogEntry, INTERRUPTED, QUEUED, RUNNING};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("job database: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("job database: {0}")]
    Json(#[from] serde_json::Error),
}

pub struct JobStore {
    conn: Mutex<Connection>,
}

impl JobStore {
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self, StoreError> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self, StoreError> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS jobs (
                 id TEXT PRIMARY KEY,
                 created_at REAL NOT NULL,
                 project_id TEXT,
                 status TEXT NOT NULL,
                 kind TEXT NOT NULL,
                 body TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS jobs_project ON jobs(project_id, created_at);
             CREATE TABLE IF NOT EXISTS logs (
                 job_id TEXT NOT NULL,
                 seq INTEGER NOT NULL,
                 timestamp REAL NOT NULL,
                 level TEXT NOT NULL,
                 message TEXT NOT NULL,
                 details TEXT,
                 PRIMARY KEY (job_id, seq)
             );",
        )?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn save(&self, job: &Job) -> Result<(), StoreError> {
        let body = serde_json::to_string(job)?;
        self.lock().execute(
            "INSERT INTO jobs (id, created_at, project_id, status, kind, body) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET status = excluded.status, body = excluded.body",
            params![job.id, job.created_at, job.project_id, job.status, job.kind, body],
        )?;
        Ok(())
    }

    pub fn get(&self, id: &str) -> Result<Option<Job>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare("SELECT body FROM jobs WHERE id = ?1")?;
        let mut rows = stmt.query(params![id])?;
        match rows.next()? {
            Some(row) => Ok(Some(serde_json::from_str(&row.get::<_, String>(0)?)?)),
            None => Ok(None),
        }
    }

    /// Jobs newest first.
    pub fn list(&self, project_id: Option<&str>, status: Option<&str>, kind: Option<&str>) -> Result<Vec<Job>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT body FROM jobs
             WHERE (?1 IS NULL OR project_id = ?1) AND (?2 IS NULL OR status = ?2) AND (?3 IS NULL OR kind = ?3)
             ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map(params![project_id, status, kind], |row| row.get::<_, String>(0))?;
        let mut jobs = Vec::new();
        for body in rows {
            jobs.push(serde_json::from_str(&body?)?);
        }
        Ok(jobs)
    }

    pub fn append_log(&self, job_id: &str, entry: &LogEntry) -> Result<(), StoreError> {
        let details = entry.details.as_ref().map(|d| d.to_string());
        self.lock().execute(
            "INSERT OR REPLACE INTO logs (job_id, seq, timestamp, level, message, details) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![job_id, entry.seq, entry.timestamp, entry.level, entry.message, details],
        )?;
        Ok(())
    }

    pub fn logs(&self, job_id: &str, since: i64) -> Result<Vec<LogEntry>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare("SELECT seq, timestamp, level, message, details FROM logs WHERE job_id = ?1 AND seq > ?2 ORDER BY seq")?;
        let rows = stmt.query_map(params![job_id, since], |row| {
            let details: Option<String> = row.get(4)?;
            Ok(LogEntry {
                seq: row.get(0)?,
                timestamp: row.get(1)?,
                level: row.get(2)?,
                message: row.get(3)?,
                details: details.and_then(|d| serde_json::from_str(&d).ok()),
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn last_log_seq(&self, job_id: &str) -> Result<i64, StoreError> {
        let conn = self.lock();
        Ok(conn.query_row("SELECT COALESCE(MAX(seq), 0) FROM logs WHERE job_id = ?1", params![job_id], |r| r.get(0))?)
    }

    /// Mark every job a previous run left `running` or `queued` as `interrupted`. Returns the jobs.
    pub fn recover_interrupted(&self) -> Result<Vec<Job>, StoreError> {
        let mut recovered = Vec::new();
        for status in [RUNNING, QUEUED] {
            for mut job in self.list(None, Some(status), None)? {
                job.status = INTERRUPTED.into();
                job.finished_at = Some(crate::model::now());
                job.error = Some(serde_json::json!({
                    "code": "JOB_INTERRUPTED",
                    "message": "The app stopped while this job was running. Retry with resume to finish the rest.",
                    "details": {},
                    "retryable": true,
                }));
                self.save(&job)?;
                recovered.push(job);
            }
        }
        Ok(recovered)
    }
}
