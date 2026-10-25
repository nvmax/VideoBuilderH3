//! A scene is named by its number on the timeline as well as by its id: "Scene 11" is `scene_id` "11" in every tool that takes a scene, and the tools
//! and instructions an agent reads say so.

use std::time::Duration;

use h3_llm::LlmConfig;
use h3_project::AppConfig;
use reqwest::Client;
use serde_json::{json, Map, Value};

struct Server {
    root: String,
    client: Client,
    _dir: tempfile::TempDir,
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
    Server { root: format!("http://{addr}"), client: Client::builder().timeout(Duration::from_secs(120)).build().unwrap(), _dir: dir }
}

impl Server {
    async fn rpc(&self, method: &str, params: Value) -> Value {
        let response = self.client.post(format!("{}/mcp", self.root)).json(&json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})).send().await.unwrap();
        assert_eq!(response.status(), 200, "{method}");
        response.json().await.unwrap()
    }

    /// A tool call: whether it failed, and the text it answered with.
    async fn tool(&self, name: &str, args: Value) -> (bool, String) {
        let answer = self.rpc("tools/call", json!({"name": name, "arguments": args})).await;
        let result = &answer["result"];
        let text = result["content"].as_array().and_then(|c| c.iter().find(|i| i["type"] == "text")).and_then(|i| i["text"].as_str()).unwrap_or("").to_string();
        (result["isError"].as_bool().unwrap_or(true), text)
    }

    async fn ok(&self, name: &str, args: Value) -> Value {
        let (failed, text) = self.tool(name, args).await;
        assert!(!failed, "{name}: {text}");
        serde_json::from_str(&text).unwrap_or(Value::String(text))
    }

    /// What a tool came to: its answer, and for a job the end of the job as well.
    async fn outcome(&self, name: &str, args: Value) -> String {
        let (_, text) = self.tool(name, args).await;
        let Some(job) = serde_json::from_str::<Value>(&text).ok().and_then(|v| v["job_id"].as_str().map(str::to_string)) else {
            return text;
        };
        let (_, ended) = self.tool("job_wait", json!({"job_id": job, "timeout_seconds": 30})).await;
        format!("{text}\n{ended}")
    }
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

/// Arguments that get a tool past its own checks to the scene it is given.
fn extras(tool: &str) -> Value {
    match tool {
        "post_apply" => json!({"op": "lut"}),
        "image_approve_revert" => json!({"action": "revert"}),
        "latents_status_rebuild" => json!({"action": "delete"}),
        "prompts_generate" => json!({"kind": "enhance", "scope": "one"}),
        "scene_update" => json!({"patch": {"label": "Renamed"}}),
        "scene_split_merge_move_resize" => json!({"op": "resize", "duration": 4}),
        _ => json!({}),
    }
}

#[tokio::test]
async fn a_scene_is_found_by_its_number_in_every_tool() {
    let s = start().await;
    s.ok("project_create", json!({"project_name": "Numbers"})).await;
    s.ok("timeline_build", json!({"project_id": "Numbers", "text": "4 a\n4 b\n4 c"})).await;
    let scenes = s.ok("scene_list", json!({"project_id": "Numbers"})).await;
    assert_eq!(scenes[1]["number"], 2);
    let second = scenes[1]["id"].as_str().unwrap().to_string();

    let by_id = s.ok("scene_get", json!({"project_id": "Numbers", "scene_id": second})).await;
    assert_eq!(s.ok("scene_get", json!({"project_id": "Numbers", "scene_id": "2"})).await, by_id);
    assert_eq!(s.ok("scene_get", json!({"project_id": "Numbers", "scene_id": 2})).await, by_id, "a number sent as a number");

    let list = s.rpc("tools/list", json!({})).await;
    let mut tried = Vec::new();
    let mut refused = Vec::new();
    for tool in list["result"]["tools"].as_array().unwrap() {
        let name = tool["name"].as_str().unwrap();
        let properties = tool["inputSchema"]["properties"].as_object().cloned().unwrap_or_default();
        // Deleting a scene would move the numbers of the others.
        if !properties.contains_key("scene_id") || name == "scene_delete" {
            continue;
        }
        let required: Vec<String> = tool["inputSchema"]["required"].as_array().map(|r| r.iter().filter_map(|n| n.as_str().map(str::to_string)).collect()).unwrap_or_default();
        let mut args: Map<String, Value> = required.iter().map(|n| (n.clone(), placeholder(&properties[n]))).collect();
        args.extend(extras(name).as_object().unwrap().clone());
        args.insert("project_id".into(), json!("Numbers"));

        args.insert("scene_id".into(), json!(second));
        let with_id = s.outcome(name, Value::Object(args.clone())).await;
        args.insert("scene_id".into(), json!("2"));
        let with_number = s.outcome(name, Value::Object(args)).await;

        assert!(!with_id.contains("SCENE_NOT_FOUND"), "{name} does not find the scene by its id: {with_id}");
        if ["SCENE_NOT_FOUND", "Scene '2' was not found", "Scene 2 not found"].iter().any(|missing| with_number.contains(missing)) {
            refused.push(format!("{name}: {}", with_number.lines().next().unwrap_or_default()));
        }
        tried.push(name.to_string());
    }
    assert!(tried.len() > 30, "{} tools took a scene: {tried:?}", tried.len());
    assert!(refused.is_empty(), "these tools do not find a scene by its number:\n{}", refused.join("\n\n"));
}

#[tokio::test]
async fn the_tools_and_the_instructions_say_a_scene_has_a_number() {
    let s = start().await;
    let init = s.rpc("initialize", json!({"protocolVersion": "2025-06-18"})).await;
    let instructions = init["result"]["instructions"].as_str().unwrap();
    assert!(instructions.contains("Scene 11") && instructions.contains("scene_id"), "{instructions}");

    let list = s.rpc("tools/list", json!({})).await;
    let tools = list["result"]["tools"].as_array().unwrap();
    let described = |name: &str| tools.iter().find(|t| t["name"] == name).unwrap_or_else(|| panic!("{name} is offered"))["description"].as_str().unwrap().to_string();
    assert!(described("scene_list").contains("number"), "{}", described("scene_list"));
    assert!(described("scene_get").contains("number"), "{}", described("scene_get"));
    let mut taking = 0;
    for tool in tools {
        let properties = tool["inputSchema"]["properties"].as_object().cloned().unwrap_or_default();
        for key in ["scene_id", "scene_ids"] {
            if let Some(property) = properties.get(key).filter(|p| p["type"] != "object" && !p["description"].as_str().is_some_and(|d| d.contains("number"))) {
                panic!("{} says nothing of the scene number in {key}: {property}", tool["name"]);
            }
            taking += usize::from(properties.contains_key(key));
        }
    }
    assert!(taking > 30, "{taking} arguments name a scene");

    let (failed, text) = s.tool("scene_get", json!({"project_id": "Nothing", "scene_id": "11"})).await;
    assert!(failed, "{text}");
    s.ok("project_create", json!({"project_name": "Short"})).await;
    s.ok("timeline_build", json!({"project_id": "Short", "text": "4 a\n4 b"})).await;
    let (failed, text) = s.tool("scene_get", json!({"project_id": "Short", "scene_id": "11"})).await;
    assert!(failed && text.contains("SCENE_NOT_FOUND") && text.contains("number") && text.contains("2 scenes"), "{text}");
}
