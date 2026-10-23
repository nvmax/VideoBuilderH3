//! Background job queue with SQLite persistence and progress events.
//!
//! One GPU job runs at a time, other jobs run in parallel. Every change is saved to SQLite, so a job a stopped app left
//! unfinished comes back as `interrupted` and can be retried with `resume`.

pub mod manager;
pub mod model;
pub mod store;

pub use manager::{CancelHook, Event, Handler, JobCtx, JobError, JobManager, JobResult, ManagerError, ProgressUpdate, JOB_CANCELLED};
pub use model::{Job, JobProgress, LogEntry};
pub use store::{JobStore, StoreError};
