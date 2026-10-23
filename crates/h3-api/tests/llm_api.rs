//! LLM settings and prompt writing through the API, against a small local stand-in for the LLM server.

use std::time::Duration;

use axum::routing::post;
use axum::{Json, Router};
use h3_project::AppConfig;
use reqwest::{Client, Method};
use serde_json::{json, Value};

struct Server {
    base: String,
    client: Client,
    mock: String,
    dir: tempfile::TempDir,
}

fn session() -> Value {
    json!({
        "project_name": "Demo",
        "revision": 1,
        "video_engine": "minimax_h3",
        "segments": [
            {"id": "a", "start": 0.0, "end": 4.0, "label": "Scene 1", "lyric_text": "hold me close"},
            {"id": "b", "start": 4.0, "end": 8.0, "label": "Scene 2", "minimax_h3_prompt": "keep this one"}
        ],
        "flux_reference_builder": {
            "subjects": [{"id": "s", "name": "Ava", "description": "dark hair", "image": {"path": "C:\\refs\\ava.png"}}],
            "locations": [],
            "subject_scene_map": {"a": ["s"], "b": ["s"]},
            "use_subject_reference": true
        }
    })
}

async fn start() -> Server {
    // The stand-in LLM always answers one shot.
    let llm = Router::new().route(
        "/v1/chat/completions",
        post(|Json(_): Json<Value>| async { Json(json!({"choices": [{"message": {"content": "{\"shots\": [{\"description\": \"<Subject 1> (Ava) stands on a roof as wind lifts her jacket, the camera drifting in.\"}]}"}}]})) }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mock = format!("http://{}/v1", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, llm).await.unwrap() });

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("projects");
    let project = root.join("Demo");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("vrgdg_builder_session.json"), serde_json::to_string_pretty(&session()).unwrap()).unwrap();
    // Nothing listens on port 9 and 1234 is replaced by it, so LM Studio counts as down.
    let mut cfg = AppConfig { comfy_url: "http://127.0.0.1:9".into(), project_root: root, ..AppConfig::default() };
    cfg.llm.lm_studio_base_url = "http://127.0.0.1:9/v1".into();
    let state = h3_api::build_state(cfg, &dir.path().join("jobs.db")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, h3_api::router(state)).await.unwrap() });
    Server { base: format!("http://{addr}/api/v1"), client: Client::builder().timeout(Duration::from_secs(30)).build().unwrap(), mock, dir }
}

impl Server {
    async fn call(&self, method: Method, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut request = self.client.request(method, format!("{}{}", self.base, path));
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Value::Null))
    }

    async fn wait_job(&self, id: &str) -> Value {
        for _ in 0..200 {
            let (_, body) = self.call(Method::GET, &format!("/jobs/{id}"), None).await;
            if ["succeeded", "failed", "cancelled"].contains(&body["data"]["status"].as_str().unwrap_or("")) {
                return body["data"].clone();
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("job {id} did not finish");
    }
}

#[tokio::test]
async fn settings_are_checked_saved_and_never_show_keys() {
    let s = start().await;
    let (code, body) = s.call(Method::GET, "/llm/settings", None).await;
    assert_eq!((code, body["data"]["runner"].as_str()), (200, Some("lm_studio")));
    let (code, body) = s.call(Method::PUT, "/llm/settings", Some(json!({"runner": "nope"}))).await;
    assert_eq!((code, body["error"]["code"].as_str()), (400, Some("VALIDATION_ERROR")));
    let (code, _) = s.call(Method::PUT, "/llm/settings", Some(json!({"no_such_setting": 1}))).await;
    assert_eq!(code, 400);
    let (code, _) = s.call(Method::PUT, "/llm/settings", Some(json!({"lm_studio_context_limit": "big"}))).await;
    assert_eq!(code, 400);
    let (code, body) = s.call(Method::PUT, "/llm/settings", Some(json!({"runner": "own_server", "own_server_url": s.mock, "own_server_model": "m", "own_server_api_key": "secret"}))).await;
    assert_eq!(code, 200, "{body}");
    assert_eq!((body["data"]["own_server_api_key"].as_str(), body["data"]["own_server_api_key_set"].as_bool()), (Some(""), Some(true)));
    // The change is in the config file next to the job database.
    let saved = std::fs::read_to_string(s.dir.path().join("config.json")).unwrap();
    assert!(saved.contains("own_server") && saved.contains("secret"));
}

#[tokio::test]
async fn lm_studio_down_is_a_clear_unavailable_error() {
    let s = start().await;
    let (code, body) = s.call(Method::GET, "/llm/active", None).await;
    assert_eq!((code, body["error"]["code"].as_str()), (503, Some("LLM_UNAVAILABLE")));
    assert_eq!(body["error"]["retryable"], true);
    let (code, body) = s.call(Method::POST, "/llm/test", None).await;
    assert_eq!((code, body["error"]["code"].as_str()), (503, Some("LLM_UNAVAILABLE")));
    let (code, body) = s.call(Method::POST, "/projects/Demo/minimax-prompts", Some(json!({}))).await;
    assert_eq!(code, 202);
    let job = s.wait_job(body["data"]["job_id"].as_str().unwrap()).await;
    assert_eq!((job["status"].as_str(), job["error"]["code"].as_str()), (Some("failed"), Some("LLM_UNAVAILABLE")));
}

#[tokio::test]
async fn prompts_are_written_for_scenes_without_one_and_saved_on_the_scene() {
    let s = start().await;
    s.call(Method::PUT, "/llm/settings", Some(json!({"runner": "own_server", "own_server_url": s.mock, "own_server_model": "m"}))).await;
    let (_, test) = s.call(Method::POST, "/llm/test", None).await;
    assert_eq!(test["data"]["ok"], true, "{test}");
    let (code, body) = s.call(Method::POST, "/projects/Demo/minimax-prompts", Some(json!({}))).await;
    assert_eq!(code, 202, "{body}");
    let job = s.wait_job(body["data"]["job_id"].as_str().unwrap()).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    assert_eq!((job["result"]["created"].as_i64(), job["result"]["skipped"].as_i64()), (Some(1), Some(1)));
    let (_, scenes) = s.call(Method::GET, "/projects/Demo/scenes", None).await;
    let first = scenes["data"][0]["minimax_h3_prompt"].as_str().unwrap();
    assert!(first.contains("[Shot 1]") && first.contains("<Subject 1>") && first.contains("Hold me close") && first.chars().count() <= 7000, "{first}");
    assert_eq!(scenes["data"][1]["minimax_h3_prompt"], "keep this one", "a scene that has a prompt is left alone");
    // Asking for the scene by name writes it again.
    let (_, body) = s.call(Method::POST, "/projects/Demo/minimax-prompts", Some(json!({"scene_ids": ["b"]}))).await;
    let job = s.wait_job(body["data"]["job_id"].as_str().unwrap()).await;
    assert_eq!(job["result"]["created"], 1, "{job}");
    let (_, scenes) = s.call(Method::GET, "/projects/Demo/scenes/2", None).await;
    assert_ne!(scenes["data"]["minimax_h3_prompt"], "keep this one");
}
