//! The API against a scratch project. No ComfyUI runs: a render job fails with COMFY_UNAVAILABLE, which is what the job
//! system and the error mapping are checked with.

use std::time::Duration;

use h3_project::AppConfig;
use reqwest::{Client, Method};
use serde_json::{json, Value};

struct Server {
    base: String,
    client: Client,
    _dir: tempfile::TempDir,
}

fn session() -> Value {
    json!({
        "project_name": "Demo",
        "revision": 3,
        "builder_save_revision": 3,
        "video_engine": "minimax_h3",
        "segments": [
            {"id": "seg_0001", "start": 0.0, "end": 4.5, "lyric_text": "first line", "minimax_h3_prompt": "A singer on a roof."},
            {"id": "seg_0002", "start": 4.5, "end": 9.0, "notes": "second"}
        ],
        "minimax_h3_settings": {"render_pass": "single"}
    })
}

async fn start() -> Server {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("projects").join("Demo Project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("vrgdg_builder_session.json"), serde_json::to_string_pretty(&session()).unwrap()).unwrap();
    let cfg = AppConfig {
        // Nothing listens on port 9, so ComfyUI counts as down.
        comfy_url: "http://127.0.0.1:9".into(),
        project_root: dir.path().join("projects"),
        ..AppConfig::default()
    };
    let state = h3_api::build_state(cfg, &dir.path().join("jobs.db")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, h3_api::router(state)).await.unwrap() });
    Server { base: format!("http://{addr}/api/v1"), client: Client::new(), _dir: dir }
}

impl Server {
    async fn call(&self, method: Method, path: &str, headers: &[(&str, &str)], body: Option<Value>) -> (u16, Value) {
        let mut request = self.client.request(method, format!("{}{}", self.base, path));
        for (k, v) in headers {
            request = request.header(*k, *v);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Value::Null))
    }

    async fn get(&self, path: &str) -> (u16, Value) {
        self.call(Method::GET, path, &[], None).await
    }

    async fn wait_job(&self, id: &str, status: &str) -> Value {
        for _ in 0..200 {
            let (_, body) = self.get(&format!("/jobs/{id}")).await;
            if body["data"]["status"] == status {
                return body["data"].clone();
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("job {id} never reached {status}");
    }
}

#[tokio::test]
async fn projects_scenes_and_settings() {
    let s = start().await;
    let (code, body) = s.get("/projects").await;
    assert_eq!(code, 200);
    assert_eq!(body["ok"], true);
    assert_eq!(body["data"][0]["id"], "Demo Project");
    assert_eq!(body["data"][0]["scene_count"], 2);
    assert_eq!(body["data"][0]["revision"], 3);

    // A space in the id arrives percent encoded.
    let (code, body) = s.get("/projects/Demo%20Project").await;
    assert_eq!(code, 200);
    assert_eq!(body["revision"], 3);
    assert_eq!(body["data"]["scenes"].as_array().unwrap().len(), 2);
    assert_eq!(body["data"]["settings"]["minimax_h3"]["render_pass"], "single");

    let (_, body) = s.get("/projects/Demo%20Project/summary").await;
    assert_eq!(body["data"]["scenes_with_prompts"], 1);
    assert_eq!(body["data"]["total_duration"], 9.0);

    let (_, body) = s.get("/projects/Demo%20Project/scenes?has_prompt=true").await;
    assert_eq!(body["data"].as_array().unwrap().len(), 1);
    let (code, body) = s.get("/projects/Demo%20Project/scenes/2").await;
    assert_eq!(code, 200);
    assert_eq!(body["data"]["id"], "seg_0002");
    assert_eq!(body["data"]["lyrics"], "second");
    let (code, body) = s.get("/projects/Demo%20Project/scenes/nope").await;
    assert_eq!(code, 404);
    assert_eq!(body["error"]["code"], "SCENE_NOT_FOUND");
    let (code, body) = s.get("/projects/Missing").await;
    assert_eq!((code, body["error"]["code"].as_str()), (404, Some("PROJECT_NOT_FOUND")));
    let (code, body) = s.get("/projects/..%2Fsecret").await;
    assert_eq!((code, body["error"]["code"].as_str()), (400, Some("VALIDATION_ERROR")));

    // A settings patch bumps the revision. A stale If-Match is a 409 and an unknown key is a 400.
    let patch = json!({"minimax_h3": {"render_pass": "two_pass"}});
    let (code, body) = s.call(Method::PATCH, "/projects/Demo%20Project/settings", &[("If-Match", "2")], Some(patch.clone())).await;
    assert_eq!((code, body["error"]["code"].as_str()), (409, Some("REVISION_CONFLICT")));
    assert_eq!(body["error"]["details"]["current_revision"], 3);
    let (code, body) = s.call(Method::PATCH, "/projects/Demo%20Project/settings", &[("If-Match", "3")], Some(patch)).await;
    assert_eq!(code, 200, "{body}");
    assert_eq!(body["revision"], 4);
    assert_eq!(body["data"]["settings"]["minimax_h3"]["render_pass"], "two_pass");
    let (_, body) = s.get("/projects/Demo%20Project/settings").await;
    assert_eq!(body["data"]["minimax_h3"]["render_pass"], "two_pass");
    let (code, body) = s.call(Method::PATCH, "/projects/Demo%20Project/settings", &[], Some(json!({"minimax_h3": {"not_a_setting": 1}}))).await;
    assert_eq!((code, body["error"]["code"].as_str()), (400, Some("SETTINGS_INVALID")));
    assert!(body["error"]["details"]["invalid_fields"]["not_a_setting"].is_string());
}

#[tokio::test]
async fn a_render_job_fails_cleanly_when_comfyui_is_down_and_can_be_retried() {
    let s = start().await;
    let (code, body) = s.call(Method::POST, "/projects/Demo%20Project/scenes/1/video/render", &[], Some(json!({"seed": 7}))).await;
    assert_eq!(code, 202, "{body}");
    let id = body["data"]["job_id"].as_str().unwrap().to_string();
    let job = s.wait_job(&id, "failed").await;
    assert_eq!(job["error"]["code"], "COMFY_UNAVAILABLE");
    assert_eq!(job["type"], "video.render");
    assert_eq!(job["project_id"], "Demo Project");
    assert_eq!(job["params"]["scene_id"], "1");

    let (_, body) = s.get(&format!("/jobs/{id}/log")).await;
    assert!(body["data"]["logs"].as_array().unwrap().len() >= 2);
    let (_, body) = s.get("/jobs?project_id=Demo%20Project&status=failed").await;
    assert_eq!(body["data"].as_array().unwrap().len(), 1);

    let (code, body) = s.call(Method::POST, &format!("/jobs/{id}/retry"), &[], Some(json!({"resume": true}))).await;
    assert_eq!(code, 202);
    assert_eq!(body["data"]["attempt"], 2);
    let retried = s.wait_job(body["data"]["id"].as_str().unwrap(), "failed").await;
    assert_eq!(retried["params"]["resume"], true);

    let (code, body) = s.get("/jobs/job_nope").await;
    assert_eq!((code, body["error"]["code"].as_str()), (404, Some("JOB_NOT_FOUND")));
    let (code, body) = s.call(Method::POST, "/projects/Demo%20Project/scenes/99/video/render", &[], None).await;
    assert_eq!((code, body["error"]["code"].as_str()), (404, Some("SCENE_NOT_FOUND")));
    let (_, body) = s.get("/queue").await;
    assert_eq!(body["data"]["gpu_running"], 0);
}

#[tokio::test]
async fn stitch_rejects_bad_requests_up_front_and_health_reports_comfy_down() {
    let s = start().await;
    let (code, body) = s.call(Method::POST, "/projects/Demo%20Project/stitch", &[], Some(json!({"bogus": 1}))).await;
    assert_eq!((code, body["error"]["code"].as_str()), (400, Some("VALIDATION_ERROR")));
    // No scene has a clip yet.
    let (code, body) = s.call(Method::POST, "/projects/Demo%20Project/stitch", &[], Some(json!({}))).await;
    assert_eq!((code, body["error"]["code"].as_str()), (400, Some("VALIDATION_ERROR")));
    let (_, body) = s.get("/health").await;
    assert_eq!(body["data"]["status"], "degraded");
    assert_eq!(body["data"]["comfy_up"], false);
    // The health answer says what this build can do, so an agent plans with it: the story steps, RefMod cards and the stems, and what is not here.
    let features = &body["data"]["features"];
    assert!(features["story_steps"].as_array().unwrap().iter().any(|s| s == "locations"), "{features}");
    assert!(features["refmod_cards"].is_string() && features["audio_stems"].is_string() && features["not_in_this_app"].is_array(), "{features}");
    let (_, body) = s.get("/meta").await;
    assert_eq!(body["data"]["api_version"], "v1");
    assert!(body["data"]["capabilities"].as_array().unwrap().iter().any(|c| c == "story"));
    assert_eq!(body["data"]["features"], *features);
}

#[tokio::test]
async fn events_stream_job_updates() {
    let s = start().await;
    let mut response = s.client.get(format!("{}/events", s.base)).send().await.unwrap();
    assert_eq!(response.status(), 200);
    let (_, body) = s.call(Method::POST, "/projects/Demo%20Project/scenes/1/video/render", &[], Some(json!({}))).await;
    let id = body["data"]["job_id"].as_str().unwrap().to_string();
    let mut seen = String::new();
    for _ in 0..50 {
        let chunk = tokio::time::timeout(Duration::from_secs(5), response.chunk()).await.unwrap().unwrap().unwrap();
        seen.push_str(&String::from_utf8_lossy(&chunk));
        if seen.contains("event: job.updated") && seen.contains(&id) && seen.contains("\"status\":\"failed\"") {
            return;
        }
    }
    panic!("no failed job event in: {seen}");
}
