use std::sync::Arc;
use std::time::Duration;

use h3_jobs::{Handler, JobError, JobManager, JobStore, ProgressUpdate};
use serde_json::{json, Value};

fn handler<F, Fut>(f: F) -> Handler
where
    F: Fn(h3_jobs::JobCtx) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Value, JobError>> + Send + 'static,
{
    Arc::new(move |ctx| Box::pin(f(ctx)))
}

async fn wait_for(manager: &JobManager, id: &str, status: &str) {
    for _ in 0..200 {
        if manager.get(id).unwrap().status == status {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("job {id} never reached {status}, now {}", manager.get(id).unwrap().status);
}

#[tokio::test]
async fn runs_a_job_and_keeps_progress_and_logs() {
    let manager = JobManager::new(JobStore::open_in_memory().unwrap());
    manager.register(
        "demo",
        handler(|ctx| async move {
            ctx.progress(ProgressUpdate { percent: 50.0, stage: "half".into(), message: "halfway".into(), scene_count: Some(2), ..Default::default() });
            ctx.log("info", "worked");
            Ok(json!({"done": true}))
        }),
    );
    let job = manager.submit("demo", Some("p"), json!({"a": 1}), true).unwrap();
    wait_for(&manager, &job.id, "succeeded").await;
    let done = manager.get(&job.id).unwrap();
    assert_eq!(done.result, Some(json!({"done": true})));
    assert_eq!(done.progress.percent, 100.0);
    assert_eq!(done.progress.scene_count, 2);
    let logs = manager.logs(&job.id, 0).unwrap();
    assert!(logs.iter().any(|l| l.message == "worked"));
    assert!(logs.windows(2).all(|w| w[0].seq < w[1].seq));
    assert_eq!(manager.logs(&job.id, logs[0].seq).unwrap().len(), logs.len() - 1);
}

#[tokio::test]
async fn gpu_jobs_run_one_at_a_time_and_a_queued_job_can_be_cancelled() {
    let manager = JobManager::new(JobStore::open_in_memory().unwrap());
    manager.register(
        "slow",
        handler(|ctx| async move {
            for _ in 0..200 {
                if ctx.is_cancelled() {
                    return Err(JobError::cancelled());
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Ok(json!({}))
        }),
    );
    let first = manager.submit("slow", None, json!({}), true).unwrap();
    let second = manager.submit("slow", None, json!({}), true).unwrap();
    wait_for(&manager, &first.id, "running").await;
    assert_eq!(manager.get(&second.id).unwrap().status, "queued");
    assert_eq!(manager.queue_summary()["gpu_queued"], 1);
    manager.cancel(&second.id).unwrap();
    assert_eq!(manager.get(&second.id).unwrap().status, "cancelled");
    manager.cancel(&first.id).unwrap();
    wait_for(&manager, &first.id, "cancelled").await;
}

#[tokio::test]
async fn a_failing_handler_records_the_error_and_retry_makes_a_new_job() {
    let manager = JobManager::new(JobStore::open_in_memory().unwrap());
    manager.register("bad", handler(|_| async { Err(JobError::new("COMFY_NODE_ERRORS", "boom")) }));
    let job = manager.submit("bad", Some("p"), json!({"scene_id": "3"}), true).unwrap();
    wait_for(&manager, &job.id, "failed").await;
    assert_eq!(manager.get(&job.id).unwrap().error.unwrap()["code"], "COMFY_NODE_ERRORS");
    assert!(manager.retry(&manager.submit("bad", None, json!({}), false).unwrap().id, false).is_err());
    let retried = manager.retry(&job.id, true).unwrap();
    assert_eq!(retried.attempt, 2);
    assert_eq!(retried.params["resume"], true);
    assert_eq!(retried.params["run_mode"], "resume_missing");
    assert_eq!(retried.params["scene_id"], "3");
}

#[tokio::test]
async fn unfinished_jobs_become_interrupted_after_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("jobs.db");
    let running_id;
    let queued_id;
    {
        let manager = JobManager::new(JobStore::open(&db).unwrap());
        manager.register("hang", handler(|_| async { tokio::time::sleep(Duration::from_secs(60)).await; Ok(json!({})) }));
        running_id = manager.submit("hang", Some("p"), json!({"x": 1}), true).unwrap().id;
        queued_id = manager.submit("hang", Some("p"), json!({}), true).unwrap().id;
        wait_for(&manager, &running_id, "running").await;
    }
    // A new process opens the same database.
    let manager = JobManager::new(JobStore::open(&db).unwrap());
    assert_eq!(manager.recover_on_startup().unwrap(), 2);
    for id in [&running_id, &queued_id] {
        let job = manager.get(id).unwrap();
        assert_eq!(job.status, "interrupted");
        assert_eq!(job.error.unwrap()["code"], "JOB_INTERRUPTED");
    }
    assert_eq!(manager.list(Some("p"), Some("interrupted"), None).unwrap().len(), 2);
    let log = manager.logs(&running_id, 0).unwrap();
    assert!(log.iter().any(|l| l.message.contains("interrupted")));
}
