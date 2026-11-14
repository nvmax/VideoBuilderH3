//! The ComfyUI prompts that jobs record, and the ones a stopped app leaves running. A stand-in ComfyUI answers.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use h3_jobs::{Handler, JobError, JobManager, JobStore};
use h3_project::AppConfig;
use reqwest::Client;
use serde_json::{json, Value};

type Calls = Arc<Mutex<Vec<String>>>;

fn handler<F, Fut>(f: F) -> Handler
where
    F: Fn(h3_jobs::JobCtx) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Value, JobError>> + Send + 'static,
{
    Arc::new(move |ctx| Box::pin(f(ctx)))
}

async fn wait_running(manager: &JobManager, id: &str) {
    for _ in 0..200 {
        if manager.get(id).unwrap().status == "running" {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("job {id} never started");
}

/// A ComfyUI that runs one prompt, holds two more and records what it is asked to stop.
async fn comfy_with_queue(calls: &Calls) -> String {
    let comfy = Router::new()
        .route(
            "/queue",
            get(|| async {
                Json(json!({
                    "queue_running": [[1, "orphan-running", {}, {}, []]],
                    "queue_pending": [[2, "orphan-waiting", {}, {}, []], [3, "someone-elses", {}, {}, []]],
                }))
            })
            .post(|State(calls): State<Calls>, Json(body): Json<Value>| async move {
                calls.lock().unwrap().push(format!("queue {body}"));
                Json(json!({}))
            }),
        )
        .route(
            "/interrupt",
            post(|State(calls): State<Calls>| async move {
                calls.lock().unwrap().push("interrupt".into());
                Json(json!({}))
            }),
        )
        .with_state(Arc::clone(calls));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, comfy).await.unwrap() });
    url
}

#[tokio::test]
async fn a_restart_stops_the_prompts_the_last_run_left_in_comfyui() {
    let calls: Calls = Arc::default();
    let comfy_url = comfy_with_queue(&calls).await;

    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("jobs.db");
    {
        // The last run: three jobs that each had a prompt in ComfyUI when the app stopped.
        let manager = JobManager::new(JobStore::open(&db).unwrap());
        manager.register("hang", handler(|_| async { tokio::time::sleep(Duration::from_secs(60)).await; Ok(json!({})) }));
        for prompt in ["orphan-running", "orphan-waiting", "already-finished"] {
            let id = manager.submit("hang", Some("p"), json!({}), false).unwrap().id;
            wait_running(&manager, &id).await;
            manager.set_prompt_id(&id, prompt);
        }
    }

    let cfg = AppConfig { comfy_url, project_root: dir.path().join("projects"), ..AppConfig::default() };
    let state = h3_api::build_state(cfg, &db).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    tokio::spawn(async move { h3_api::serve_on(listener, state).await.unwrap() });

    for _ in 0..200 {
        if calls.lock().unwrap().len() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    // Give a wrong extra call time to arrive.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let seen = calls.lock().unwrap().clone();
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert!(seen.contains(&"interrupt".to_string()), "the running prompt is interrupted: {seen:?}");
    let removed = seen.iter().find(|c| c.starts_with("queue ")).expect("the waiting prompt is taken out of the queue");
    assert!(removed.contains("orphan-waiting") && !removed.contains("someone-elses") && !removed.contains("already-finished"), "{removed}");
}

#[tokio::test]
async fn a_restart_leaves_comfyui_alone_when_no_job_had_a_prompt() {
    let calls: Calls = Arc::default();
    let comfy_url = comfy_with_queue(&calls).await;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("jobs.db");
    {
        let manager = JobManager::new(JobStore::open(&db).unwrap());
        manager.register("hang", handler(|_| async { tokio::time::sleep(Duration::from_secs(60)).await; Ok(json!({})) }));
        let id = manager.submit("hang", Some("p"), json!({}), false).unwrap().id;
        wait_running(&manager, &id).await;
    }
    let cfg = AppConfig { comfy_url, project_root: dir.path().join("projects"), ..AppConfig::default() };
    let state = h3_api::build_state(cfg, &db).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    tokio::spawn(async move { h3_api::serve_on(listener, state).await.unwrap() });
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(calls.lock().unwrap().is_empty(), "nothing to stop, so ComfyUI is not asked to");
}

#[tokio::test]
async fn an_image_job_keeps_the_prompt_comfyui_gave_it() {
    let comfy_dir = tempfile::tempdir().unwrap();
    let main_py = comfy_dir.path().join("ComfyUI").join("main.py").display().to_string();
    let comfy = Router::new()
        .route("/system_stats", get(move || async move { Json(json!({"system": {"argv": [main_py]}, "devices": []})) }))
        .route("/object_info", get(|| async { Json(json!({})) }))
        .route("/prompt", post(|| async { Json(json!({"prompt_id": "p-image-1", "number": 1, "node_errors": {}})) }))
        // The prompt finishes without a picture, so the job fails, after the prompt id was given to it.
        .route("/history/{id}", get(|axum::extract::Path(id): axum::extract::Path<String>| async move { Json(json!({ id: {"status": {"status_str": "success", "messages": []}, "outputs": {}} })) }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let comfy_url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, comfy).await.unwrap() });

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("projects");
    let project = root.join("Demo");
    std::fs::create_dir_all(&project).unwrap();
    let session = json!({"project_name": "Demo", "revision": 3, "builder_save_revision": 3, "segments": [{"id": "seg_0001", "start": 0.0, "end": 4.5, "t2i_prompt": "a lake"}]});
    std::fs::write(project.join("vrgdg_builder_session.json"), serde_json::to_string_pretty(&session).unwrap()).unwrap();
    let cfg = AppConfig { comfy_url, project_root: root, ..AppConfig::default() };
    let state = h3_api::build_state(cfg, &dir.path().join("jobs.db")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/api/v1", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, h3_api::router(state)).await.unwrap() });

    let client = Client::new();
    let started: Value = client.post(format!("{base}/projects/Demo/scenes/1/image/generate")).json(&json!({"prompt": "a lake"})).send().await.unwrap().json().await.unwrap();
    let id = started["data"]["job_id"].as_str().expect("a job id").to_string();
    let mut job = Value::Null;
    for _ in 0..200 {
        let body: Value = client.get(format!("{base}/jobs/{id}")).send().await.unwrap().json().await.unwrap();
        if body["data"]["status"] == "failed" || body["data"]["status"] == "succeeded" {
            job = body["data"].clone();
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!job.is_null(), "the job finished");
    assert_eq!(job["comfy"]["current_prompt_id"], "p-image-1", "{job}");
    assert_eq!(job["comfy"]["prompt_ids"], json!(["p-image-1"]), "{job}");
}
