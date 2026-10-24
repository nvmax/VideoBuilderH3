//! What the open Video Builder is told while an agent works: every change an agent makes to a project (so the Builder can show it), and a feed of what the
//! agent is doing (MCP tool calls, REST calls from programs, jobs). The Builder reads both from the event stream `/vrgdg/events`.

use std::time::Duration;

use h3_llm::LlmConfig;
use h3_project::AppConfig;
use reqwest::Client;
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedReceiver;

struct Server {
    root: String,
    client: Client,
    events: UnboundedReceiver<Value>,
    _dir: tempfile::TempDir,
}

/// The server, and a window listening to its event stream.
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
    let root = format!("http://{addr}");
    let client = Client::builder().timeout(Duration::from_secs(60)).build().unwrap();
    let (sender, events) = tokio::sync::mpsc::unbounded_channel();
    let mut stream = Client::new().get(format!("{root}/vrgdg/events")).send().await.unwrap();
    tokio::spawn(async move {
        let mut buffer = String::new();
        while let Ok(Some(chunk)) = stream.chunk().await {
            buffer.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(end) = buffer.find("\n\n") {
                let block: String = buffer.drain(..end + 2).collect();
                for line in block.lines() {
                    if let Some(data) = line.strip_prefix("data:") {
                        if let Ok(value) = serde_json::from_str::<Value>(data.trim()) {
                            let _ = sender.send(value);
                        }
                    }
                }
            }
        }
    });
    Server { root, client, events, _dir: dir }
}

impl Server {
    async fn tool(&self, name: &str, args: Value) -> Value {
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": args}});
        let answer: Value = self.client.post(format!("{}/mcp", self.root)).json(&body).send().await.unwrap().json().await.unwrap();
        let text = answer["result"]["content"][0]["text"].as_str().unwrap_or("").to_string();
        serde_json::from_str(&text).unwrap_or(Value::String(text))
    }

    /// The next event that `wanted` accepts, within a few seconds. Events before it are dropped.
    async fn next(&mut self, wanted: impl Fn(&Value) -> bool) -> Value {
        let found = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let event = self.events.recv().await.expect("the event stream ended");
                if wanted(&event) {
                    return event;
                }
            }
        })
        .await;
        found.expect("the event did not arrive")
    }

    /// Every event that has arrived by now.
    async fn drain(&mut self) -> Vec<Value> {
        tokio::time::sleep(Duration::from_millis(400)).await;
        let mut all = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            all.push(event);
        }
        all
    }
}

fn is(kind: &'static str) -> impl Fn(&Value) -> bool {
    move |event| event["type"] == kind
}

#[tokio::test]
async fn the_builder_is_told_about_every_change_an_agent_makes_and_about_nothing_else() {
    let mut s = start().await;
    s.tool("project_create", json!({"project_name": "Demo Song"})).await;
    s.tool("timeline_build", json!({"project_id": "Demo Song", "text": "4 first line\n4 second line"})).await;
    // Creating the project and making scenes change the project as a whole: the Builder reloads it when it can. Each save has its own event.
    let events = s.drain().await;
    let changes: Vec<&Value> = events.iter().filter(|e| is("vrgdg.project_changed")(e)).collect();
    assert_eq!(changes.len(), 2, "{changes:?}");
    assert!(changes.iter().all(|e| e["data"]["change"]["kind"] == "project" && e["data"]["source"] == "agent_api"), "{changes:?}");
    assert_eq!(changes[1]["data"]["revision"].as_i64().unwrap(), changes[0]["data"]["revision"].as_i64().unwrap() + 1);
    let made = changes[1].clone();
    assert!(made["data"]["project_folder"].as_str().unwrap().to_lowercase().ends_with("demo song"), "{made}");
    let revision = made["data"]["revision"].as_i64().unwrap();
    let scenes = s.tool("scene_list", json!({"project_id": "Demo Song"})).await;
    let scene = scenes[0]["id"].as_str().unwrap().to_string();
    s.drain().await;

    // A scene edit names the scene and the fields, and carries the next revision, so the Builder can merge it without a reload.
    s.tool("scene_update", json!({"project_id": "Demo Song", "scene_id": scene, "patch": {"label": "Opening", "minimax_h3_prompt": "A singer on a roof."}})).await;
    let edit = s.next(is("vrgdg.project_changed")).await;
    assert_eq!(edit["data"]["change"]["kind"], "scene_fields", "{edit}");
    let fields = edit["data"]["change"]["scenes"][&scene]["segment"].as_array().unwrap().iter().filter_map(Value::as_str).collect::<Vec<_>>();
    assert!(fields.contains(&"label") && fields.contains(&"minimax_h3_prompt"), "{fields:?}");
    assert_eq!(edit["data"]["revision"].as_i64().unwrap(), revision + 1, "revisions follow each other, which the Builder's merge needs");

    // A Timeline Note is named by its id.
    s.tool("timeline_note_create", json!({"project_id": "Demo Song", "id": "n1", "start": 1.0, "end": 3.0, "label": "Intro"})).await;
    let note = s.next(is("vrgdg.project_changed")).await;
    assert_eq!(note["data"]["change"]["marker_ids"], json!(["n1"]), "{note}");

    // The Video Builder's own save is not announced to itself.
    let loaded: Value = s.client.post(format!("{}/vrgdg/music_builder/load_session", s.root)).json(&json!({"project_folder": made["data"]["project_folder"]})).send().await.unwrap().json().await.unwrap();
    let mut session = loaded["session"].clone();
    session["segments"][0]["label"] = json!("Typed in the window");
    session["builder_save_revision"] = json!(session["revision"].as_i64().unwrap() + 1);
    s.drain().await;
    let saved: Value = s.client.post(format!("{}/vrgdg/music_builder/save_session", s.root)).json(&json!({"project_folder": made["data"]["project_folder"], "session": session})).send().await.unwrap().json().await.unwrap();
    assert_eq!(saved["stale"], false, "{saved}");
    let after = s.drain().await;
    assert!(!after.iter().any(is("vrgdg.project_changed")), "{after:?}");
}

#[tokio::test]
async fn the_feed_lists_what_the_agent_does_while_it_does_it() {
    let mut s = start().await;
    s.tool("project_create", json!({"project_name": "Demo Song"})).await;
    // A change: shown as running, then done, as one entry.
    let running = s.next(|e| is("vrgdg.agent_activity")(e) && e["data"]["tool"] == "project_create").await;
    assert_eq!((running["data"]["source"].clone(), running["data"]["title"].clone(), running["data"]["status"].clone()), (json!("mcp"), json!("Project create"), json!("running")));
    let done = s.next(|e| is("vrgdg.agent_activity")(e) && e["data"]["id"] == running["data"]["id"] && e["data"]["status"] == "done").await;
    assert!(done["data"]["duration_ms"].is_number() && done["data"]["project_id"] == "Demo Song", "{done}");

    s.tool("timeline_build", json!({"project_id": "Demo Song", "text": "4 first line\n4 second line"})).await;
    let scenes = s.tool("scene_list", json!({"project_id": "Demo Song"})).await;
    let scene = scenes[0]["id"].as_str().unwrap().to_string();
    s.drain().await;

    // What the call was told is summarized, and what is private is not shown.
    s.tool("scene_update", json!({"project_id": "Demo Song", "scene_id": scene, "patch": {"label": "Opening"}})).await;
    let update = s.next(|e| is("vrgdg.agent_activity")(e) && e["data"]["tool"] == "scene_update").await;
    assert_eq!((update["data"]["scene_id"].clone(), update["data"]["project_id"].clone()), (json!(scene), json!("Demo Song")));
    assert!(update["data"]["detail"].as_str().unwrap().contains("patch: {label}"), "{update}");
    s.tool("api_put_elevenlabs", json!({"project_id": "Demo Song", "body": {"api_key": "sk-secret-value"}})).await;
    let everything = s.drain().await;
    assert!(!everything.iter().any(|e| e.to_string().contains("sk-secret-value")), "a key must not reach the feed");

    // Reads do not fill the feed. A failure says why.
    s.tool("scene_list", json!({"project_id": "Demo Song"})).await;
    s.tool("job_get", json!({"job_id": "nothing"})).await;
    s.tool("scene_update", json!({"project_id": "Demo Song", "scene_id": "nope", "patch": {"label": "x"}})).await;
    let later = s.drain().await;
    assert!(!later.iter().any(|e| e["data"]["tool"] == "scene_list" || e["data"]["tool"] == "job_get"), "{later:?}");
    let failed = later.iter().find(|e| e["data"]["tool"] == "scene_update" && e["data"]["status"] == "failed").expect("the failed call is listed");
    assert!(failed["data"]["message"].as_str().unwrap().contains("SCENE_NOT_FOUND"), "{failed}");

    // A program that calls the REST API is listed. The Builder's own call (it sends an Origin) is not.
    s.client.patch(format!("{}/api/v1/projects/Demo%20Song/scenes/{scene}", s.root)).json(&json!({"label": "From a script"})).send().await.unwrap();
    let rest = s.next(|e| is("vrgdg.agent_activity")(e) && e["data"]["source"] == "api").await;
    assert_eq!((rest["data"]["scene_id"].clone(), rest["data"]["project_id"].clone(), rest["data"]["status"].clone()), (json!(scene), json!("Demo Song"), json!("done")));
    s.drain().await;
    s.client.patch(format!("{}/api/v1/projects/Demo%20Song/scenes/{scene}", s.root)).header("origin", s.root.as_str()).json(&json!({"label": "From the window"})).send().await.unwrap();
    assert!(!s.drain().await.iter().any(|e| e["data"]["source"] == "api"));

    // A job is one entry that goes on to its end.
    let started = s.tool("minimax_prompts", json!({"project_id": "Demo Song"})).await;
    let job = s.next(|e| is("vrgdg.agent_activity")(e) && e["data"]["source"] == "job" && e["data"]["status"] == "failed").await;
    assert_eq!((job["data"]["job_id"].clone(), job["data"]["title"].clone()), (started["job_id"].clone(), json!("Writing MiniMax prompts")));

    // A window that opens later asks for what it missed.
    let backlog: Value = s.client.get(format!("{}/api/v1/agent/activity?after=0", s.root)).send().await.unwrap().json().await.unwrap();
    let items = backlog["data"]["items"].as_array().unwrap();
    assert!(items.len() >= 5 && items.windows(2).all(|w| w[0]["id"].as_u64() < w[1]["id"].as_u64()), "{backlog}");
    let last = backlog["data"]["last_id"].as_u64().unwrap();
    let none: Value = s.client.get(format!("{}/api/v1/agent/activity?after={last}", s.root)).send().await.unwrap().json().await.unwrap();
    assert_eq!(none["data"]["items"], json!([]));
}
