//! The project the app's window has open: the window reports it, and an agent learns it when it connects and by asking.

use std::time::Duration;

use h3_api::open_project::OPEN_PROJECT_TOOL;
use h3_llm::LlmConfig;
use h3_project::AppConfig;
use reqwest::Client;
use serde_json::{json, Value};

struct Server {
    root: String,
    client: Client,
    dir: tempfile::TempDir,
}

async fn start() -> Server {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("projects");
    std::fs::create_dir_all(&root).unwrap();
    let llm = LlmConfig { lm_studio_base_url: "http://127.0.0.1:9".into(), own_server_url: "http://127.0.0.1:9".into(), ..LlmConfig::default() };
    let cfg = AppConfig { comfy_url: "http://127.0.0.1:9".into(), project_root: root, llm, ..AppConfig::default() };
    let state = h3_api::build_state(cfg, &dir.path().join("jobs.db")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, h3_api::router(state)).await.unwrap() });
    Server { root: format!("http://{addr}"), client: Client::builder().timeout(Duration::from_secs(60)).build().unwrap(), dir }
}

impl Server {
    async fn rpc(&self, method: &str, params: Value) -> Value {
        let response = self.client.post(format!("{}/mcp", self.root)).json(&json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})).send().await.unwrap();
        assert_eq!(response.status(), 200, "{method}");
        response.json().await.unwrap()
    }

    /// The instructions an agent gets when it connects.
    async fn instructions(&self) -> String {
        let init = self.rpc("initialize", json!({"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}})).await;
        init["result"]["instructions"].as_str().unwrap().to_string()
    }

    /// What the tool answers with.
    async fn asked(&self) -> Value {
        let answer = self.rpc("tools/call", json!({"name": OPEN_PROJECT_TOOL, "arguments": {}})).await;
        assert_eq!(answer["result"]["isError"], false, "{answer}");
        serde_json::from_str(answer["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
    }

    /// The window says what it has open, the way the page does (a browser sends an Origin).
    async fn report(&self, folder: &str) -> Value {
        let response = self.client.put(format!("{}/api/v1/app/open-project", self.root)).header("origin", "http://127.0.0.1:8190").json(&json!({"project_folder": folder})).send().await.unwrap();
        assert_eq!(response.status(), 200);
        response.json::<Value>().await.unwrap()["data"].clone()
    }

    fn folder(&self, id: &str) -> String {
        self.dir.path().join("projects").join(id).display().to_string()
    }
}

#[tokio::test]
async fn an_agent_is_told_which_project_the_window_has_open() {
    let s = start().await;
    assert!(s.instructions().await.contains("No project is open in the app right now"));
    assert_eq!(s.asked().await, json!({"open": false, "project_id": null, "project_folder": null}));

    s.rpc("tools/call", json!({"name": "project_create", "arguments": {"project_name": "Demo Song"}})).await;
    let said = s.report(&s.folder("Demo Song")).await;
    assert_eq!((said["open"].clone(), said["project_id"].clone()), (json!(true), json!("Demo Song")));

    // At connection the instructions name it, and the tool answers the same.
    let text = s.instructions().await;
    assert!(text.contains(r#"is "Demo Song""#) && text.contains(OPEN_PROJECT_TOOL), "{text}");
    let asked = s.asked().await;
    assert_eq!((asked["open"].clone(), asked["project_id"].clone()), (json!(true), json!("Demo Song")));
    assert!(asked["project_folder"].as_str().unwrap().ends_with("Demo Song"));

    // The tool the instructions name is one the server offers.
    let tools = s.rpc("tools/list", json!({})).await;
    assert!(tools["result"]["tools"].as_array().unwrap().iter().any(|t| t["name"] == OPEN_PROJECT_TOOL));
}

#[tokio::test]
async fn only_a_project_the_agent_api_can_find_counts_as_open() {
    let s = start().await;
    s.rpc("tools/call", json!({"name": "project_create", "arguments": {"project_name": "Demo Song"}})).await;
    // A project that was never saved, a path with a trailing slash and the other kind of slash.
    assert_eq!(s.report(&s.folder("Not Saved Yet")).await["open"], false);
    assert_eq!(s.report(&format!("{}/", s.folder("Demo Song"))).await["project_id"], "Demo Song");
    assert_eq!(s.report(&s.folder("Demo Song").replace('\\', "/")).await["project_id"], "Demo Song");
    // Nothing open, and a name that tries to leave the project folder.
    assert_eq!(s.report("").await["open"], false);
    assert_eq!(s.report("../../outside").await["open"], false);
    // A project deleted after the window opened it is no longer open.
    s.report(&s.folder("Demo Song")).await;
    std::fs::remove_dir_all(s.folder("Demo Song")).unwrap();
    assert_eq!(s.asked().await["open"], false);
    assert!(s.instructions().await.contains("No project is open"));
}

#[tokio::test]
async fn the_windows_report_is_not_listed_as_something_an_agent_did() {
    let s = start().await;
    s.rpc("tools/call", json!({"name": "project_create", "arguments": {"project_name": "Demo Song"}})).await;
    s.report(&s.folder("Demo Song")).await;
    let feed: Value = s.client.get(format!("{}/api/v1/agent/activity?after=0", s.root)).send().await.unwrap().json().await.unwrap();
    let listed = feed["data"]["items"].as_array().unwrap().iter().any(|e| e["tool"].as_str().is_some_and(|t| t.contains("open-project")));
    assert!(!listed, "{feed}");
}
