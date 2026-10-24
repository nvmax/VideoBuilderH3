//! A scene's MiniMax references through the API: the picker data, choosing keys and handing the scene back to its mappings.

use std::time::Duration;

use h3_project::AppConfig;
use reqwest::{Client, Method};
use serde_json::{json, Value};

struct Server {
    base: String,
    client: Client,
    _dir: tempfile::TempDir,
}

async fn start() -> Server {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("projects");
    std::fs::create_dir_all(&root).unwrap();
    let cfg = AppConfig { comfy_url: "http://127.0.0.1:9".into(), project_root: root, ..AppConfig::default() };
    let state = h3_api::build_state(cfg, &dir.path().join("jobs.db")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, h3_api::router(state)).await.unwrap() });
    Server { base: format!("http://{addr}/api/v1"), client: Client::builder().timeout(Duration::from_secs(60)).build().unwrap(), _dir: dir }
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
}

#[tokio::test]
async fn references_are_chosen_per_scene() {
    let s = start().await;
    s.call(Method::POST, "/projects", Some(json!({"name": "Refs"}))).await;
    let p = "/projects/Refs";
    s.call(Method::POST, &format!("{p}/timeline/bulk"), Some(json!({"text": "4 a\n4 b", "mode": "durations"}))).await;
    s.call(Method::PUT, &format!("{p}/references/subjects/subj1"), Some(json!({"name": "Ava", "description": "a singer"}))).await;

    let (code, got) = s.call(Method::GET, &format!("{p}/scenes/1/minimax-references"), None).await;
    assert_eq!(code, 200, "{got}");
    assert_eq!(got["data"]["pipeline"], "standard");
    assert_eq!(got["data"]["scene_number"], 1);
    assert_eq!(got["data"]["custom"], false);
    assert!(got["data"]["limits"]["max_images"].as_i64().unwrap() >= 1);

    // Keys the scene cannot use are refused and say what is allowed.
    let (code, bad) = s.call(Method::PUT, &format!("{p}/scenes/1/minimax-references"), Some(json!({"keys": ["location:nope"]}))).await;
    assert_eq!((code, bad["error"]["code"].as_str()), (400, Some("VALIDATION_ERROR")), "{bad}");
    assert!(bad["error"]["message"].as_str().unwrap().contains("Not available for this scene: location:nope"));
    let (code, _) = s.call(Method::PUT, &format!("{p}/scenes/1/minimax-references"), Some(json!({"keys": ["a", "a"]}))).await;
    assert_eq!(code, 400);
    let (code, _) = s.call(Method::PUT, &format!("{p}/scenes/1/minimax-references"), Some(json!({"keys": "x"}))).await;
    assert_eq!(code, 400);
    let (code, _) = s.call(Method::PUT, &format!("{p}/scenes/1/minimax-references"), Some(json!({}))).await;
    assert_eq!(code, 400);
    let (code, _) = s.call(Method::PUT, &format!("{p}/scenes/1/minimax-references"), Some(json!({"keys": [], "automatic": true}))).await;
    assert_eq!(code, 400);

    // An empty chosen list is a valid custom order. Automatic hands the scene back.
    let (code, empty) = s.call(Method::PUT, &format!("{p}/scenes/1/minimax-references"), Some(json!({"keys": []}))).await;
    assert_eq!(code, 200, "{empty}");
    assert_eq!(empty["data"]["custom"], true);
    let (_, again) = s.call(Method::GET, &format!("{p}/scenes/1/minimax-references"), None).await;
    assert_eq!(again["data"]["custom"], true);
    let (code, auto) = s.call(Method::PUT, &format!("{p}/scenes/1/minimax-references"), Some(json!({"automatic": true}))).await;
    assert_eq!(code, 200, "{auto}");
    assert_eq!(auto["data"]["custom"], false);

    let (code, missing) = s.call(Method::GET, &format!("{p}/scenes/99/minimax-references"), None).await;
    assert_eq!((code, missing["error"]["code"].as_str()), (404, Some("SCENE_NOT_FOUND")));
}
