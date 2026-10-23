//! The job manager: a persistent queue that runs one GPU job at a time. Port of `agent_api/jobs/manager.py`.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use serde_json::{json, Value};
use tokio::sync::{broadcast, Semaphore};

use crate::model::*;
use crate::store::{JobStore, StoreError};

pub const JOB_CANCELLED: &str = "JOB_CANCELLED";

/// Why a handler stopped. `code == JOB_CANCELLED` ends the job as `cancelled`.
#[derive(Debug, Clone)]
pub struct JobError {
    pub code: String,
    pub message: String,
    pub details: Value,
    pub retryable: bool,
}

impl JobError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self { code: code.to_string(), message: message.into(), details: json!({}), retryable: false }
    }

    pub fn cancelled() -> Self {
        Self::new(JOB_CANCELLED, "Job execution was cancelled.")
    }

    pub fn to_value(&self) -> Value {
        json!({"code": self.code, "message": self.message, "details": self.details, "retryable": self.retryable})
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ManagerError {
    #[error("Job '{0}' was not found.")]
    NotFound(String),
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Store(#[from] StoreError),
}

pub type JobResult = Result<Value, JobError>;
pub type Handler = Arc<dyn Fn(JobCtx) -> Pin<Box<dyn Future<Output = JobResult> + Send>> + Send + Sync>;
/// Called when a running job is cancelled, to interrupt the work (ComfyUI's current prompt).
pub type CancelHook = Arc<dyn Fn(&Job) + Send + Sync>;

/// An event for the SSE stream.
#[derive(Debug, Clone)]
pub struct Event {
    pub name: String,
    pub data: Value,
    pub project_id: Option<String>,
}

/// Progress a handler reports. Unset batch fields keep their last value.
#[derive(Debug, Clone, Default)]
pub struct ProgressUpdate {
    pub percent: f64,
    pub stage: String,
    pub message: String,
    pub scene_id: Option<String>,
    pub scene_index: Option<i64>,
    pub scene_count: Option<i64>,
    pub stage_index: i64,
    pub stage_count: i64,
}

struct Inner {
    store: JobStore,
    jobs: Mutex<HashMap<String, Job>>,
    flags: Mutex<HashMap<String, Arc<AtomicBool>>>,
    handlers: RwLock<HashMap<String, Handler>>,
    cancel_hook: RwLock<Option<CancelHook>>,
    events: broadcast::Sender<Event>,
    gpu: Arc<Semaphore>,
    cpu: Arc<Semaphore>,
    queued_gpu: Mutex<VecDeque<String>>,
    active_gpu: Mutex<Option<String>>,
    log_seq: Mutex<HashMap<String, i64>>,
}

#[derive(Clone)]
pub struct JobManager {
    inner: Arc<Inner>,
}

/// What a handler gets to work with.
#[derive(Clone)]
pub struct JobCtx {
    pub job_id: String,
    pub project_id: Option<String>,
    pub params: Value,
    pub cancel: Arc<AtomicBool>,
    manager: JobManager,
}

impl JobCtx {
    pub fn progress(&self, update: ProgressUpdate) {
        self.manager.update_progress(&self.job_id, update);
    }

    pub fn log(&self, level: &str, message: &str) {
        self.manager.append_log(&self.job_id, level, message, None);
    }

    pub fn set_prompt_id(&self, prompt_id: &str) {
        self.manager.set_prompt_id(&self.job_id, prompt_id);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

impl JobManager {
    pub fn new(store: JobStore) -> Self {
        let (events, _) = broadcast::channel(256);
        Self {
            inner: Arc::new(Inner {
                store,
                jobs: Mutex::new(HashMap::new()),
                flags: Mutex::new(HashMap::new()),
                handlers: RwLock::new(HashMap::new()),
                cancel_hook: RwLock::new(None),
                events,
                gpu: Arc::new(Semaphore::new(1)),
                cpu: Arc::new(Semaphore::new(4)),
                queued_gpu: Mutex::new(VecDeque::new()),
                active_gpu: Mutex::new(None),
                log_seq: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub fn register(&self, kind: &str, handler: Handler) {
        self.inner.handlers.write().unwrap_or_else(|e| e.into_inner()).insert(kind.to_string(), handler);
    }

    pub fn set_cancel_hook(&self, hook: CancelHook) {
        *self.inner.cancel_hook.write().unwrap_or_else(|e| e.into_inner()) = Some(hook);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.inner.events.subscribe()
    }

    fn emit(&self, name: &str, data: Value, project_id: Option<String>) {
        let _ = self.inner.events.send(Event { name: name.to_string(), data, project_id });
    }

    /// Mark jobs a previous run left unfinished as `interrupted`. Call once at startup.
    pub fn recover_on_startup(&self) -> Result<usize, ManagerError> {
        let recovered = self.inner.store.recover_interrupted()?;
        for job in &recovered {
            self.append_log(&job.id, "warning", "The app stopped while this job was unfinished. It is marked interrupted.", None);
        }
        Ok(recovered.len())
    }

    fn mutate<R>(&self, id: &str, change: impl FnOnce(&mut Job) -> R) -> Result<(R, Job), ManagerError> {
        let mut jobs = self.inner.jobs.lock().unwrap_or_else(|e| e.into_inner());
        if !jobs.contains_key(id) {
            let loaded = self.inner.store.get(id)?.ok_or_else(|| ManagerError::NotFound(id.to_string()))?;
            jobs.insert(id.to_string(), loaded);
        }
        let job = jobs.get_mut(id).ok_or_else(|| ManagerError::NotFound(id.to_string()))?;
        let result = change(job);
        let snapshot = job.clone();
        drop(jobs);
        if let Err(e) = self.inner.store.save(&snapshot) {
            eprintln!("[VRGDG-RS Jobs] Could not save job {id}: {e}");
        }
        Ok((result, snapshot))
    }

    pub fn get(&self, id: &str) -> Result<Job, ManagerError> {
        if let Some(job) = self.inner.jobs.lock().unwrap_or_else(|e| e.into_inner()).get(id) {
            return Ok(job.clone());
        }
        self.inner.store.get(id)?.ok_or_else(|| ManagerError::NotFound(id.to_string()))
    }

    /// Jobs newest first. Live state wins over the stored copy.
    pub fn list(&self, project_id: Option<&str>, status: Option<&str>, kind: Option<&str>) -> Result<Vec<Job>, ManagerError> {
        let mut jobs = self.inner.store.list(project_id, status, kind)?;
        let live = self.inner.jobs.lock().unwrap_or_else(|e| e.into_inner());
        jobs.retain_mut(|job| {
            if let Some(current) = live.get(&job.id) {
                *job = current.clone();
            }
            project_id.is_none_or(|p| job.project_id.as_deref() == Some(p)) && status.is_none_or(|s| job.status == s) && kind.is_none_or(|k| job.kind == k)
        });
        // A live job that changed status may no longer be in the stored result for its old status.
        for job in live.values() {
            let wanted = project_id.is_none_or(|p| job.project_id.as_deref() == Some(p)) && status.is_none_or(|s| job.status == s) && kind.is_none_or(|k| job.kind == k);
            if wanted && !jobs.iter().any(|j| j.id == job.id) {
                jobs.push(job.clone());
            }
        }
        jobs.sort_by(|a, b| b.created_at.total_cmp(&a.created_at));
        Ok(jobs)
    }

    pub fn append_log(&self, id: &str, level: &str, message: &str, details: Option<Value>) {
        let seq = {
            let mut counters = self.inner.log_seq.lock().unwrap_or_else(|e| e.into_inner());
            let next = match counters.get(id) {
                Some(n) => n + 1,
                None => self.inner.store.last_log_seq(id).unwrap_or(0) + 1,
            };
            counters.insert(id.to_string(), next);
            next
        };
        let entry = LogEntry { seq, timestamp: now(), level: level.to_string(), message: message.to_string(), details };
        if let Err(e) = self.inner.store.append_log(id, &entry) {
            eprintln!("[VRGDG-RS Jobs] Could not save a log line for {id}: {e}");
        }
    }

    pub fn logs(&self, id: &str, since: i64) -> Result<Vec<LogEntry>, ManagerError> {
        self.get(id)?;
        Ok(self.inner.store.logs(id, since)?)
    }

    pub fn update_progress(&self, id: &str, update: ProgressUpdate) {
        let result = self.mutate(id, |job| {
            let previous = job.progress.clone();
            job.progress = JobProgress {
                percent: update.percent,
                stage: update.stage,
                stage_index: update.stage_index,
                stage_count: update.stage_count.max(1),
                message: update.message,
                scene_id: update.scene_id,
                eta_seconds: None,
                scene_index: update.scene_index.unwrap_or(previous.scene_index),
                scene_count: update.scene_count.unwrap_or(previous.scene_count),
            };
        });
        if let Ok((_, job)) = result {
            self.emit("job.updated", job.to_api(), job.project_id.clone());
        }
    }

    pub fn set_prompt_id(&self, id: &str, prompt_id: &str) {
        let _ = self.mutate(id, |job| {
            if let Some(ids) = job.comfy.get_mut("prompt_ids").and_then(Value::as_array_mut) {
                if !ids.iter().any(|v| v.as_str() == Some(prompt_id)) {
                    ids.push(json!(prompt_id));
                }
            }
            job.comfy["current_prompt_id"] = json!(prompt_id);
        });
    }

    /// Queue a job and start it as soon as its turn comes.
    pub fn submit(&self, kind: &str, project_id: Option<&str>, params: Value, is_gpu: bool) -> Result<Job, ManagerError> {
        self.submit_with(kind, project_id, params, is_gpu, 1, 1)
    }

    fn submit_with(&self, kind: &str, project_id: Option<&str>, params: Value, is_gpu: bool, attempt: i64, max_attempts: i64) -> Result<Job, ManagerError> {
        let mut job = Job::new(generate_job_id(), kind, project_id.map(str::to_string), params, is_gpu);
        job.attempt = attempt;
        job.max_attempts = max_attempts;
        self.inner.store.save(&job)?;
        self.inner.jobs.lock().unwrap_or_else(|e| e.into_inner()).insert(job.id.clone(), job.clone());
        self.inner.flags.lock().unwrap_or_else(|e| e.into_inner()).insert(job.id.clone(), Arc::new(AtomicBool::new(false)));
        if is_gpu {
            self.inner.queued_gpu.lock().unwrap_or_else(|e| e.into_inner()).push_back(job.id.clone());
        }
        self.append_log(&job.id, "info", &format!("Job '{}' queued.", job.id), None);
        self.emit("job.created", job.to_api(), job.project_id.clone());
        self.emit("queue.updated", self.queue_summary(), job.project_id.clone());
        let manager = self.clone();
        let id = job.id.clone();
        tokio::spawn(async move { manager.run(id).await });
        Ok(job)
    }

    fn flag(&self, id: &str) -> Arc<AtomicBool> {
        let mut flags = self.inner.flags.lock().unwrap_or_else(|e| e.into_inner());
        Arc::clone(flags.entry(id.to_string()).or_insert_with(|| Arc::new(AtomicBool::new(false))))
    }

    pub fn cancel(&self, id: &str) -> Result<Job, ManagerError> {
        let job = self.get(id)?;
        if job.is_terminal() {
            return Ok(job);
        }
        self.flag(id).store(true, Ordering::Relaxed);
        self.append_log(id, "warning", &format!("Cancellation requested for job '{id}'."), None);
        let (_, job) = self.mutate(id, |job| {
            job.cancel_requested = true;
            if job.status == QUEUED {
                job.status = CANCELLED.into();
                job.finished_at = Some(now());
            }
        })?;
        self.inner.queued_gpu.lock().unwrap_or_else(|e| e.into_inner()).retain(|queued| queued != id);
        if job.status == RUNNING {
            if let Some(hook) = self.inner.cancel_hook.read().unwrap_or_else(|e| e.into_inner()).clone() {
                hook(&job);
            }
        }
        self.emit("job.updated", job.to_api(), job.project_id.clone());
        self.emit("queue.updated", self.queue_summary(), job.project_id.clone());
        Ok(job)
    }

    /// A new job from a finished one. With `resume` the batch skips what already has a clip.
    pub fn retry(&self, id: &str, resume: bool) -> Result<Job, ManagerError> {
        let old = self.get(id)?;
        if !old.is_terminal() {
            return Err(ManagerError::Invalid(format!("Cannot retry job '{id}' with non-terminal status '{}'.", old.status)));
        }
        let mut params = old.params.clone();
        if resume {
            if let Some(map) = params.as_object_mut() {
                map.insert("resume".into(), json!(true));
                map.insert("run_mode".into(), json!("resume_missing"));
            }
        }
        self.submit_with(&old.kind, old.project_id.as_deref(), params, old.is_gpu, old.attempt + 1, old.max_attempts.max(old.attempt + 1))
    }

    pub fn queue_summary(&self) -> Value {
        let queued: Vec<String> = self.inner.queued_gpu.lock().unwrap_or_else(|e| e.into_inner()).iter().cloned().collect();
        let active = self.inner.active_gpu.lock().unwrap_or_else(|e| e.into_inner()).clone();
        json!({
            "gpu_running": i64::from(active.is_some()),
            "gpu_queued": queued.len(),
            "active_job_id": active,
            "queue": queued,
            "comfy_queue_depth": 0,
        })
    }

    async fn run(self, id: String) {
        let job = match self.get(&id) {
            Ok(job) => job,
            Err(_) => return,
        };
        let semaphore = if job.is_gpu { Arc::clone(&self.inner.gpu) } else { Arc::clone(&self.inner.cpu) };
        let Ok(_permit) = semaphore.acquire_owned().await else { return };
        let flag = self.flag(&id);
        if flag.load(Ordering::Relaxed) || self.get(&id).map(|j| j.is_terminal()).unwrap_or(true) {
            let _ = self.mutate(&id, |job| {
                if !job.is_terminal() {
                    job.status = CANCELLED.into();
                    job.finished_at = Some(now());
                }
            });
            self.finish_bookkeeping(&id);
            return;
        }
        if job.is_gpu {
            *self.inner.active_gpu.lock().unwrap_or_else(|e| e.into_inner()) = Some(id.clone());
            self.inner.queued_gpu.lock().unwrap_or_else(|e| e.into_inner()).retain(|queued| queued != &id);
        }
        let started = self.mutate(&id, |job| {
            job.status = RUNNING.into();
            job.started_at = Some(now());
            job.progress.stage = "started".into();
            job.progress.message = "Running...".into();
        });
        if let Ok((_, job)) = started {
            self.emit("job.updated", job.to_api(), job.project_id.clone());
        }
        self.append_log(&id, "info", &format!("Job '{id}' started."), None);

        let handler = self.inner.handlers.read().unwrap_or_else(|e| e.into_inner()).get(&job.kind).cloned();
        let outcome: JobResult = match handler {
            None => Err(JobError::new("HANDLER_NOT_FOUND", format!("No handler registered for job type '{}'.", job.kind))),
            Some(handler) => {
                let ctx = JobCtx { job_id: id.clone(), project_id: job.project_id.clone(), params: job.params.clone(), cancel: Arc::clone(&flag), manager: self.clone() };
                // A panic in a handler fails this job, not the whole app.
                match tokio::spawn(handler(ctx)).await {
                    Ok(result) => result,
                    Err(e) => Err(JobError { code: "INTERNAL_ERROR".into(), message: format!("The job crashed: {e}"), details: json!({}), retryable: true }),
                }
            }
        };
        let _ = self.mutate(&id, |job| {
            job.finished_at = Some(now());
            match &outcome {
                Ok(value) => {
                    job.status = SUCCEEDED.into();
                    job.progress.percent = 100.0;
                    job.progress.stage = "completed".into();
                    job.progress.message = "Job finished successfully.".into();
                    job.result = Some(value.clone());
                }
                Err(error) if error.code == JOB_CANCELLED => job.status = CANCELLED.into(),
                Err(error) => {
                    job.status = FAILED.into();
                    job.error = Some(error.to_value());
                }
            }
        });
        let (level, line) = match &outcome {
            Ok(_) => ("info", format!("Job '{id}' completed successfully.")),
            Err(e) if e.code == JOB_CANCELLED => ("warning", format!("Job '{id}' was cancelled.")),
            Err(e) => ("error", format!("Job '{id}' failed: {}", e.message)),
        };
        self.append_log(&id, level, &line, None);
        self.finish_bookkeeping(&id);
    }

    fn finish_bookkeeping(&self, id: &str) {
        {
            let mut active = self.inner.active_gpu.lock().unwrap_or_else(|e| e.into_inner());
            if active.as_deref() == Some(id) {
                *active = None;
            }
        }
        self.inner.queued_gpu.lock().unwrap_or_else(|e| e.into_inner()).retain(|queued| queued != id);
        if let Ok(job) = self.get(id) {
            self.emit("job.updated", job.to_api(), job.project_id.clone());
            self.emit("queue.updated", self.queue_summary(), job.project_id.clone());
        }
    }
}
