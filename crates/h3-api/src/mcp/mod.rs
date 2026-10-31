//! The MCP server built into the app, so an agent can connect to H3 Studio itself with nothing else installed.
//!
//! It speaks MCP over HTTP (`POST /mcp`, JSON-RPC 2.0, protocol versions 2024-11-05 to 2025-06-18) and exposes the same tools, resources and prompts
//! as the pack's own MCP server, calling this app's API inside the process. The app's own `--mcp-stdio` command bridges the same server for agents that
//! can only start a program. Who may call it is decided for the whole server by `guard.rs`.

pub mod bridge;
mod catalog;
mod docs;
mod exec;

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use serde_json::{json, Map, Value};

use crate::app::{AppState, Shared};
use crate::error::{success, ApiResult};
use catalog::catalog;
use exec::{Api, Output};

const SERVER_NAME: &str = "vrgdg-h3-studio";
/// The protocol versions this server speaks, newest first.
const VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// Tools that only read. Clients can run them without asking.
const READ_ONLY: [&str; 20] = [
    "system_health", "list_modes", "list_models", "project_list", "project_get", "project_summary", "project_get_settings", "minimax_settings_schema", "project_validate", "audio_analyze",
    "scene_list", "scene_get", "references_get", "timeline_notes_list", "llm_active", "job_get", "job_wait", "asset_view", "asset_download_url", "pipeline_plan",
];
const DESTRUCTIVE: [&str; 3] = ["project_delete", "scene_delete", "api_request"];

#[derive(Clone)]
struct McpState {
    api: Api,
    app: Arc<AppState>,
}

/// The `/mcp` route, calling `api` (the `/api/v1` router with its state) for every tool.
pub fn router(api: Router, app: &Arc<AppState>) -> Router {
    let port = app.cfg.api_bind.rsplit(':').next().unwrap_or("8190");
    let state = McpState { api: Api::new(api, format!("http://127.0.0.1:{port}/api/v1")), app: Arc::clone(app) };
    Router::new().route("/mcp", post(receive).get(refuse).delete(refuse)).with_state(state)
}

// -- the transport -----------------------------------------------------------------------------------------------------------

fn reply(status: StatusCode, value: &Value) -> Response {
    (status, [(header::CONTENT_TYPE, "application/json")], value.to_string()).into_response()
}

/// A stream for server messages is not offered, which the protocol allows: it answers 405.
async fn refuse() -> Response {
    let mut response = StatusCode::METHOD_NOT_ALLOWED.into_response();
    response.headers_mut().insert(header::ALLOW, HeaderValue::from_static("POST"));
    response
}

async fn receive(State(state): State<McpState>, body: Bytes) -> Response {
    let Ok(message) = serde_json::from_slice::<Value>(&body) else {
        return reply(StatusCode::BAD_REQUEST, &json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "Parse error"}}));
    };
    match message {
        Value::Array(batch) => {
            let mut answers = Vec::new();
            for item in batch {
                answers.extend(dispatch(&state, item).await);
            }
            if answers.is_empty() { StatusCode::ACCEPTED.into_response() } else { reply(StatusCode::OK, &Value::Array(answers)) }
        }
        single => match dispatch(&state, single).await {
            Some(answer) => reply(StatusCode::OK, &answer),
            None => StatusCode::ACCEPTED.into_response(),
        },
    }
}

/// One JSON-RPC message. A notification or an answer from the client gets no reply.
async fn dispatch(state: &McpState, message: Value) -> Option<Value> {
    let method = message.get("method").and_then(Value::as_str)?.to_string();
    let id = message.get("id").filter(|id| !id.is_null())?.clone();
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    Some(match handle(state, &method, &params).await {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err((code, text)) => json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": text}}),
    })
}

async fn handle(state: &McpState, method: &str, params: &Value) -> Result<Value, (i64, String)> {
    match method {
        "initialize" => {
            // The open Builder shows who connected.
            state.app.agent_connected(params);
            Ok(initialize(params, &state.app.open_project_note()))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tool_list()})),
        "tools/call" => Ok(call_tool(state, params).await),
        "resources/list" => Ok(json!({"resources": docs::resources()})),
        "resources/templates/list" => Ok(json!({"resourceTemplates": docs::templates()})),
        "resources/read" => read_resource(state, params).await,
        "prompts/list" => Ok(json!({"prompts": docs::prompts()})),
        "prompts/get" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("arguments").and_then(Value::as_object).cloned().unwrap_or_default();
            docs::prompt(name, &args).ok_or((-32602, format!("Prompt not found: '{name}'")))
        }
        other => Err((-32601, format!("Method not found: '{other}'"))),
    }
}

/// `open_project` says which project the app has open now, as the last line of the instructions.
fn initialize(params: &Value, open_project: &str) -> Value {
    let wanted = params.get("protocolVersion").and_then(Value::as_str);
    let version = wanted.filter(|w| VERSIONS.contains(w)).unwrap_or(VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": {"tools": {"listChanged": false}, "resources": {"listChanged": false, "subscribe": false}, "prompts": {"listChanged": false}},
        "serverInfo": {"name": SERVER_NAME, "title": "VRGDG H3 Studio", "version": env!("CARGO_PKG_VERSION")},
        "instructions": format!("{}\n{open_project}", docs::INSTRUCTIONS),
    })
}

// -- tools -------------------------------------------------------------------------------------------------------------------

fn api_request_tool() -> Value {
    json!({
        "name": "api_request",
        "description": "Call any endpoint of the agent API by method and path. The list is in resource vrgdg://docs/endpoints. `path` is relative to the API base with ids filled in, for example /projects/MySong/scenes. Endpoints that run as jobs return a job id: call job_wait.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "method": {"type": "string", "enum": ["GET", "POST", "PUT", "PATCH", "DELETE"]},
                "path": {"type": "string", "description": "for example /projects/MySong/scenes/seg_1/video/trim"},
                "body": {"type": "object", "description": "JSON request body"},
                "query": {"type": "object", "description": "Query parameters"},
                "if_match_revision": {"type": "integer"},
            },
            "required": ["method", "path"],
        },
        "annotations": {"readOnlyHint": false, "destructiveHint": true, "openWorldHint": false},
    })
}

fn tool_list() -> Vec<Value> {
    let mut list: Vec<Value> = catalog()
        .tools
        .iter()
        .map(|tool| {
            let mut entry = json!({"name": tool.name, "description": tool.description, "inputSchema": tool.input_schema});
            let annotations = tool.annotations.clone().or_else(|| {
                if READ_ONLY.contains(&tool.name.as_str()) {
                    Some(json!({"readOnlyHint": true, "openWorldHint": false}))
                } else if DESTRUCTIVE.contains(&tool.name.as_str()) {
                    Some(json!({"readOnlyHint": false, "destructiveHint": true, "openWorldHint": false}))
                } else {
                    None
                }
            });
            if let Some(annotations) = annotations {
                entry["annotations"] = annotations;
            }
            entry
        })
        .collect();
    list.push(api_request_tool());
    list
}

fn pretty(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
    }
}

fn tool_result(text: String, is_error: bool) -> Value {
    json!({"content": [{"type": "text", "text": text}], "isError": is_error})
}

/// Calls an agent makes again and again to wait or to check, which would only fill the feed.
const POLLING: [&str; 3] = ["job_get", "job_wait", "system_health"];

/// How the Builder's activity feed lists this call: `write` for what changes something, `read` for a look at the project or a picture (shown dimmed), and
/// `None` for the calls that only wait or check.
fn feed_kind(name: &str, args: &Map<String, Value>) -> Option<&'static str> {
    if POLLING.contains(&name) {
        return None;
    }
    if name == "asset_view" || READ_ONLY.contains(&name) {
        return Some("read");
    }
    match catalog().tool(name).and_then(|tool| tool.route.as_ref()) {
        Some(route) => Some(if route.method == "GET" { "read" } else { "write" }),
        None if name == "api_request" => {
            let method = args.get("method").and_then(Value::as_str).unwrap_or("GET");
            let path = args.get("path").and_then(Value::as_str).unwrap_or("");
            if !method.eq_ignore_ascii_case("GET") {
                Some("write")
            } else if path.starts_with("/jobs") || path.starts_with("/health") {
                None
            } else {
                Some("read")
            }
        }
        None => Some("write"),
    }
}

async fn call_tool(state: &McpState, params: &Value) -> Value {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args: Map<String, Value> = params.get("arguments").and_then(Value::as_object).cloned().unwrap_or_default();
    // The open Builder shows the call while it runs and how it ended.
    let started = std::time::Instant::now();
    let entry = feed_kind(name, &args).map(|kind| {
        let (project, scene, detail) = crate::activity::summarize(&args);
        let fields = json!({
            "source": "mcp", "kind": kind, "tool": name, "title": crate::activity::humanize(name), "detail": detail, "project_id": project, "scene_id": scene, "status": "running",
        });
        // Only what changes a project can touch it: its saves are listed on the entry while it runs.
        if kind == "write" { state.app.begin_call(&project, fields) } else { state.app.activity(None, fields) }
    });
    // The calls the tool makes to the API are what the entry names as the API it ran.
    let api = state.api.recording();
    let outcome = exec::run(&api, name, &args).await;
    if let Some(id) = entry {
        let mut patch = json!({"duration_ms": started.elapsed().as_millis() as u64});
        let calls = api.calls();
        if !calls.is_empty() {
            patch["api"] = json!(calls.join(", "));
            // What the first of them does, in a sentence.
            if let Some(about) = calls[0].split_once(' ').and_then(|(method, path)| describe_endpoint(method, path)) {
                patch["about"] = json!(about);
            }
        }
        match &outcome {
            Ok(Output::Data(data)) => {
                patch["status"] = json!("done");
                for field in ["job_id", "project_id"] {
                    if let Some(value) = data.get(field).and_then(Value::as_str) {
                        patch[field] = json!(value);
                    }
                }
                let result = crate::activity::result_summary(data);
                if !result.is_empty() {
                    patch["result"] = json!(result);
                }
            }
            Ok(Output::Image { .. }) => patch["status"] = json!("done"),
            Err(error) => {
                patch["status"] = json!("failed");
                patch["message"] = json!(format!("{}: {}", error.code, error.message.lines().next().unwrap_or("")));
            }
        }
        state.app.end_call(id, patch);
    }
    match outcome {
        Ok(Output::Data(data)) => tool_result(pretty(&data), false),
        Ok(Output::Image { data, mime, info }) => json!({"content": [{"type": "image", "data": data, "mimeType": mime}, {"type": "text", "text": pretty(&info)}], "isError": false}),
        Err(error) => tool_result(error.text(), true),
    }
}

// -- resources ---------------------------------------------------------------------------------------------------------------

/// Text with its `%XX` escapes decoded.
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' && at + 3 <= bytes.len() {
            if let Some(value) = std::str::from_utf8(&bytes[at + 1..at + 3]).ok().and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                out.push(value);
                at += 3;
                continue;
            }
        }
        out.push(bytes[at]);
        at += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn contents(uri: &str, mime: &str, text: String) -> Value {
    json!({"contents": [{"uri": uri, "mimeType": mime, "text": text}]})
}

async fn read_resource(state: &McpState, params: &Value) -> Result<Value, (i64, String)> {
    let uri = params.get("uri").and_then(Value::as_str).unwrap_or("").trim().to_string();
    if let Some((mime, text)) = docs::document(&uri) {
        return Ok(contents(&uri, mime, text.to_string()));
    }
    let failed = |e: exec::ToolError| (-32603, format!("Could not read '{uri}': {}", e.message));
    let json_of = |data: Value| contents(&uri, "application/json", pretty(&data));
    let parts: Vec<String> = uri.strip_prefix("vrgdg://").unwrap_or("").split('/').map(decode).collect();
    let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
    match parts.as_slice() {
        ["projects"] => state.api.get("/projects", &[]).await.map(json_of).map_err(failed),
        ["modes"] => state.api.get("/modes", &[]).await.map(json_of).map_err(failed),
        ["project", id, "scene", scene] => state.api.get(&format!("/projects/{}/scenes/{}", exec::seg(id), exec::seg(scene)), &[]).await.map(json_of).map_err(failed),
        ["project", id, "lyrics"] => {
            let data = state.api.get(&format!("/projects/{}/lyrics", exec::seg(id)), &[]).await.map_err(failed)?;
            Ok(contents(&uri, "text/plain", data.get("lyrics_text").map_or_else(|| pretty(&data), pretty)))
        }
        ["project", id] => state.api.get(&format!("/projects/{}", exec::seg(id)), &[]).await.map(json_of).map_err(failed),
        ["jobs", id, "log"] => {
            let data = state.api.get(&format!("/jobs/{}", exec::seg(id)), &[]).await.map_err(failed)?;
            let job = data.get("job").unwrap_or(&data);
            let field = |key: &str| job.get(key).map_or_else(|| "None".to_string(), pretty);
            Ok(contents(&uri, "text/plain", format!("Job ID: {}\nType: {}\nStatus: {}\nProgress: {}%\nMessage: {}\nError: {}", field("id"), field("type"), field("status"), field("progress"), field("message"), field("error"))))
        }
        _ => Err((-32002, format!("Unknown resource: {uri}"))),
    }
}

// -- routes of the REST API ------------------------------------------------------------------------------------------------------

/// `GET /setup/mcp`: how to connect an agent to this app, for the MCP Setup screen.
pub async fn info(State(app): Shared, headers: HeaderMap) -> ApiResult {
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok()).filter(|h| crate::guard::host_allowed(h)).map_or_else(|| format!("127.0.0.1:{}", app.cfg.api_bind.rsplit(':').next().unwrap_or("8190")), str::to_string);
    let program = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
    Ok(success(
        json!({
            "server_name": SERVER_NAME,
            "version": env!("CARGO_PKG_VERSION"),
            "protocol_versions": VERSIONS,
            "mcp_url": format!("http://{host}/mcp"),
            "api_url": format!("http://{host}/api/v1"),
            "openapi_url": format!("http://{host}/api/v1/openapi.json"),
            "stdio": {"command": program, "args": ["--mcp-stdio"]},
            "tools": tool_list().len(),
            "resources": docs::resources().len(),
            "prompts": docs::prompts().len(),
        }),
        None,
    ))
}

/// What an endpoint of the agent API does, in a sentence, for the Builder's activity feed.
pub fn describe_endpoint(method: &str, path: &str) -> Option<String> {
    let found = catalog().find(method, path)?;
    let first = found.description.split(". ").next().unwrap_or(&found.description).trim_end_matches('.');
    Some(if first.chars().count() > 90 { first.chars().take(87).collect::<String>() + "..." } else { first.to_string() })
}

/// `GET /openapi.json`: the contract of the endpoints this app serves.
pub async fn openapi() -> Response {
    ([(header::CONTENT_TYPE, "application/json")], docs::OPENAPI).into_response()
}

/// `GET /endpoints.md`: the same endpoints as a short list.
pub async fn endpoints() -> Response {
    ([(header::CONTENT_TYPE, "text/markdown; charset=utf-8")], docs::ENDPOINTS).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_in_a_resource_uri_are_decoded() {
        assert_eq!(decode("Demo%20Song"), "Demo Song");
        assert_eq!(decode("100%"), "100%");
        assert_eq!(decode("a%2Fb"), "a/b");
    }
}
