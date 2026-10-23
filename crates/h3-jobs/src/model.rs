//! Job data model. Field names and values match the pack's `agent_api/jobs/models.py` so API clients keep working.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const QUEUED: &str = "queued";
pub const RUNNING: &str = "running";
pub const SUCCEEDED: &str = "succeeded";
pub const FAILED: &str = "failed";
pub const CANCELLED: &str = "cancelled";
pub const INTERRUPTED: &str = "interrupted";

pub fn is_terminal(status: &str) -> bool {
    matches!(status, SUCCEEDED | FAILED | CANCELLED | INTERRUPTED)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct JobProgress {
    pub percent: f64,
    pub stage: String,
    pub stage_index: i64,
    pub stage_count: i64,
    pub message: String,
    pub scene_id: Option<String>,
    pub eta_seconds: Option<f64>,
    pub scene_index: i64,
    pub scene_count: i64,
}

impl Default for JobProgress {
    fn default() -> Self {
        Self { percent: 0.0, stage: "queued".into(), stage_index: 0, stage_count: 1, message: String::new(), scene_id: None, eta_seconds: None, scene_index: 0, scene_count: 0 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LogEntry {
    pub seq: i64,
    pub timestamp: f64,
    pub level: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Job {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub project_id: Option<String>,
    pub status: String,
    pub created_at: f64,
    pub started_at: Option<f64>,
    pub finished_at: Option<f64>,
    pub progress: JobProgress,
    pub comfy: Value,
    pub params: Value,
    pub result: Option<Value>,
    pub error: Option<Value>,
    pub warnings: Vec<String>,
    pub cancel_requested: bool,
    pub attempt: i64,
    pub max_attempts: i64,
    #[serde(default = "yes")]
    pub is_gpu: bool,
}

fn yes() -> bool {
    true
}

impl Job {
    pub fn new(id: String, kind: &str, project_id: Option<String>, params: Value, is_gpu: bool) -> Self {
        Self {
            id,
            kind: kind.to_string(),
            project_id,
            status: QUEUED.into(),
            created_at: now(),
            started_at: None,
            finished_at: None,
            progress: JobProgress::default(),
            comfy: json!({"prompt_ids": [], "current_prompt_id": null}),
            params,
            result: None,
            error: None,
            warnings: vec![],
            cancel_requested: false,
            attempt: 1,
            max_attempts: 1,
            is_gpu,
        }
    }

    pub fn is_terminal(&self) -> bool {
        is_terminal(&self.status)
    }

    /// The API view. `is_gpu` is internal and left out, as in the pack.
    pub fn to_api(&self) -> Value {
        let mut value = serde_json::to_value(self).unwrap_or(Value::Null);
        if let Value::Object(map) = &mut value {
            map.remove("is_gpu");
            if let Some(Value::Object(progress)) = map.get_mut("progress") {
                if let Some(p) = progress.get("percent").and_then(Value::as_f64) {
                    progress.insert("percent".into(), json!((p * 10.0).round() / 10.0));
                }
            }
        }
        value
    }
}

/// Seconds since the Unix epoch.
pub fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// `job_YYYYMMDD_HHMMSS_xxxx`.
pub fn generate_job_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    format!("job_{}_{:04x}", chrono::Local::now().format("%Y%m%d_%H%M%S"), hasher.finish() & 0xffff)
}
