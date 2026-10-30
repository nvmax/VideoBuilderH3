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
    /// The server's state, to put job handlers of the test in place of the real ones.
    state: std::sync::Arc<h3_api::AppState>,
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
    let served = state.clone();
    tokio::spawn(async move { axum::serve(listener, h3_api::router(served)).await.unwrap() });
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
    Server { root, client, events, state, _dir: dir }
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
    // The window numbers its next save one above the number it loaded.
    session["builder_save_revision"] = json!(session["builder_save_revision"].as_i64().unwrap() + 1);
    s.drain().await;
    let saved: Value = s.client.post(format!("{}/vrgdg/music_builder/save_session", s.root)).json(&json!({"project_folder": made["data"]["project_folder"], "session": session})).send().await.unwrap().json().await.unwrap();
    assert_eq!(saved["stale"], false, "{saved}");
    let after = s.drain().await;
    assert!(!after.iter().any(is("vrgdg.project_changed")), "{after:?}");
}

#[tokio::test]
async fn a_transition_effect_or_filter_an_agent_adds_is_announced_as_its_lane_and_not_as_the_project() {
    let mut s = start().await;
    s.tool("project_create", json!({"project_name": "Demo"})).await;
    s.tool("timeline_build", json!({"project_id": "Demo", "text": "4 first line\n4 second line"})).await;
    s.drain().await;
    // The Builder merges a lane list without a reload, so a render running in its window cannot hold the block back. A change to `project` would wait for it.
    let api = format!("{}/api/v1/projects/Demo", s.root);
    let cases = [
        ("transitions/set", json!({"transition": "BlockDissolve", "start": 3.5, "seconds": 1.0}), "scene_transitions"),
        ("effects/set", json!({"look": "BeatBounce", "start": 0.5, "seconds": 2.0}), "video_effects"),
        ("filters/set", json!({"look": "Adjust", "start": 0.0, "seconds": 3.0}), "video_filters"),
    ];
    let mut revision = 0;
    for (path, body, key) in cases {
        let answer: Value = s.client.post(format!("{api}/{path}")).json(&body).send().await.unwrap().json().await.unwrap();
        assert_eq!(answer["ok"], true, "{answer}");
        let event = s.next(is("vrgdg.project_changed")).await;
        assert_eq!(event["data"]["change"], json!({"kind": "scene_fields", "scenes": {}, "lists": [key]}), "{key}: {event}");
        let number = event["data"]["revision"].as_i64().unwrap();
        assert!(number > revision, "revisions follow each other: {number} after {revision}");
        revision = number;
    }
    // Taking the last block out of a lane is announced too.
    let cleared: Value = s.client.put(format!("{api}/transitions")).json(&json!({"scene_transitions": []})).send().await.unwrap().json().await.unwrap();
    assert_eq!(cleared["ok"], true, "{cleared}");
    let event = s.next(is("vrgdg.project_changed")).await;
    assert_eq!(event["data"]["change"]["lists"], json!(["scene_transitions"]), "{event}");
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

    // A read is listed as a read, which the Builder dims. A call that only waits or checks (job_get) is not listed. A failure says why.
    s.tool("scene_list", json!({"project_id": "Demo Song"})).await;
    s.tool("job_get", json!({"job_id": "nothing"})).await;
    s.tool("scene_update", json!({"project_id": "Demo Song", "scene_id": "nope", "patch": {"label": "x"}})).await;
    let later = s.drain().await;
    assert!(!later.iter().any(|e| e["data"]["tool"] == "job_get"), "{later:?}");
    let read = later.iter().find(|e| e["data"]["tool"] == "scene_list" && e["data"]["status"] == "done").expect("a read is listed");
    assert_eq!((read["data"]["kind"].clone(), read["data"]["api"].clone()), (json!("read"), json!("GET /projects/Demo%20Song/scenes")), "{read}");
    let failed = later.iter().find(|e| e["data"]["tool"] == "scene_update" && e["data"]["status"] == "failed").expect("the failed call is listed");
    assert!(failed["data"]["message"].as_str().unwrap().contains("SCENE_NOT_FOUND"), "{failed}");

    // A program that calls the REST API is listed. The Builder's own call (it sends an Origin) is not.
    s.client.patch(format!("{}/api/v1/projects/Demo%20Song/scenes/{scene}", s.root)).json(&json!({"label": "From a script"})).send().await.unwrap();
    // It is listed while it runs (so what it saves is attributed to it) and again when it is done.
    let started = s.next(|e| is("vrgdg.agent_activity")(e) && e["data"]["source"] == "api").await;
    assert_eq!(started["data"]["status"], "running", "{started}");
    let rest = s.next(|e| is("vrgdg.agent_activity")(e) && e["data"]["source"] == "api" && e["data"]["status"] == "done").await;
    assert_eq!(rest["data"]["id"], started["data"]["id"], "one entry");
    assert_eq!((rest["data"]["scene_id"].clone(), rest["data"]["project_id"].clone(), rest["data"]["status"].clone()), (json!(scene), json!("Demo Song"), json!("done")));
    assert_eq!(rest["data"]["api"], json!(format!("PATCH /projects/Demo%20Song/scenes/{scene}")), "the call it made is named");
    assert!(rest["data"]["touched"].as_array().is_some_and(|t| t.len() == 1 && t[0]["scene_id"] == json!(scene)), "what the call saved: {rest}");
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

impl Server {
    async fn initialize(&self, name: &str, version: &str) {
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": name, "version": version}}});
        let answer: Value = self.client.post(format!("{}/mcp", self.root)).json(&body).send().await.unwrap().json().await.unwrap();
        assert_eq!(answer["result"]["serverInfo"]["title"], "VRGDG H3 Studio", "{answer}");
    }

    async fn backlog(&self) -> Value {
        let body: Value = self.client.get(format!("{}/api/v1/agent/activity?after=0", self.root)).send().await.unwrap().json().await.unwrap();
        body["data"].clone()
    }
}

#[tokio::test]
async fn an_agent_that_connects_is_announced_once() {
    let mut s = start().await;
    s.initialize("claude-code", "2.1.0").await;
    let hello = s.next(|e| is("vrgdg.agent_activity")(e) && e["data"]["kind"] == "connect").await;
    assert_eq!((hello["data"]["title"].clone(), hello["data"]["status"].clone()), (json!("Agent connected"), json!("done")), "{hello}");
    assert!(hello["data"]["detail"].as_str().unwrap().starts_with("claude-code 2.1.0 over MCP"), "{hello}");
    assert_eq!(hello["data"]["client"], "claude-code 2.1.0");

    // A client that says hello with every call is one connection. Another client is another.
    s.initialize("claude-code", "2.1.0").await;
    assert!(!s.drain().await.iter().any(|e| e["data"]["kind"] == "connect"), "the same client twice in a minute is announced once");
    s.initialize("codex", "0.9.0").await;
    let second = s.next(|e| is("vrgdg.agent_activity")(e) && e["data"]["kind"] == "connect").await;
    assert_eq!(second["data"]["client"], "codex 0.9.0");
}

#[tokio::test]
async fn a_call_says_which_api_it_ran_what_it_answered_and_what_it_touched() {
    let mut s = start().await;
    s.tool("project_create", json!({"project_name": "Demo Song"})).await;
    s.tool("timeline_build", json!({"project_id": "Demo Song", "text": "4 first line\n4 second line"})).await;
    let scenes = s.tool("scene_list", json!({"project_id": "Demo Song"})).await;
    let scene = scenes[0]["id"].as_str().unwrap().to_string();
    s.drain().await;

    s.tool("scene_update", json!({"project_id": "Demo Song", "scene_id": scene, "patch": {"label": "Opening", "minimax_h3_prompt": "A singer on a roof."}})).await;
    s.drain().await;
    let backlog = s.backlog().await;
    let items = backlog["items"].as_array().unwrap();
    let update = items.iter().find(|e| e["tool"] == "scene_update").expect("the call is listed");
    assert_eq!(update["kind"], "write", "{update}");
    assert_eq!(update["api"], json!(format!("PATCH /projects/Demo%20Song/scenes/{scene}")), "{update}");
    assert!(update["about"].as_str().is_some_and(|a| !a.is_empty()), "what the endpoint does: {update}");
    assert!(update["result"].as_str().is_some_and(|r| !r.is_empty()), "what it answered: {update}");
    // The save made while the call ran is listed on it, by scene and field.
    let touched = update["touched"].as_array().expect("what it touched");
    assert_eq!(touched.len(), 1, "{touched:?}");
    assert_eq!(touched[0]["scene_id"], json!(scene));
    let fields: Vec<&str> = touched[0]["fields"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert!(fields.contains(&"label") && fields.contains(&"minimax_h3_prompt"), "{fields:?}");

    // A read has no list of what it touched, and an entry that ended does not grow when something else saves the project.
    let read = items.iter().find(|e| e["tool"] == "scene_list").expect("the read is listed");
    assert!(read["touched"].is_null(), "{read}");
    s.tool("scene_update", json!({"project_id": "Demo Song", "scene_id": scene, "patch": {"notes": "later"}})).await;
    s.drain().await;
    let after = s.backlog().await;
    let first = after["items"].as_array().unwrap().iter().find(|e| e["id"] == update["id"]).unwrap();
    assert_eq!(first["touched"], update["touched"], "the first call is over and keeps what it touched");
}

fn fake_render(release: std::sync::Arc<tokio::sync::Notify>, outcome: Result<(), &'static str>) -> h3_jobs::Handler {
    std::sync::Arc::new(move |ctx: h3_jobs::JobCtx| {
        let release = release.clone();
        Box::pin(async move {
            let say = |stage: &str, percent: f64, message: &str| {
                ctx.progress(h3_jobs::ProgressUpdate { percent, stage: stage.into(), message: message.into(), scene_id: Some("seg_7".into()), stage_count: 1, ..Default::default() });
            };
            say("preparing", 5.0, "Preparing scene seg_7...");
            say("rendering", 40.0, "Rendering with ComfyUI - Pass 1 sampler step 12 of 30 - 0:20 elapsed");
            release.notified().await;
            match outcome {
                Ok(()) => {
                    say("completed", 100.0, "Scene video rendered successfully.");
                    Ok(json!({}))
                }
                Err(message) => Err(h3_jobs::JobError::new("RENDER_FAILED", message)),
            }
        }) as std::pin::Pin<Box<dyn std::future::Future<Output = h3_jobs::JobResult> + Send>>
    })
}

#[tokio::test]
async fn a_scene_a_job_renders_is_told_to_the_builder_until_the_scene_is_done() {
    let release = std::sync::Arc::new(tokio::sync::Notify::new());
    let mut s = start().await;
    s.state.jobs.register("video.render", fake_render(release.clone(), Ok(())));
    let job = s.state.jobs.submit("video.render", Some("Demo Song"), json!({"scene_id": "seg_7"}), true).unwrap();

    let on = s.next(|e| is("vrgdg.scene_work")(e) && e["data"]["state"] == "on" && e["data"]["stage"] == "rendering").await;
    assert_eq!((on["data"]["scene_id"].clone(), on["data"]["what"].clone(), on["data"]["project_id"].clone(), on["data"]["job_id"].clone()), (json!("seg_7"), json!("video"), json!("Demo Song"), json!(job.id)), "{on}");
    assert_eq!(on["data"]["percent"], 40.0, "a one scene job says how far it is");
    assert!(on["data"]["message"].as_str().unwrap().contains("step 12 of 30"), "{on}");

    // A window that opens now still learns that the scene is being rendered, and sees the job with its percent and scene.
    let backlog = s.backlog().await;
    assert_eq!(backlog["scenes_in_work"].as_array().map(Vec::len), Some(1), "{backlog}");
    assert_eq!(backlog["scenes_in_work"][0]["scene_id"], "seg_7");
    let entry = backlog["items"].as_array().unwrap().iter().find(|e| e["source"] == "job").expect("the job is listed");
    assert_eq!((entry["percent"].clone(), entry["scene_id"].clone(), entry["stage"].clone()), (json!(40.0), json!("seg_7"), json!("rendering")), "{entry}");

    release.notify_one();
    let off = s.next(|e| is("vrgdg.scene_work")(e) && e["data"]["state"] == "off").await;
    assert_eq!((off["data"]["scene_id"].clone(), off["data"]["ok"].clone()), (json!("seg_7"), json!(true)), "{off}");
    assert_eq!(s.backlog().await["scenes_in_work"], json!([]));
}

#[tokio::test]
async fn a_job_that_fails_while_rendering_a_scene_says_the_scene_did_not_finish() {
    let release = std::sync::Arc::new(tokio::sync::Notify::new());
    let mut s = start().await;
    s.state.jobs.register("video.render", fake_render(release.clone(), Err("ComfyUI ran out of memory")));
    s.state.jobs.submit("video.render", Some("Demo Song"), json!({"scene_id": "seg_7"}), true).unwrap();
    s.next(|e| is("vrgdg.scene_work")(e) && e["data"]["state"] == "on").await;
    release.notify_one();
    let off = s.next(|e| is("vrgdg.scene_work")(e) && e["data"]["state"] == "off").await;
    assert_eq!((off["data"]["scene_id"].clone(), off["data"]["ok"].clone()), (json!("seg_7"), json!(false)), "{off}");
}

#[tokio::test]
async fn a_job_that_only_sets_a_marker_percent_does_not_show_it_as_progress() {
    let release = std::sync::Arc::new(tokio::sync::Notify::new());
    let mut s = start().await;
    let waiting = release.clone();
    // One call to an LLM sets 20% and nothing more. That number is not how far the call is, which is what looked stuck.
    s.state.jobs.register(
        "llm.enhance_prompt",
        std::sync::Arc::new(move |ctx: h3_jobs::JobCtx| {
            let waiting = waiting.clone();
            Box::pin(async move {
                ctx.progress(h3_jobs::ProgressUpdate { percent: 20.0, stage: "enhancing".into(), message: "Enhancing the prompt...".into(), stage_count: 1, ..Default::default() });
                waiting.notified().await;
                Ok(json!({}))
            }) as std::pin::Pin<Box<dyn std::future::Future<Output = h3_jobs::JobResult> + Send>>
        }),
    );
    s.state.jobs.submit("llm.enhance_prompt", Some("Demo Song"), json!({}), false).unwrap();
    let running = s.next(|e| is("vrgdg.agent_activity")(e) && e["data"]["source"] == "job" && e["data"]["stage"] == "enhancing").await;
    assert_eq!(running["data"]["message"], "Enhancing the prompt...");
    assert!(running["data"]["percent"].is_null(), "{running}");
    release.notify_one();
}
