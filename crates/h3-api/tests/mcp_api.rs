//! The MCP server built into the app, spoken to over HTTP like an agent would: the protocol, the origin guard, every tool reaching a route that exists,
//! a project built with the named tools, errors an agent can act on, resources and prompts.

use std::time::Duration;

use base64::Engine;
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
    // Nothing here may reach a real ComfyUI or LLM: every address is a port nothing listens on.
    let llm = LlmConfig { lm_studio_base_url: "http://127.0.0.1:9".into(), own_server_url: "http://127.0.0.1:9".into(), ..LlmConfig::default() };
    let cfg = AppConfig { comfy_url: "http://127.0.0.1:9".into(), project_root: root, llm, ..AppConfig::default() };
    let state = h3_api::build_state(cfg, &dir.path().join("jobs.db")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, h3_api::router(state)).await.unwrap() });
    Server { root: format!("http://{addr}"), client: Client::builder().timeout(Duration::from_secs(120)).build().unwrap(), dir }
}

impl Server {
    async fn rpc(&self, method: &str, params: Value) -> Value {
        let response = self.client.post(format!("{}/mcp", self.root)).json(&json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})).send().await.unwrap();
        assert_eq!(response.status(), 200, "{method}");
        response.json().await.unwrap()
    }

    /// A tool call: whether it failed, the text it answered with, and the whole result.
    async fn tool(&self, name: &str, args: Value) -> (bool, String, Value) {
        let answer = self.rpc("tools/call", json!({"name": name, "arguments": args})).await;
        let result = answer["result"].clone();
        let text = result["content"].as_array().and_then(|c| c.iter().find(|i| i["type"] == "text")).and_then(|i| i["text"].as_str()).unwrap_or("").to_string();
        (result["isError"].as_bool().unwrap_or(true), text, result)
    }

    async fn ok(&self, name: &str, args: Value) -> Value {
        let (failed, text, _) = self.tool(name, args).await;
        assert!(!failed, "{name}: {text}");
        serde_json::from_str(&text).unwrap_or(Value::String(text))
    }
}

#[tokio::test]
async fn the_protocol_is_spoken_and_only_this_computer_may_call() {
    let s = start().await;
    let init = s.rpc("initialize", json!({"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}})).await;
    assert_eq!((init["result"]["protocolVersion"].clone(), init["result"]["serverInfo"]["name"].clone()), (json!("2025-03-26"), json!("vrgdg-h3-studio")));
    assert!(init["result"]["instructions"].as_str().unwrap().contains("job_wait"));
    let newest = s.rpc("initialize", json!({"protocolVersion": "1999-01-01"})).await;
    assert_eq!(newest["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(s.rpc("ping", json!({})).await["result"], json!({}));
    assert_eq!(s.rpc("nothing/here", json!({})).await["error"]["code"], -32601);

    let url = format!("{}/mcp", s.root);
    let note = s.client.post(&url).json(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).send().await.unwrap();
    assert_eq!(note.status(), 202);
    let batch = s.client.post(&url).json(&json!([{"jsonrpc": "2.0", "id": 1, "method": "ping"}, {"jsonrpc": "2.0", "id": 2, "method": "prompts/list"}])).send().await.unwrap();
    assert_eq!(batch.json::<Value>().await.unwrap().as_array().unwrap().len(), 2);
    assert_eq!(s.client.get(&url).send().await.unwrap().status(), 405);
    assert_eq!(s.client.post(&url).body("not json").send().await.unwrap().status(), 400);

    let page = s.client.post(&url).header("origin", "https://evil.example").json(&json!({"jsonrpc": "2.0", "id": 1, "method": "ping"})).send().await.unwrap();
    assert_eq!(page.status(), 403);
    let local = s.client.post(&url).header("origin", "http://localhost:3000").json(&json!({"jsonrpc": "2.0", "id": 1, "method": "ping"})).send().await.unwrap();
    assert_eq!(local.status(), 200);
    let rebound = s.client.post(&url).header("host", "rebind.attacker.example:8190").json(&json!({"jsonrpc": "2.0", "id": 1, "method": "ping"})).send().await.unwrap();
    assert_eq!(rebound.status(), 403);
}

/// A value for a required argument of the type the schema names.
fn placeholder(schema: &Value) -> Value {
    match schema["type"].as_str() {
        Some("integer" | "number") => json!(1),
        Some("boolean") => json!(true),
        Some("array") => json!([]),
        Some("object") => json!({}),
        _ => json!("x"),
    }
}

#[tokio::test]
async fn every_tool_reaches_a_route_that_exists() {
    let s = start().await;
    let list = s.rpc("tools/list", json!({})).await;
    let tools = list["result"]["tools"].as_array().unwrap();
    assert!(tools.len() > 100, "{} tools", tools.len());
    let mut names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    names.sort_unstable();
    let count = names.len();
    names.dedup();
    assert_eq!(names.len(), count, "tool names are unique");
    for name in ["project_create", "job_wait", "llm_active", "story_create", "api_request", "api_get_elevenlabs", "api_post_scenes_dialogue_craft", "api_get_instructions"] {
        assert!(names.contains(&name), "{name} is offered");
    }

    for tool in tools {
        let name = tool["name"].as_str().unwrap();
        assert_eq!(tool["inputSchema"]["type"], "object", "{name}");
        assert!(!tool["description"].as_str().unwrap().is_empty(), "{name}");
        // The one tool that asks an LLM something would talk to whatever listens on the LLM address.
        if name == "api_post_llm_test" || name == "api_request" {
            continue;
        }
        let properties = tool["inputSchema"]["properties"].as_object().cloned().unwrap_or_default();
        let required: Vec<String> = tool["inputSchema"]["required"].as_array().map(|r| r.iter().filter_map(|n| n.as_str().map(str::to_string)).collect()).unwrap_or_default();
        let args: serde_json::Map<String, Value> = required.iter().map(|n| (n.clone(), placeholder(&properties[n]))).collect();
        let (_, text, _) = s.tool(name, Value::Object(args)).await;
        assert!(!text.contains("HTTP_404") && !text.contains("HTTP_405") && !text.contains("TOOL_NOT_FOUND"), "{name} does not reach a route: {text}");
    }

    // The tools with several recipes, each recipe once.
    let cases = [
        ("post_apply", json!({"project_id": "x", "op": "apply_all"})),
        ("post_apply", json!({"project_id": "x", "op": "lut", "scene_id": "y"})),
        ("scene_split_merge_move_resize", json!({"project_id": "x", "scene_id": "y", "op": "split", "at_time": 1})),
        ("scene_split_merge_move_resize", json!({"project_id": "x", "scene_id": "y", "op": "merge"})),
        ("scene_split_merge_move_resize", json!({"project_id": "x", "scene_id": "y", "op": "move", "start_time": 1})),
        ("scene_split_merge_move_resize", json!({"project_id": "x", "scene_id": "y", "op": "resize", "duration": 2})),
        ("story_generate", json!({"project_id": "x", "what": "arc"})),
        ("story_generate", json!({"project_id": "x", "what": "motion_notes"})),
        ("story_generate", json!({"project_id": "x", "what": "concepts"})),
        ("prompts_generate", json!({"project_id": "x", "kind": "image", "scope": "all"})),
        ("prompts_generate", json!({"project_id": "x", "kind": "enhance", "scope": "one", "scene_id": "y"})),
        ("prompts_generate", json!({"project_id": "x", "kind": "edit", "scope": "one", "scene_id": "y"})),
        ("instructions_get_set", json!({"key": "zimage_t2i", "action": "save", "text": "t", "project_id": "x"})),
        ("instructions_get_set", json!({"key": "zimage_t2i", "action": "reset", "project_id": "x"})),
        ("image_approve_revert", json!({"project_id": "x", "scene_id": "y", "action": "revert"})),
        ("image_approve_revert", json!({"project_id": "x", "scene_id": "y", "action": "delete"})),
        ("image_generate", json!({"project_id": "x", "scene_id": "y"})),
        ("video_render", json!({"project_id": "x", "scene_id": "y"})),
        ("latents_status_rebuild", json!({"project_id": "x", "action": "dirty"})),
        ("latents_status_rebuild", json!({"project_id": "x", "action": "rebuild"})),
        ("latents_status_rebuild", json!({"project_id": "x", "action": "delete", "scene_id": "y"})),
        ("upload_file", json!({"filename": "a.cube", "content": "x"})),
        ("upload_file", json!({"filename": "a.wav", "kind": "audio", "project_id": "x", "content": "x"})),
        ("upload_file", json!({"filename": "a.png", "kind": "image", "project_id": "x", "scene_id": "y", "content": "x"})),
        ("reference_upsert", json!({"project_id": "x", "kind": "locations", "reference_id": "r", "payload": {}})),
        ("timeline_note_delete", json!({"project_id": "x", "note_id": "n"})),
    ];
    for (name, args) in cases {
        let (_, text, _) = s.tool(name, args.clone()).await;
        assert!(!text.contains("HTTP_404") && !text.contains("HTTP_405") && !text.contains("TOOL_NOT_FOUND"), "{name} {args} does not reach a route: {text}");
    }
}

#[tokio::test]
async fn a_project_is_built_with_the_tools_and_errors_tell_the_agent_what_to_do() {
    let s = start().await;
    let created = s.ok("project_create", json!({"project_name": "Demo Song"})).await;
    assert_eq!(created["project_id"], "Demo Song");
    assert_eq!(s.ok("project_list", json!({})).await[0]["id"], "Demo Song");
    s.ok("timeline_build", json!({"project_id": "Demo Song", "text": "4 first line\n4 second line\n4"})).await;
    let scenes = s.ok("scene_list", json!({"project_id": "Demo Song"})).await;
    assert_eq!(scenes.as_array().unwrap().len(), 3);
    let scene = scenes[0]["id"].as_str().unwrap().to_string();
    s.ok("scene_update", json!({"project_id": "Demo Song", "scene_id": scene, "patch": {"label": "Opening"}})).await;
    assert_eq!(s.ok("scene_get", json!({"project_id": "Demo Song", "scene_id": scene})).await["label"], "Opening");
    s.ok("lyrics_set", json!({"project_id": "Demo Song", "lyrics": ["one", "two"]})).await;
    assert_eq!(s.ok("project_update_settings", json!({"project_id": "Demo Song", "settings": {"project": {"video_type": "story"}}})).await["settings"]["project"]["video_type"], "story");
    assert_eq!(s.ok("project_get_settings", json!({"project_id": "Demo Song"})).await["project"]["video_type"], "story");
    assert!(s.ok("api_get_assets", json!({"project_id": "Demo Song"})).await["count"].is_number());
    assert_eq!(s.ok("api_request", json!({"method": "GET", "path": "/projects/Demo Song/scenes"})).await.as_array().unwrap().len(), 3);

    // An error says its code and what to do.
    let (failed, text, _) = s.tool("scene_get", json!({"project_id": "Demo Song", "scene_id": "nope"})).await;
    assert!(failed && text.contains("SCENE_NOT_FOUND") && text.contains("Next steps") && text.contains("scene_list"), "{text}");
    let (failed, text, _) = s.tool("project_get", json!({})).await;
    assert!(failed && text.contains("project_id is required"), "{text}");
    let (failed, text, _) = s.tool("project_delete", json!({"project_id": "Demo Song"})).await;
    assert!(failed && text.contains("confirm"), "{text}");
    // The generic call only reaches the agent API.
    for path in ["/setup/config", "/projects/../x", "http://elsewhere/projects"] {
        let (failed, text, _) = s.tool("api_request", json!({"method": "GET", "path": path})).await;
        assert!(failed, "{path}: {text}");
    }
    let (failed, text, _) = s.tool("nothing_here", json!({})).await;
    assert!(failed && text.contains("TOOL_NOT_FOUND"), "{text}");

    // A job is waited for.
    let started = s.ok("minimax_prompts", json!({"project_id": "Demo Song"})).await;
    let job = s.ok("job_wait", json!({"job_id": started["job_id"], "timeout_seconds": 30})).await;
    assert_eq!(job["status"], "failed", "{job}");

    // A picture is shown as an image.
    let folder = s.dir.path().join("projects").join("Demo Song").join("images");
    std::fs::create_dir_all(&folder).unwrap();
    let png = base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/q842iQAAAABJRU5ErkJggg==").unwrap();
    let file = folder.join("still.png");
    std::fs::write(&file, png).unwrap();
    let (failed, _, result) = s.tool("asset_view", json!({"path": file.display().to_string()})).await;
    assert!(!failed && result["content"][0]["type"] == "image" && result["content"][0]["mimeType"] == "image/png", "{result}");
    let (failed, text, _) = s.tool("asset_view", json!({"path": "C:/Windows/win.ini"})).await;
    assert!(failed, "{text}");
}

#[tokio::test]
async fn resources_prompts_and_the_rest_endpoints_describe_the_server() {
    let s = start().await;
    s.ok("project_create", json!({"project_name": "Demo Song"})).await;
    s.ok("lyrics_set", json!({"project_id": "Demo Song", "lyrics": "one\ntwo"})).await;

    let listed = s.rpc("resources/list", json!({})).await["result"]["resources"].clone();
    assert!(listed.as_array().unwrap().iter().any(|r| r["uri"] == "vrgdg://docs/music-video-playbook"));
    assert_eq!(s.rpc("resources/templates/list", json!({})).await["result"]["resourceTemplates"].as_array().unwrap().len(), 4);
    let playbook = s.rpc("resources/read", json!({"uri": "vrgdg://docs/music-video-playbook"})).await;
    let text = playbook["result"]["contents"][0]["text"].as_str().unwrap();
    assert!(text.contains("Part 3") && text.contains("H3 Studio MCP server") && text.contains("Speaking mode"), "the playbook is written for this app");
    let endpoints = s.rpc("resources/read", json!({"uri": "vrgdg://docs/endpoints"})).await;
    assert!(endpoints["result"]["contents"][0]["text"].as_str().unwrap().contains("/projects/{pid}/elevenlabs"));
    let spec = s.rpc("resources/read", json!({"uri": "vrgdg://docs/openapi"})).await;
    assert!(serde_json::from_str::<Value>(spec["result"]["contents"][0]["text"].as_str().unwrap()).unwrap()["paths"].is_object());
    let project = s.rpc("resources/read", json!({"uri": "vrgdg://project/Demo%20Song"})).await;
    assert!(project["result"]["contents"][0]["text"].as_str().unwrap().contains("Demo Song"));
    let lyrics = s.rpc("resources/read", json!({"uri": "vrgdg://project/Demo%20Song/lyrics"})).await;
    assert_eq!(lyrics["result"]["contents"][0]["text"], "one\ntwo");
    assert_eq!(s.rpc("resources/read", json!({"uri": "vrgdg://nothing"})).await["error"]["code"], -32002);

    assert_eq!(s.rpc("prompts/list", json!({})).await["result"]["prompts"].as_array().unwrap().len(), 5);
    let chat = s.rpc("prompts/get", json!({"name": "chat_make_music_video"})).await;
    assert!(chat["result"]["messages"][0]["content"]["text"].as_str().unwrap().contains("H3 Studio MCP tools"));
    let make = s.rpc("prompts/get", json!({"name": "make_music_video", "arguments": {"project_name": "Night Drive", "audio_file": "C:/song.mp3"}})).await;
    assert!(make["result"]["messages"][0]["content"]["text"].as_str().unwrap().contains("project_name: Night Drive"));
    assert_eq!(s.rpc("prompts/get", json!({"name": "nope"})).await["error"]["code"], -32602);

    // The setup screen and REST-only agents read these.
    let info: Value = s.client.get(format!("{}/api/v1/setup/mcp", s.root)).send().await.unwrap().json().await.unwrap();
    assert!(info["data"]["mcp_url"].as_str().unwrap().ends_with("/mcp") && info["data"]["tools"].as_u64().unwrap() > 100, "{info}");
    assert_eq!(info["data"]["stdio"]["args"], json!(["--mcp-stdio"]));
    let spec: Value = s.client.get(format!("{}/api/v1/openapi.json", s.root)).send().await.unwrap().json().await.unwrap();
    assert!(spec["paths"]["/projects/{project_id}/scenes"].is_object() || spec["paths"]["/projects/{pid}/scenes"].is_object());
    let markdown = s.client.get(format!("{}/api/v1/endpoints.md", s.root)).send().await.unwrap().text().await.unwrap();
    assert!(markdown.starts_with("# H3 Studio API endpoints"));
}
