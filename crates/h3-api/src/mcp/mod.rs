//! The MCP server built into the app, so an agent can connect to H3 Studio itself with nothing else installed.
//!
//! It speaks MCP over HTTP (`POST /mcp`, JSON-RPC 2.0, protocol versions 2024-11-05 to 2025-06-18) and exposes the same tools, resources and prompts
//! as the pack's own MCP server, calling this app's API inside the process. The app's own `--mcp-stdio` command bridges the same server for agents that
//! can only start a program. Only pages on this computer may call it: a request that carries another site's `Origin`, or a `Host` that is a name other
//! than `localhost`, is refused (the guard against a web page driving a local server).

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
}

/// The `/mcp` route, calling `api` (the `/api/v1` router with its state) for every tool.
pub fn router(api: Router, app: &Arc<AppState>) -> Router {
    let port = app.cfg.api_bind.rsplit(':').next().unwrap_or("8190");
    let state = McpState { api: Api::new(api, format!("http://127.0.0.1:{port}/api/v1")) };
    Router::new().route("/mcp", post(receive).get(refuse).delete(refuse)).with_state(state)
}

// -- the transport -----------------------------------------------------------------------------------------------------------

fn host_name(authority: &str) -> &str {
    match authority.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or(""),
        None => authority.rsplit_once(':').map_or(authority, |(name, _)| name),
    }
}

fn host_allowed(host: &str) -> bool {
    let name = host_name(host);
    name == "localhost" || name.ends_with(".localhost") || name.parse::<std::net::IpAddr>().is_ok()
}

fn origin_allowed(origin: &str) -> bool {
    let authority = origin.split("://").nth(1).unwrap_or("").split('/').next().unwrap_or("");
    let name = host_name(authority);
    name == "localhost" || name.ends_with(".localhost") || name.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

fn forbidden(why: &str) -> Response {
    (StatusCode::FORBIDDEN, [(header::CONTENT_TYPE, "application/json")], json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32000, "message": why}}).to_string()).into_response()
}

fn reply(status: StatusCode, value: &Value) -> Response {
    (status, [(header::CONTENT_TYPE, "application/json")], value.to_string()).into_response()
}

/// A stream for server messages is not offered, which the protocol allows: it answers 405.
async fn refuse() -> Response {
    let mut response = StatusCode::METHOD_NOT_ALLOWED.into_response();
    response.headers_mut().insert(header::ALLOW, HeaderValue::from_static("POST"));
    response
}

async fn receive(State(state): State<McpState>, headers: HeaderMap, body: Bytes) -> Response {
    if headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()).is_some_and(|origin| !origin_allowed(origin)) {
        return forbidden("This server only answers requests from this computer.");
    }
    if headers.get(header::HOST).and_then(|v| v.to_str().ok()).is_some_and(|host| !host_allowed(host)) {
        return forbidden("This server only answers requests addressed to localhost or an IP address.");
    }
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
        "initialize" => Ok(initialize(params)),
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

fn initialize(params: &Value) -> Value {
    let wanted = params.get("protocolVersion").and_then(Value::as_str);
    let version = wanted.filter(|w| VERSIONS.contains(w)).unwrap_or(VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": {"tools": {"listChanged": false}, "resources": {"listChanged": false, "subscribe": false}, "prompts": {"listChanged": false}},
        "serverInfo": {"name": SERVER_NAME, "title": "VRGDG H3 Studio", "version": env!("CARGO_PKG_VERSION")},
        "instructions": docs::INSTRUCTIONS,
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

async fn call_tool(state: &McpState, params: &Value) -> Value {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args: Map<String, Value> = params.get("arguments").and_then(Value::as_object).cloned().unwrap_or_default();
    match exec::run(&state.api, name, &args).await {
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
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok()).filter(|h| host_allowed(h)).map_or_else(|| format!("127.0.0.1:{}", app.cfg.api_bind.rsplit(':').next().unwrap_or("8190")), str::to_string);
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
    fn only_this_computer_may_call() {
        assert!(host_allowed("127.0.0.1:8190") && host_allowed("localhost:8190") && host_allowed("[::1]:8190") && host_allowed("192.168.1.20:8190"));
        assert!(!host_allowed("evil.example:8190") && !host_allowed("rebind.attacker.com"));
        assert!(origin_allowed("http://127.0.0.1:8190") && origin_allowed("http://localhost:3000") && origin_allowed("http://tauri.localhost") && origin_allowed("http://[::1]:8190"));
        assert!(!origin_allowed("https://evil.example") && !origin_allowed("null") && !origin_allowed("http://192.168.1.20:8190"));
    }

    #[test]
    fn escapes_in_a_resource_uri_are_decoded() {
        assert_eq!(decode("Demo%20Song"), "Demo Song");
        assert_eq!(decode("100%"), "100%");
        assert_eq!(decode("a%2Fb"), "a/b");
    }
}
