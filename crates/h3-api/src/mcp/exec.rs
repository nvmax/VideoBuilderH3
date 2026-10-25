//! Running a tool: one call (or a few) to this app's own API, made inside the process, and the answer as an MCP client reads it. The recipes of the
//! named tools follow the pack's `mcp_server/tools.py`, so an agent gets the same behavior from either.

#![allow(clippy::result_large_err)]

use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{header, HeaderMap, Method, Request};
use axum::Router;
use base64::Engine;
use serde_json::{json, Map, Value};
use tower::ServiceExt;

use super::catalog::{catalog, Tool};

pub type Args = Map<String, Value>;

/// A tool that did not work, worded for the agent: what failed and what to try.
pub struct ToolError {
    pub code: String,
    pub message: String,
    pub details: Value,
    pub next_steps: Vec<String>,
}

impl ToolError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self { code: code.to_string(), message: message.into(), details: Value::Null, next_steps: next_steps(code) }
    }

    pub fn validation(message: impl Into<String>) -> Self {
        Self::new("VALIDATION_ERROR", message)
    }

    pub fn text(&self) -> String {
        let mut out = format!("Error [{}]: {}", self.code, self.message);
        if self.details.as_object().is_some_and(|d| !d.is_empty()) {
            out.push_str(&format!("\n\nDetails: {}", self.details));
        }
        if !self.next_steps.is_empty() {
            out.push_str("\n\nNext steps:\n");
            out.push_str(&self.next_steps.iter().map(|s| format!("- {s}")).collect::<Vec<_>>().join("\n"));
        }
        out
    }
}

/// What to do about an error code, for the codes an agent can act on.
fn next_steps(code: &str) -> Vec<String> {
    let step = match code {
        "PREDECESSOR_MISSING" => "Render the predecessor scene first, or turn latent continuation off.",
        "LATENT_STALE" => "Call latents_status_rebuild with action 'rebuild' to rebuild the dirty latent chain.",
        "REVISION_CONFLICT" => "Read the project again with project_get and retry with the new revision.",
        "SCENE_NOT_FOUND" => "Call scene_list to see the scenes of this project. A scene is named by its id or by its number (1 is the first scene, as in Scene 11).",
        "PROJECT_NOT_FOUND" => "Call project_list to see the projects.",
        "JOB_NOT_FOUND" => "Call api_get_jobs to see the jobs.",
        "VALIDATION_ERROR" => "Check the arguments against the tool's description, and call project_validate to look for missing settings.",
        "LLM_UNAVAILABLE" => "The LLM the app is set to use is not answering. Ask the user to start it (never load or switch a model yourself), then call llm_active and try again.",
        "COMFY_UNAVAILABLE" => "ComfyUI is not reachable. Ask the user to start it and check its address in App Settings, then call system_health.",
        _ => return Vec::new(),
    };
    vec![step.to_string()]
}

/// What a tool returns: data to show as text, or a picture with a note.
pub enum Output {
    Data(Value),
    Image { data: String, mime: &'static str, info: Value },
}

// -- calling the API ----------------------------------------------------------------------------------------------------------

/// The app's own API, called without a network round trip.
#[derive(Clone)]
pub struct Api {
    router: Router,
    base: String,
    /// The calls made through this copy, when it was made by [`Api::recording`].
    calls: Option<std::sync::Arc<std::sync::Mutex<Vec<String>>>>,
}

/// Percent-encode text for one path segment or a query value.
pub fn seg(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// A path the caller wrote: separators and escapes already in it stay, anything else is escaped.
fn encode_path(path: &str) -> String {
    let mut out = String::new();
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' | b'%' => out.push(byte as char),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn uri(path: &str, query: &[(String, String)]) -> String {
    let mut out = path.to_string();
    if !query.is_empty() {
        out.push('?');
        out.push_str(&query.iter().map(|(k, v)| format!("{}={}", seg(k), seg(v))).collect::<Vec<_>>().join("&"));
    }
    out
}

/// A JSON value as the text of a query parameter. A list is joined with commas.
fn query_text(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        Value::Array(items) => Some(items.iter().filter_map(query_text).collect::<Vec<_>>().join(",")),
        other => Some(other.to_string()),
    }
}

fn query_pairs(value: &Value) -> Vec<(String, String)> {
    value.as_object().map(|m| m.iter().filter_map(|(k, v)| query_text(v).map(|t| (k.clone(), t))).collect()).unwrap_or_default()
}

impl Api {
    pub fn new(router: Router, base: String) -> Self {
        Self { router, base, calls: None }
    }

    /// A copy that remembers the calls made through it, for the Builder's activity feed.
    pub fn recording(&self) -> Self {
        Self { calls: Some(std::sync::Arc::default()), ..self.clone() }
    }

    /// The calls a recording copy made, as `METHOD /path` in the order they were made, a call repeated in a row once.
    pub fn calls(&self) -> Vec<String> {
        let mut all: Vec<String> = self.calls.as_ref().map(|log| log.lock().unwrap_or_else(|e| e.into_inner()).clone()).unwrap_or_default();
        all.dedup();
        all
    }

    /// The address a client outside this process uses for the API.
    pub fn base(&self) -> &str {
        &self.base
    }

    async fn raw(&self, method: &str, path: &str, query: &[(String, String)], body: Option<&Value>, if_match: Option<i64>) -> Result<(u16, HeaderMap, Vec<u8>), ToolError> {
        if let Some(log) = &self.calls {
            let slash = if path.starts_with('/') { "" } else { "/" };
            log.lock().unwrap_or_else(|e| e.into_inner()).push(format!("{} {slash}{path}", method.to_uppercase()));
        }
        let method = Method::from_bytes(method.as_bytes()).map_err(|_| ToolError::validation(format!("'{method}' is not an HTTP method.")))?;
        let mut builder = Request::builder().method(method).uri(uri(path, query)).header(header::CONTENT_TYPE, "application/json");
        if let Some(revision) = if_match {
            builder = builder.header("if-match", revision.to_string());
        }
        let payload = body.map(|b| serde_json::to_vec(b).unwrap_or_default()).unwrap_or_default();
        let request = builder.body(Body::from(payload)).map_err(|e| ToolError::validation(format!("The request could not be made: {e}")))?;
        let response = match self.router.clone().oneshot(request).await {
            Ok(response) => response,
            Err(never) => match never {},
        };
        let (parts, body) = response.into_parts();
        let bytes = axum::body::to_bytes(body, 1 << 30).await.map_err(|e| ToolError::new("INTERNAL_ERROR", format!("The reply could not be read: {e}")))?;
        Ok((parts.status.as_u16(), parts.headers, bytes.to_vec()))
    }

    /// One call. The data of the answer on success, the API's error on failure.
    pub async fn call(&self, method: &str, path: &str, query: &[(String, String)], body: Option<&Value>, if_match: Option<i64>) -> Result<Value, ToolError> {
        let (status, _, bytes) = self.raw(method, path, query, body, if_match).await?;
        envelope(status, &bytes)
    }

    pub async fn get(&self, path: &str, query: &[(String, String)]) -> Result<Value, ToolError> {
        self.call("GET", path, query, None, None).await
    }

    pub async fn send(&self, method: &str, path: &str, body: &Value, if_match: Option<i64>) -> Result<Value, ToolError> {
        self.call(method, path, &[], Some(body), if_match).await
    }
}

/// `{"ok": true, "data": ...}` gives the data, anything else the error it carries.
fn envelope(status: u16, bytes: &[u8]) -> Result<Value, ToolError> {
    let body: Value = serde_json::from_slice(bytes).unwrap_or_else(|_| json!({"ok": status < 400, "data": String::from_utf8_lossy(bytes)}));
    if status >= 400 || body.get("ok") == Some(&json!(false)) {
        let error = body.get("error").cloned().unwrap_or(Value::Null);
        let code = error.get("code").and_then(Value::as_str).map_or(format!("HTTP_{status}"), str::to_string);
        let message = error.get("message").and_then(Value::as_str).map_or_else(|| format!("The app answered {status}."), str::to_string);
        let mut failure = ToolError::new(&code, message);
        failure.details = error.get("details").cloned().unwrap_or(Value::Null);
        return Err(failure);
    }
    Ok(body.get("data").cloned().unwrap_or(body))
}

// -- reading the arguments ---------------------------------------------------------------------------------------------------

fn text(args: &Args, key: &str) -> String {
    match args.get(key) {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

fn first_text(args: &Args, keys: &[&str]) -> String {
    keys.iter().map(|k| text(args, k)).find(|t| !t.is_empty()).unwrap_or_default()
}

fn need(args: &Args, key: &str) -> Result<String, ToolError> {
    let value = text(args, key);
    if value.is_empty() { Err(ToolError::validation(format!("{key} is required."))) } else { Ok(value) }
}

fn project(args: &Args) -> Result<String, ToolError> {
    Ok(format!("/projects/{}", seg(&need(args, "project_id")?)))
}

fn scene(args: &Args) -> Result<String, ToolError> {
    Ok(format!("{}/scenes/{}", project(args)?, seg(&need(args, "scene_id")?)))
}

/// The arguments as a request body, without the ones that went into the path.
fn rest(args: &Args, skip: &[&str]) -> Value {
    Value::Object(args.iter().filter(|(k, _)| !skip.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect())
}

fn object_arg(args: &Args, key: &str) -> Value {
    args.get(key).filter(|v| v.is_object()).cloned().unwrap_or_else(|| json!({}))
}

fn revision(args: &Args) -> Option<i64> {
    args.get("if_match_revision").and_then(Value::as_i64)
}

fn flag(args: &Args, key: &str) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn number(args: &Args, key: &str) -> Option<Value> {
    args.get(key).filter(|v| v.is_number()).cloned()
}

fn pick(args: &Args, keys: &[&str]) -> Value {
    Value::Object(keys.iter().filter_map(|k| args.get(*k).map(|v| ((*k).to_string(), v.clone()))).collect())
}

// -- the tools ---------------------------------------------------------------------------------------------------------------

/// A tool of the catalog by name, or `api_request`.
pub async fn run(api: &Api, name: &str, args: &Args) -> Result<Output, ToolError> {
    if name == "api_request" {
        return api_request(api, args).await.map(Output::Data);
    }
    let Some(tool) = catalog().tool(name) else {
        return Err(ToolError::new("TOOL_NOT_FOUND", format!("Tool not found: '{name}'.")));
    };
    if tool.kind == "endpoint" {
        return endpoint_tool(api, tool, args).await.map(Output::Data);
    }
    named_tool(api, name, args).await
}

/// A generated tool: the one call its route stands for.
async fn endpoint_tool(api: &Api, tool: &Tool, args: &Args) -> Result<Value, ToolError> {
    let route = tool.route.as_ref().ok_or_else(|| ToolError::new("INTERNAL_ERROR", "The tool has no route."))?;
    let mut missing: Vec<&str> = route.args.values().map(String::as_str).filter(|arg| text(args, arg).is_empty()).collect();
    missing.sort_unstable();
    missing.dedup();
    if !missing.is_empty() {
        return Err(ToolError::validation(format!("missing {}.", missing.join(", "))));
    }
    let mut path = String::new();
    for part in route.path.split('/').filter(|s| !s.is_empty()) {
        path.push('/');
        match part.strip_prefix('{').and_then(|p| p.strip_suffix('}')) {
            Some(param) => path.push_str(&seg(&text(args, route.args.get(param).map_or(param, String::as_str)))),
            None => path.push_str(part),
        }
    }
    let body = args.get("body").filter(|v| v.is_object()).cloned();
    let query = args.get("query").map(query_pairs).unwrap_or_default();
    let body = if route.method == "GET" { None } else { Some(body.unwrap_or_else(|| json!({}))) };
    api.call(&route.method, &path, &query, body.as_ref(), revision(args)).await
}

/// Any endpoint of the API by method and path. Paths outside the agent API (the app's own setup routes) are refused.
async fn api_request(api: &Api, args: &Args) -> Result<Value, ToolError> {
    let method = Some(text(args, "method")).filter(|m| !m.is_empty()).unwrap_or_else(|| "GET".into()).to_uppercase();
    let path = text(args, "path");
    if !matches!(method.as_str(), "GET" | "POST" | "PUT" | "PATCH" | "DELETE") {
        return Err(ToolError::validation("method must be GET, POST, PUT, PATCH or DELETE."));
    }
    if !path.starts_with('/') || path.contains("://") || path.contains("..") {
        return Err(ToolError::validation("path must start with / and be relative to the API base, for example /projects."));
    }
    if !catalog().serves(&method, &path) {
        return Err(ToolError::validation(format!("{method} {path} is not an endpoint of this app's agent API. Read resource vrgdg://docs/endpoints for the list.")));
    }
    let body = args.get("body").filter(|v| v.is_object()).cloned();
    let body = if method == "GET" { None } else { Some(body.unwrap_or_else(|| json!({}))) };
    let query = args.get("query").map(query_pairs).unwrap_or_default();
    api.call(&method, &encode_path(&path), &query, body.as_ref(), revision(args)).await
}

fn one(key: &str, value: String) -> Vec<(String, String)> {
    if value.is_empty() { Vec::new() } else { vec![(key.to_string(), value)] }
}

async fn named_tool(api: &Api, name: &str, args: &Args) -> Result<Output, ToolError> {
    let data = match name {
        "system_health" => api.get("/health", &[]).await?,
        "list_modes" => api.get("/modes", &[]).await?,
        "list_models" => api.get("/models", &[]).await?,
        "project_list" => api.get("/projects", &[]).await?,
        "project_create" => project_create(api, args).await?,
        "project_get" => api.get(&project(args)?, &one("include", text(args, "include"))).await?,
        "project_summary" => api.get(&format!("{}/summary", project(args)?), &[]).await?,
        "project_update_settings" => api.send("PATCH", &format!("{}/settings", project(args)?), &object_arg(args, "settings"), revision(args)).await?,
        "project_get_settings" => api.get(&format!("{}/settings", project(args)?), &[]).await?,
        "minimax_settings_schema" => api.get("/settings/minimax-h3/schema", &[]).await?,
        "project_duplicate" => {
            let mut body = Map::new();
            let new_name = first_text(args, &["new_name", "new_project_name"]);
            if !new_name.is_empty() {
                body.insert("new_name".into(), json!(new_name));
            }
            if args.get("options").is_some_and(Value::is_object) {
                body.insert("options".into(), args["options"].clone());
            }
            api.send("POST", &format!("{}/duplicate", project(args)?), &Value::Object(body), None).await?
        }
        "project_validate" => api.send("POST", &format!("{}/settings/preflight", project(args)?), &object_arg(args, "settings"), None).await?,
        "project_export" => {
            let kind = Some(text(args, "export_type")).filter(|t| !t.is_empty()).unwrap_or_else(|| "zip".into());
            api.send("POST", &format!("{}/export", project(args)?), &json!({"export_type": kind}), None).await?
        }
        "project_delete" => {
            let id = need(args, "project_id")?;
            if !flag(args, "confirm") {
                return Err(ToolError::validation("Deletion aborted: parameter 'confirm' must be explicitly true."));
            }
            api.call("DELETE", &project(args)?, &[("confirm".to_string(), id)], None, None).await?
        }
        "audio_attach" => api.send("PUT", &format!("{}/audio", project(args)?), &json!({"audio_path": need(args, "audio_file")?}), None).await?,
        "audio_analyze" => api.get(&format!("{}/audio/beats", project(args)?), &[]).await?,
        "lyrics_set" => {
            let lyrics = match args.get("lyrics") {
                Some(Value::Array(items)) => items.iter().map(|i| i.get("text").map_or_else(|| i.as_str().unwrap_or("").to_string(), |t| t.as_str().unwrap_or("").to_string())).collect::<Vec<_>>().join("\n"),
                Some(Value::String(s)) => s.clone(),
                _ => return Err(ToolError::validation("lyrics is required.")),
            };
            api.send("PUT", &format!("{}/lyrics", project(args)?), &json!({"lyrics_text": lyrics}), None).await?
        }
        "lyrics_transcribe" => {
            let language = Some(text(args, "language")).filter(|l| !l.is_empty()).unwrap_or_else(|| "english".into());
            let language = if language.eq_ignore_ascii_case("en") { "english".to_string() } else { language };
            api.send("POST", &format!("{}/lyrics/align", project(args)?), &json!({"language": language}), None).await?
        }
        "timeline_build" => {
            let body = json!({
                "text": text(args, "text"),
                "mode": Some(text(args, "mode")).filter(|m| !m.is_empty()).unwrap_or_else(|| "durations".into()),
                "action": Some(text(args, "action")).filter(|m| !m.is_empty()).unwrap_or_else(|| "replace".into()),
                "append_start": args.get("append_start").and_then(Value::as_f64).unwrap_or(0.0),
                "clear_media": flag(args, "clear_media"),
            });
            api.send("POST", &format!("{}/timeline/bulk", project(args)?), &body, None).await?
        }
        "scene_list" => api.get(&format!("{}/scenes", project(args)?), &[]).await?,
        "scene_get" => api.get(&scene(args)?, &[]).await?,
        "scene_update" => api.send("PATCH", &scene(args)?, &object_arg(args, "patch"), revision(args)).await?,
        "scene_insert" => api.send("POST", &format!("{}/scenes", project(args)?), &object_arg(args, "scene"), None).await?,
        "scene_delete" => api.call("DELETE", &scene(args)?, &[("ripple".to_string(), flag(args, "ripple").to_string())], None, None).await?,
        "scene_split_merge_move_resize" => scene_operation(api, args).await?,
        "scenes_bulk_edit" => api.send("POST", &format!("{}/scenes/bulk", project(args)?), &json!({"operations": args.get("operations").cloned().unwrap_or_else(|| json!([]))}), None).await?,
        "story_generate" => {
            let what = Some(text(args, "what")).filter(|w| !w.is_empty()).unwrap_or_else(|| "brief".into()).to_lowercase();
            let params = object_arg(args, "params");
            let base = project(args)?;
            let path = match what.as_str() {
                "arc" | "brief" | "beats" => format!("{base}/story/{what}"),
                "motion_notes" => format!("{base}/prompts/motion-notes"),
                _ => format!("{base}/prompts/concepts"),
            };
            api.send("POST", &path, &params, None).await?
        }
        "story_get_set" => {
            let path = format!("{}/story", project(args)?);
            match args.get("story") {
                Some(story) => api.send("PUT", &path, story, None).await?,
                None => api.get(&path, &[]).await?,
            }
        }
        "references_get" => api.get(&format!("{}/references", project(args)?), &[]).await?,
        "reference_upsert" => {
            let kind = if text(args, "kind").to_lowercase().starts_with("loc") { "locations" } else { "subjects" };
            api.send("PUT", &format!("{}/references/{kind}/{}", project(args)?, seg(&need(args, "reference_id")?)), &object_arg(args, "payload"), None).await?
        }
        "references_extract" => api.send("POST", &format!("{}/references/locations/extract", project(args)?), &object_arg(args, "params"), None).await?,
        "reference_scene_mapping_set" => api.send("PUT", &format!("{}/references/scene-mapping", project(args)?), &object_arg(args, "mapping"), None).await?,
        "prompts_generate" => prompts_generate(api, args).await?,
        "instructions_get_set" => instructions(api, args).await?,
        "image_generate" => {
            let body = object_arg(args, "params");
            match text(args, "scene_id").as_str() {
                "" => api.send("POST", &format!("{}/images/generate", project(args)?), &body, None).await?,
                _ => api.send("POST", &format!("{}/image/generate", scene(args)?), &body, None).await?,
            }
        }
        "image_set_from_upload" => api.send("PUT", &format!("{}/image", scene(args)?), &pick(args, &["image_data", "source_path"]), None).await?,
        "image_approve_revert" => image_action(api, args).await?,
        "video_render" => {
            let body = object_arg(args, "params");
            match text(args, "scene_id").as_str() {
                "" => api.send("POST", &format!("{}/video/render", project(args)?), &body, None).await?,
                _ => api.send("POST", &format!("{}/video/render", scene(args)?), &body, None).await?,
            }
        }
        "video_recover" => api.send("POST", &format!("{}/video/recover", scene(args)?), &object_arg(args, "params"), None).await?,
        "video_select_take" => api.send("POST", &format!("{}/video/select", scene(args)?), &json!({"source_path": need(args, "source_path")?}), None).await?,
        "latents_status_rebuild" => latents(api, args).await?,
        "post_apply" => post_apply(api, args).await?,
        "stitch_final" => api.send("POST", &format!("{}/stitch", project(args)?), &object_arg(args, "params"), None).await?,
        "pipeline_build_full_video" => {
            let pipeline = Some(text(args, "pipeline")).filter(|p| !p.is_empty()).unwrap_or_else(|| "full_video".into());
            if pipeline != "full_video" {
                return Err(ToolError::validation("Only the full_video pipeline is available. This app makes MiniMax H3 videos."));
            }
            api.send("POST", &format!("{}/pipelines/build-full-video", project(args)?), &rest(args, &["project_id", "pipeline"]), None).await?
        }
        "lyrics_align" => api.send("POST", &format!("{}/lyrics/align", project(args)?), &rest(args, &["project_id"]), None).await?,
        "timeline_from_lines" => api.send("POST", &format!("{}/timeline/from-lines", project(args)?), &rest(args, &["project_id"]), None).await?,
        "timeline_enforce_length" => api.send("POST", &format!("{}/timeline/enforce-length", project(args)?), &rest(args, &["project_id"]), None).await?,
        "timeline_notes_list" => api.get(&format!("{}/timeline/notes", project(args)?), &[]).await?,
        "timeline_note_create" => api.send("POST", &format!("{}/timeline/notes", project(args)?), &pick(args, &["id", "start", "end", "type", "label", "note"]), revision(args)).await?,
        "timeline_note_update" => {
            let path = format!("{}/timeline/notes/{}", project(args)?, seg(&need(args, "note_id")?));
            api.send("PATCH", &path, &pick(args, &["start", "end", "type", "label", "note"]), revision(args)).await?
        }
        "timeline_note_delete" => api.call("DELETE", &format!("{}/timeline/notes/{}", project(args)?, seg(&need(args, "note_id")?)), &[], None, revision(args)).await?,
        "reference_describe" => {
            let kind = Some(text(args, "kind")).filter(|k| !k.is_empty()).unwrap_or_else(|| "subjects".into());
            let path = format!("{}/references/{}/{}/describe", project(args)?, seg(&kind), seg(&need(args, "ref_id")?));
            api.send("POST", &path, &rest(args, &["project_id", "kind", "ref_id"]), None).await?
        }
        "reference_extract_locations" => api.send("POST", &format!("{}/references/locations/extract", project(args)?), &rest(args, &["project_id"]), None).await?,
        "reference_assign_scenes" => api.send("POST", &format!("{}/references/assign-scenes", project(args)?), &rest(args, &["project_id"]), None).await?,
        "llm_active" => api.get("/llm/active", &one("project_id", text(args, "project_id"))).await?,
        "story_settings" => api.send("PUT", &format!("{}/story/settings", project(args)?), &rest(args, &["project_id"]), None).await?,
        "story_create" => api.send("POST", &format!("{}/story/{}", project(args)?, seg(&need(args, "step")?)), &rest(args, &["project_id", "step"]), None).await?,
        "minimax_prompts" => api.send("POST", &format!("{}/minimax-prompts", project(args)?), &rest(args, &["project_id"]), None).await?,
        "pipeline_from_song" => api.send("POST", "/pipelines/from-song", &Value::Object(args.clone()), None).await?,
        "pipeline_plan" => api.get(&format!("{}/pipelines/plan", project(args)?), &query_pairs(&rest(args, &["project_id"]))).await?,
        "job_get" => api.get(&format!("/jobs/{}", seg(&need(args, "job_id")?)), &[]).await?,
        "job_wait" => job_wait(api, args).await?,
        "job_cancel" => api.send("POST", &format!("/jobs/{}/cancel", seg(&need(args, "job_id")?)), &json!({}), None).await?,
        "job_retry" => api.send("POST", &format!("/jobs/{}/retry", seg(&need(args, "job_id")?)), &json!({}), None).await?,
        "asset_view" => return asset_view(api, args).await,
        "asset_download_url" => asset_url(api, args).await?,
        "upload_file" => upload_file(api, args).await?,
        other => return Err(ToolError::new("TOOL_NOT_FOUND", format!("Tool not found: '{other}'."))),
    };
    Ok(Output::Data(data))
}

/// Create the project, then give it its song or a silent track when the call names one.
async fn project_create(api: &Api, args: &Args) -> Result<Value, ToolError> {
    let name = first_text(args, &["project_name", "name"]);
    let mut body = json!({"name": name});
    if !text(args, "template_from").is_empty() {
        body["template_from"] = json!(text(args, "template_from"));
    }
    let created = api.send("POST", "/projects", &body, None).await?;
    let id = first_text(created.as_object().unwrap_or(&Map::new()), &["project_id", "id"]);
    if !id.is_empty() {
        let path = format!("/projects/{}", seg(&id));
        if !text(args, "audio_file").is_empty() {
            api.send("PUT", &format!("{path}/audio"), &json!({"audio_path": text(args, "audio_file")}), None).await?;
        } else if let Some(duration) = args.get("duration").and_then(Value::as_f64).filter(|d| *d > 0.0) {
            api.send("POST", &format!("{path}/audio/silent"), &json!({"duration": duration}), None).await?;
        }
    }
    Ok(created)
}

async fn scene_operation(api: &Api, args: &Args) -> Result<Value, ToolError> {
    let base = scene(args)?;
    let op = text(args, "op").to_lowercase();
    let (action, body) = match op.as_str() {
        "split" => ("split", json!({"at_time": args.get("at_time").or_else(|| args.get("split_time")).cloned().unwrap_or(Value::Null)})),
        "merge" => ("merge", json!({"with_direction": Some(first_text(args, &["with_direction", "direction"])).filter(|d| !d.is_empty()).unwrap_or_else(|| "next".into())})),
        "move" => ("move", json!({"start_time": args.get("start_time").cloned().unwrap_or_else(|| json!(0.0)), "ripple": flag(args, "ripple")})),
        "resize" => {
            let mut body = json!({"ripple": flag(args, "ripple")});
            for key in ["duration", "end_time"] {
                if let Some(value) = number(args, key) {
                    body[key] = value;
                }
            }
            ("resize", body)
        }
        other => return Err(ToolError::validation(format!("Invalid op: '{other}'. Must be one of: split, merge, move, resize."))),
    };
    api.send("POST", &format!("{base}/{action}"), &body, None).await
}

async fn prompts_generate(api: &Api, args: &Args) -> Result<Value, ToolError> {
    let base = project(args)?;
    let kind = Some(text(args, "kind")).filter(|k| !k.is_empty()).unwrap_or_else(|| "image".into()).to_lowercase();
    let scope = Some(text(args, "scope")).filter(|k| !k.is_empty()).unwrap_or_else(|| "all".into()).to_lowercase();
    let params = object_arg(args, "params");
    if scope == "one" && !text(args, "scene_id").is_empty() {
        let kind = if matches!(kind.as_str(), "image" | "video" | "enhance" | "edit") { kind } else { "video".into() };
        return api.send("POST", &format!("{}/prompts/{kind}", scene(args)?), &params, None).await;
    }
    let mut body = Map::new();
    body.insert("kind".into(), json!(kind));
    body.insert("scope".into(), json!(scope));
    body.extend(params.as_object().cloned().unwrap_or_default());
    api.send("POST", &format!("{base}/prompts/batch"), &Value::Object(body), None).await
}

async fn instructions(api: &Api, args: &Args) -> Result<Value, ToolError> {
    let key = seg(&need(args, "key")?);
    let action = Some(text(args, "action")).filter(|a| !a.is_empty()).unwrap_or_else(|| "get".into()).to_lowercase();
    let project_id = text(args, "project_id");
    let scene_id = text(args, "scene_id");
    let scope = text(args, "scope");
    match action.as_str() {
        "save" => {
            let mut body = Map::new();
            body.insert("text".into(), json!(first_text(args, &["text", "prompt"])));
            body.insert("scope".into(), json!(if !scene_id.is_empty() && scope.is_empty() { "scene".to_string() } else if scope.is_empty() { "all".to_string() } else { scope }));
            if !scene_id.is_empty() {
                body.insert("scene_id".into(), json!(scene_id));
            }
            if !project_id.is_empty() {
                body.insert("project_id".into(), json!(project_id));
            }
            api.send("PUT", &format!("/instructions/{key}"), &Value::Object(body), None).await
        }
        "reset" => {
            let mut body = Map::new();
            body.insert("scope".into(), json!(if !scene_id.is_empty() && scope.is_empty() { "scene".to_string() } else if scope.is_empty() { "all".to_string() } else { scope }));
            if !scene_id.is_empty() {
                body.insert("scene_id".into(), json!(scene_id));
            }
            if !project_id.is_empty() {
                body.insert("project_id".into(), json!(project_id));
            }
            api.send("DELETE", &format!("/instructions/{key}/override"), &Value::Object(body), None).await
        }
        _ => api.get(&format!("/instructions/{key}"), &one("project_id", project_id)).await,
    }
}

async fn image_action(api: &Api, args: &Args) -> Result<Value, ToolError> {
    let base = format!("{}/image", scene(args)?);
    match Some(text(args, "action")).filter(|a| !a.is_empty()).unwrap_or_else(|| "approve".into()).to_lowercase().as_str() {
        "approve" => api.send("POST", &format!("{base}/approve"), &pick(args, &["image_path"]), None).await,
        "revert" => {
            let mut body = json!({"delta": args.get("delta").cloned().unwrap_or_else(|| json!(-1))});
            if let Some(index) = args.get("index") {
                body["index"] = index.clone();
            }
            api.send("POST", &format!("{base}/revert"), &body, None).await
        }
        "delete" => api.call("DELETE", &base, &[], None, None).await,
        other => Err(ToolError::validation(format!("Invalid image action: '{other}'."))),
    }
}

async fn latents(api: &Api, args: &Args) -> Result<Value, ToolError> {
    let base = project(args)?;
    match Some(text(args, "action")).filter(|a| !a.is_empty()).unwrap_or_else(|| "status".into()).to_lowercase().as_str() {
        "dirty" => api.get(&format!("{base}/latents/dirty"), &[]).await,
        "rebuild" => api.send("POST", &format!("{base}/latents/rebuild"), &object_arg(args, "params"), None).await,
        "delete" => {
            let scene_id = Some(text(args, "scene_id")).filter(|s| !s.is_empty()).unwrap_or_else(|| "scene_001".into());
            api.call("DELETE", &format!("{base}/scenes/{}/latent", seg(&scene_id)), &[("all".to_string(), flag(args, "all").to_string())], None, None).await
        }
        _ => api.get(&format!("{base}/latents"), &[]).await,
    }
}

async fn post_apply(api: &Api, args: &Args) -> Result<Value, ToolError> {
    let settings = object_arg(args, "settings");
    match Some(text(args, "op")).filter(|o| !o.is_empty()).unwrap_or_else(|| "lut".into()).to_lowercase().as_str() {
        "apply_all" => api.send("POST", &format!("{}/post/apply-all", project(args)?), &settings, None).await,
        "lut" => api.send("POST", &format!("{}/post/lut", scene(args)?), &settings, None).await,
        other => Err(ToolError::validation(format!("Invalid post operation '{other}'. Use lut (with scene_id) or apply_all."))),
    }
}

/// Wait for a job up to `timeout_seconds`, then answer with the job as it is.
async fn job_wait(api: &Api, args: &Args) -> Result<Value, ToolError> {
    let id = need(args, "job_id")?;
    let timeout = args.get("timeout_seconds").and_then(Value::as_f64).unwrap_or(60.0).clamp(1.0, 3600.0);
    let deadline = Instant::now() + Duration::from_secs_f64(timeout);
    let path = format!("/jobs/{}", seg(&id));
    let mut last = json!({});
    loop {
        let data = api.get(&path, &[]).await?;
        let job = data.get("job").cloned().unwrap_or(data);
        let status = job.get("status").and_then(Value::as_str).unwrap_or("").to_lowercase();
        if matches!(status.as_str(), "succeeded" | "completed" | "failed" | "cancelled" | "interrupted") {
            return Ok(job);
        }
        last = job;
        if Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let mut waiting = json!({"timeout": true, "message": format!("Job {id} is still running after {timeout} s. Call job_wait again.")});
    if let (Some(target), Some(fields)) = (waiting.as_object_mut(), last.as_object()) {
        target.extend(fields.clone());
    }
    Ok(waiting)
}

fn image_type(path: &str) -> Option<&'static str> {
    match path.rsplit('.').next()?.to_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        "gif" => Some("image/gif"),
        _ => None,
    }
}

/// An image of the project as the agent can look at it: a file, or a scene's video thumbnail or approved image.
async fn asset_view(api: &Api, args: &Args) -> Result<Output, ToolError> {
    let mut path = first_text(args, &["path", "asset_id"]);
    if path.is_empty() && !text(args, "project_id").is_empty() && !text(args, "scene_id").is_empty() {
        let found = api.get(&scene(args)?, &[]).await?;
        path = ["video_thumbnail", "approved_image"].iter().filter_map(|k| found.get(*k).and_then(|a| a.get("path")).and_then(Value::as_str)).next().unwrap_or("").to_string();
    }
    if path.is_empty() {
        return Err(ToolError::validation("Give `path` to an image file, or `project_id` and `scene_id` of a scene that has a picture."));
    }
    let Some(mime) = image_type(&path) else {
        return Err(ToolError::validation(format!("No image file found at: {path}")));
    };
    let (status, _, bytes) = api.raw("GET", "/files", &[("path".to_string(), path.clone())], None, None).await?;
    if status >= 400 {
        envelope(status, &bytes)?;
    }
    Ok(Output::Image { data: base64::engine::general_purpose::STANDARD.encode(&bytes), mime, info: json!({"path": path, "bytes": bytes.len()}) })
}

/// Where a project file can be fetched from by a client that is not this process.
async fn asset_url(api: &Api, args: &Args) -> Result<Value, ToolError> {
    let path = first_text(args, &["path", "asset_id"]);
    if path.is_empty() {
        return Err(ToolError::validation("path is required."));
    }
    let query = [("path".to_string(), path.clone())];
    let (status, headers, bytes) = api.raw("HEAD", "/files", &query, None, None).await?;
    if status >= 400 {
        let (_, _, body) = api.raw("GET", "/files", &query, None, None).await?;
        envelope(status, if bytes.is_empty() { &body } else { &bytes })?;
    }
    let size = headers.get(header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    Ok(json!({"path": path, "bytes": size, "url": format!("{}/files?path={}", api.base(), seg(&path))}))
}

async fn upload_file(api: &Api, args: &Args) -> Result<Value, ToolError> {
    let filename = need(args, "filename")?;
    let kind = Some(text(args, "kind")).filter(|k| !k.is_empty()).unwrap_or_else(|| "general".into()).to_lowercase();
    let content = text(args, "content");
    let (project_id, scene_id) = (text(args, "project_id"), text(args, "scene_id"));
    if kind == "lut" || filename.to_lowercase().ends_with(".cube") {
        return api.send("POST", "/post/luts/upload", &json!({"filename": filename, "data": content}), None).await;
    }
    if kind == "audio" && !project_id.is_empty() {
        return api.send("PUT", &format!("{}/audio", project(args)?), &json!({"audio_data": content, "audio_name": filename}), None).await;
    }
    if kind == "image" && !project_id.is_empty() && !scene_id.is_empty() {
        return api.send("PUT", &format!("{}/image", scene(args)?), &json!({"image_data": content}), None).await;
    }
    Err(ToolError::validation(
        "Uploads: kind 'lut' (a LUT file), 'audio' (with project_id) or 'image' (with project_id and scene_id). Files already on this machine do not need uploading: pass their path to audio_attach, reference_upsert or image_set_from_upload.",
    ))
}
