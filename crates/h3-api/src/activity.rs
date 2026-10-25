//! What an agent is doing, for the open Video Builder to show. Four things feed it: an agent connecting to the MCP server (`mcp/mod.rs`), the MCP tools an
//! agent calls (the same), the REST calls a program makes to the agent API (`rest_activity`, below), and the jobs that run on the server (`follow_jobs`).
//! Each becomes an entry the Builder lists as it happens, sent over the event stream (`/vrgdg/events`) as `vrgdg.agent_activity`. The last entries are kept
//! for a window that opens later (`GET /api/v1/agent/activity`).
//!
//! An entry that works in a project also learns what it touched: a save of that project while the entry runs (`h3_project::changes`) adds the scenes and
//! fields it changed to the entry's `touched` list. The project changes themselves arrive separately, as `vrgdg.project_changed`.
//!
//! The scene a job is working on is told to the Builder as `vrgdg.scene_work`, so a scene an agent renders looks like one the user renders.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Query, Request, State};
use axum::http::{header, Method};
use axum::middleware::Next;
use axum::response::Response;
use serde_json::{json, Map, Value};

use crate::app::{AppState, Shared};
use crate::error::{success, ApiResult};

/// How many entries are kept for a window that opens later.
const KEEP: usize = 300;

/// How many scenes or fields an entry lists as touched.
const TOUCHED_KEEP: usize = 40;

/// The feed: the last entries, and what feeds it.
#[derive(Default)]
pub struct Feed {
    entries: Mutex<VecDeque<Value>>,
    last_id: AtomicU64,
    following_jobs: AtomicBool,
    /// The agent that announced itself last and when, so a client that starts every call with `initialize` is announced once.
    last_client: Mutex<Option<(String, Instant)>>,
    /// Calls and jobs that are running: entry id to the project (lower case) they work in. A save of that project is what they touched.
    open: Mutex<HashMap<u64, String>>,
    /// The scene each job works on now: job id to what the Builder was told.
    work: Mutex<HashMap<String, Value>>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

impl AppState {
    /// A message for the open Builder windows, `{"type": ..., "data": ...}` on the event stream.
    pub fn tell_builder(&self, kind: &str, data: Value) {
        let _ = self.builder_events.send(json!({"type": kind, "data": data}).to_string());
    }

    /// Add an entry to the feed (`id` None), or change the one with that `id`, and tell the Builder windows. Returns the entry's id.
    pub fn activity(&self, id: Option<u64>, fields: Value) -> u64 {
        let feed = &self.activity;
        let entry = {
            let mut entries = feed.entries.lock().unwrap_or_else(|e| e.into_inner());
            match id.and_then(|wanted| entries.iter_mut().find(|e| e["id"] == json!(wanted))) {
                Some(found) => {
                    if let (Some(target), Some(patch)) = (found.as_object_mut(), fields.as_object()) {
                        target.extend(patch.clone());
                    }
                    found.clone()
                }
                None => {
                    let mut entry = fields;
                    let new_id = id.unwrap_or_else(|| feed.last_id.fetch_add(1, Ordering::SeqCst) + 1);
                    entry["id"] = json!(new_id);
                    entry["time"] = json!(now_ms());
                    entries.push_back(entry.clone());
                    while entries.len() > KEEP {
                        entries.pop_front();
                    }
                    entry
                }
            }
        };
        self.tell_builder("vrgdg.agent_activity", entry.clone());
        entry["id"].as_u64().unwrap_or(0)
    }

    /// The entries after `after`, oldest first.
    pub fn recent_activity(&self, after: u64) -> Vec<Value> {
        self.activity.entries.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|e| e["id"].as_u64().unwrap_or(0) > after).cloned().collect()
    }

    /// An agent said hello to the MCP server (`initialize`). It is announced once for a client that keeps saying it.
    pub fn agent_connected(&self, params: &Value) {
        let info = &params["clientInfo"];
        let clean = |key: &str| info[key].as_str().map(str::trim).filter(|t| !t.is_empty()).map(str::to_string);
        let name = clean("title").or_else(|| clean("name")).unwrap_or_else(|| "An agent".to_string());
        let who = match clean("version") {
            Some(version) => format!("{name} {version}"),
            None => name,
        };
        {
            let mut last = self.activity.last_client.lock().unwrap_or_else(|e| e.into_inner());
            let repeat = last.as_ref().is_some_and(|(seen, at)| *seen == who && at.elapsed() < Duration::from_secs(60));
            *last = Some((who.clone(), Instant::now()));
            if repeat {
                return;
            }
        }
        let protocol = params["protocolVersion"].as_str().map(|p| format!(", protocol {p}")).unwrap_or_default();
        self.activity(None, json!({
            "source": "mcp", "kind": "connect", "tool": "initialize", "title": "Agent connected", "detail": format!("{who} over MCP{protocol}"),
            "client": who, "status": "done",
        }));
    }

    /// Start the entry of a call that works in `project` (empty when it names none). Saves of that project while the call runs are what it touched.
    pub fn begin_call(&self, project: &str, fields: Value) -> u64 {
        let id = self.activity(None, fields);
        self.watch_project(id, project);
        id
    }

    /// Remember that the entry `id` works in `project`, so a save of the project is listed as touched by it.
    pub fn watch_project(&self, id: u64, project: &str) {
        if !project.trim().is_empty() {
            self.activity.open.lock().unwrap_or_else(|e| e.into_inner()).insert(id, project.trim().to_lowercase());
        }
    }

    /// The entry `id` no longer works in a project: what is saved from now on is not its doing.
    pub fn unwatch(&self, id: u64) {
        self.activity.open.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
    }

    /// The entry of a call that has ended. `patch` is its final state.
    pub fn end_call(&self, id: u64, patch: Value) {
        self.unwatch(id);
        self.activity(Some(id), patch);
    }

    /// A project was saved by something other than the Builder: the running entries that work in it touched what `change` describes.
    pub fn touched(&self, project: &str, change: &Value) {
        let wanted = project.trim().to_lowercase();
        let ids: Vec<u64> = self.activity.open.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|(_, p)| **p == wanted).map(|(id, _)| *id).collect();
        for id in ids {
            let mut items: Vec<Value> = {
                let entries = self.activity.entries.lock().unwrap_or_else(|e| e.into_inner());
                entries.iter().find(|e| e["id"] == json!(id)).and_then(|e| e["touched"].as_array().cloned()).unwrap_or_default()
            };
            if merge_touched(&mut items, change) {
                self.activity(Some(id), json!({"touched": items}));
            }
        }
    }

    /// Tell the Builder which scene a job works on (`scene` None: it works on none now). `what` is `video` or `image`. The Builder shows the scene as rendering
    /// for `video` and a badge on the card for both. The scene before the new one is told to stop.
    pub fn scene_work(&self, job_id: &str, project: &str, scene: Option<(&str, &'static str)>, ok: bool, detail: Value) {
        let (stopped, started) = {
            let mut work = self.activity.work.lock().unwrap_or_else(|e| e.into_inner());
            let previous = work.get(job_id).cloned();
            match scene {
                Some((scene_id, what)) => {
                    let mut now = json!({"job_id": job_id, "project_id": project, "scene_id": scene_id, "what": what});
                    if let (Some(target), Some(extra)) = (now.as_object_mut(), detail.as_object()) {
                        target.extend(extra.clone());
                    }
                    work.insert(job_id.to_string(), now.clone());
                    let moved = previous.filter(|p| p["scene_id"] != json!(scene_id) || p["what"] != json!(what));
                    (moved, Some(now))
                }
                None => (work.remove(job_id), None),
            }
        };
        if let Some(mut before) = stopped {
            before["state"] = json!("off");
            before["ok"] = json!(ok);
            self.tell_builder("vrgdg.scene_work", before);
        }
        if let Some(mut now) = started {
            now["state"] = json!("on");
            self.tell_builder("vrgdg.scene_work", now);
        }
    }

    /// The scenes jobs work on now, for a window that opens late.
    pub fn scenes_in_work(&self) -> Vec<Value> {
        self.activity.work.lock().unwrap_or_else(|e| e.into_inner()).values().cloned().collect()
    }
}

/// Add what a change touched to an entry's list. Returns whether the list changed.
fn merge_touched(items: &mut Vec<Value>, change: &Value) -> bool {
    let before = items.clone();
    let mut add = |scene_id: Option<&str>, fields: Vec<String>, flag: Option<&str>| {
        let found = items.iter_mut().find(|item| match scene_id {
            Some(id) => item["scene_id"] == json!(id),
            None => flag.is_some_and(|f| item.get(f).is_some()),
        });
        match found {
            Some(item) if scene_id.is_some() => {
                let mut all: Vec<String> = item["fields"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
                for field in fields {
                    if !all.contains(&field) {
                        all.push(field);
                    }
                }
                item["fields"] = json!(all);
            }
            Some(_) => {}
            None => {
                let mut item = Map::new();
                if let Some(id) = scene_id {
                    item.insert("scene_id".into(), json!(id));
                    item.insert("fields".into(), json!(fields));
                }
                if let Some(flag) = flag {
                    item.insert(flag.into(), json!(true));
                }
                items.push(Value::Object(item));
            }
        }
    };
    match change["kind"].as_str() {
        Some("scene_fields") => {
            if let Some(scenes) = change["scenes"].as_object() {
                for (scene_id, info) in scenes {
                    let strings = |key: &str| -> Vec<String> { info[key].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default() };
                    let mut fields = strings("segment");
                    fields.extend(strings("references").into_iter().map(|r| match r.as_str() {
                        "subjects" => "characters".to_string(),
                        "locations" => "location".to_string(),
                        _ => r,
                    }));
                    add(Some(scene_id), fields, None);
                }
            }
            if change["marker_ids"].as_array().is_some_and(|m| !m.is_empty()) {
                add(None, Vec::new(), Some("notes"));
            }
        }
        _ => add(None, Vec::new(), Some("project")),
    }
    items.truncate(TOUCHED_KEEP);
    *items != before
}

/// `GET /agent/activity?after=N`: what the agent did lately, for a window that opened after it started, and the scenes jobs work on now.
pub async fn recent(State(state): Shared, Query(query): Query<HashMap<String, String>>) -> ApiResult {
    let after = query.get("after").and_then(|a| a.parse().ok()).unwrap_or(0);
    let items = state.recent_activity(after);
    Ok(success(json!({"items": items, "last_id": state.activity.last_id.load(Ordering::SeqCst), "scenes_in_work": state.scenes_in_work()}), None))
}

// -- words for the feed -----------------------------------------------------------------------------------------------------

/// `scene_update` as "Scene update".
pub fn humanize(name: &str) -> String {
    let spaced = name.trim_start_matches("api_").replace(['_', '.'], " ");
    let mut chars = spaced.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().collect::<String>() + chars.as_str())
}

fn job_title(kind: &str) -> String {
    match kind {
        "video.render" => "Rendering a scene",
        "videos.render_batch" => "Rendering scenes",
        "video.stitch" => "Stitching the final video",
        "video.trim" => "Trimming a scene video",
        "video.match_start_color" => "Matching a scene's opening colors",
        "video.slideshow" => "Making a slideshow",
        "image.generate" => "Making a scene image",
        "images.generate_batch" => "Making scene images",
        "reference.image.generate" => "Making a reference picture",
        "lyrics.align" => "Timing the lyrics",
        "timeline.from_lines" => "Making scenes from the lyrics",
        "pipeline.build_full_video" => "Building the full video",
        "pipeline.from_song" => "Building a video from a song",
        "latents.rebuild" => "Rebuilding latents",
        "post.lut" => "Applying a LUT",
        "post.apply_all" => "Applying a LUT to every scene",
        "minimax.prompts" => "Writing MiniMax prompts",
        "reference.describe" => "Describing a reference picture",
        "reference.extract_locations" => "Finding filming locations",
        "reference.image_prompt" => "Writing a reference picture prompt",
        "llm.scene_image_prompt" => "Writing a scene image prompt",
        "llm.batch_prompts" => "Writing image prompts",
        "llm.concepts" => "Writing concept prompts",
        "llm.motion_notes" => "Writing motion notes",
        "llm.enhance_prompt" => "Improving a prompt",
        "llm.edit_prompt" => "Editing a prompt",
        "elevenlabs.craft_dialogue" => "Writing dialogue",
        "elevenlabs.voice_description" => "Writing a voice description",
        other => return humanize(other),
    }
    .to_string()
}

/// Whether the percent of this kind of job moves with the work. The others (one call to an LLM, one pass of ffmpeg) have no way to say how far they are, and
/// the number they set is only a marker of which step they are in, so the feed shows their message and a clock instead.
fn measured(kind: &str) -> bool {
    matches!(
        kind,
        "video.render" | "videos.render_batch" | "image.generate" | "images.generate_batch" | "pipeline.build_full_video" | "pipeline.from_song" | "lyrics.align"
            | "llm.batch_prompts" | "post.apply_all"
    )
}

/// What a job working on a scene does there, as the Builder shows it: `video`, `image`, or `None` when it is not working on the scene (a scene that is
/// finished, the stitch).
fn work_of(kind: &str, stage: &str) -> Option<&'static str> {
    if matches!(stage, "completed" | "stitching" | "queued" | "started") {
        return None;
    }
    match kind {
        "image.generate" | "images.generate_batch" => Some("image"),
        "video.render" | "videos.render_batch" => Some("video"),
        "pipeline.build_full_video" | "pipeline.from_song" => Some(if stage == "generating_image" { "image" } else { "video" }),
        _ => None,
    }
}

/// An argument that must not appear in the feed: a key, a token, or a file's contents.
fn private(name: &str) -> bool {
    let lower = name.to_lowercase();
    ["key", "token", "password", "secret"].iter().any(|word| lower.contains(word)) || lower.ends_with("_data") || lower == "data" || lower == "content"
}

fn describe(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(s) if s.chars().count() <= 40 => Some(format!("\"{s}\"")),
        Value::String(s) => Some(format!("{} characters", s.chars().count())),
        Value::Array(items) => Some(format!("{} items", items.len())),
        Value::Object(map) => Some(format!("{{{}}}", map.keys().filter(|k| !private(k)).take(6).cloned().collect::<Vec<_>>().join(", "))),
        other => Some(other.to_string()),
    }
}

/// The project and the scene a tool call is about, and what it was told, in few words.
pub fn summarize(args: &Map<String, Value>) -> (String, String, String) {
    let text = |key: &str| args.get(key).and_then(Value::as_str).map(str::trim).unwrap_or_default().to_string();
    let project = [text("project_id"), text("project_name")].into_iter().find(|t| !t.is_empty()).unwrap_or_default();
    let scene = [text("scene_id"), args.get("scene_ids").and_then(Value::as_array).filter(|a| a.len() == 1).and_then(|a| a[0].as_str()).unwrap_or_default().to_string()].into_iter().find(|t| !t.is_empty()).unwrap_or_default();
    let parts: Vec<String> = args.iter().filter(|(k, _)| !matches!(k.as_str(), "project_id" | "project_name" | "scene_id" | "if_match_revision") && !private(k)).filter_map(|(k, v)| describe(v).map(|d| format!("{k}: {d}"))).collect();
    let mut detail = parts.join(", ");
    if detail.chars().count() > 160 {
        detail = detail.chars().take(157).collect::<String>() + "...";
    }
    (project, scene, detail)
}

/// What a tool answered, in few words: its short fields and the size of its lists. Paths and anything private are left out.
pub fn result_summary(data: &Value) -> String {
    let Some(map) = data.as_object() else { return String::new() };
    let parts: Vec<String> = map
        .iter()
        .filter(|(k, _)| !private(k) && !k.ends_with("_path") && !k.ends_with("_url") && k.as_str() != "job")
        .filter_map(|(k, v)| match v {
            Value::Bool(_) | Value::Number(_) => Some(format!("{k}: {v}")),
            Value::String(s) if !s.is_empty() && s.chars().count() <= 30 => Some(format!("{k}: {s}")),
            Value::Array(items) => Some(format!("{k}: {}", items.len())),
            _ => None,
        })
        .take(6)
        .collect();
    let mut text = parts.join(", ");
    if text.chars().count() > 160 {
        text = text.chars().take(157).collect::<String>() + "...";
    }
    text
}

// -- jobs -------------------------------------------------------------------------------------------------------------------

/// Turn the server's job events into feed entries, one per job that changes as the job goes on, and tell the Builder which scene each job works on. Called once.
pub fn follow_jobs(state: &Arc<AppState>) {
    if state.activity.following_jobs.swap(true, Ordering::SeqCst) {
        return;
    }
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        state.activity.following_jobs.store(false, Ordering::SeqCst);
        return;
    };
    let mut events = state.jobs.subscribe();
    let state = Arc::clone(state);
    runtime.spawn(async move {
        // Job id -> its entry, and what that entry last said, so a repeated progress message is not announced again.
        let mut entries: HashMap<String, (u64, String)> = HashMap::new();
        loop {
            let event = match events.recv().await {
                Ok(event) => event,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            };
            if event.name != "job.created" && event.name != "job.updated" {
                continue;
            }
            let job = &event.data;
            let Some(job_id) = job["id"].as_str() else { continue };
            let kind = job["type"].as_str().unwrap_or("");
            let project = job["project_id"].as_str().unwrap_or("");
            let status = match job["status"].as_str().unwrap_or("") {
                "succeeded" => "done",
                "failed" | "interrupted" => "failed",
                "cancelled" => "cancelled",
                _ => "running",
            };
            let progress = &job["progress"];
            let stage = progress["stage"].as_str().unwrap_or("");
            let message = progress["message"].as_str().filter(|m| !m.is_empty()).map(str::to_string).unwrap_or_else(|| stage.to_string());
            let error = job["error"]["message"].as_str().unwrap_or("");
            let shown = if status == "failed" && !error.is_empty() { error.to_string() } else { message.clone() };
            let percent = progress["percent"].as_f64().unwrap_or(0.0);

            // The scene the job works on now. A job that ended works on none.
            let scene = progress["scene_id"].as_str().filter(|s| !s.is_empty());
            let working = if status == "running" { scene.zip(work_of(kind, stage)) } else { None };
            let single_scene = matches!(kind, "video.render" | "image.generate");
            let mut detail = json!({"message": message, "stage": stage});
            if single_scene && measured(kind) {
                detail["percent"] = json!(percent);
            }
            state.scene_work(job_id, project, working, status != "failed", detail);

            let signature = format!("{status}|{shown}|{}", percent.round());
            if entries.get(job_id).is_some_and(|(_, last)| *last == signature) {
                continue;
            }
            let mut fields = json!({
                "source": "job", "kind": "job", "tool": kind, "title": job_title(if kind.is_empty() { "job" } else { kind }), "status": status, "message": shown,
                "job_id": job_id, "stage": stage,
            });
            // A percent that only marks a step of one long call is not shown as progress.
            if measured(kind) {
                fields["percent"] = json!(percent);
            }
            if progress["scene_count"].as_i64().unwrap_or(0) > 1 {
                fields["scene_index"] = progress["scene_index"].clone();
                fields["scene_count"] = progress["scene_count"].clone();
            }
            if !project.is_empty() {
                fields["project_id"] = json!(project);
            }
            if let Some(scene) = scene {
                fields["scene_id"] = json!(scene);
            }
            let known = entries.get(job_id).map(|(id, _)| *id);
            let id = state.activity(known, fields);
            if status == "running" {
                if known.is_none() {
                    state.watch_project(id, project);
                }
                entries.insert(job_id.to_string(), (id, signature));
            } else {
                entries.remove(job_id);
                state.unwatch(id);
            }
        }
    });
}

// -- REST calls ------------------------------------------------------------------------------------------------------------

/// The path of an agent API call without its prefix, or `None` for a route that is not the agent API.
fn api_path(path: &str) -> Option<&str> {
    path.strip_prefix("/vrgdg/api/v1").or_else(|| path.strip_prefix("/api/v1")).filter(|rest| rest.starts_with('/'))
}

/// The segment after `name` in an API path (`/projects/Demo/scenes/seg_1` and `scenes` give `seg_1`).
fn after_segment(path: &str, name: &str) -> String {
    let mut parts = path.split('/').filter(|p| !p.is_empty());
    while let Some(part) = parts.next() {
        if part == name {
            return parts.next().map(percent_decode).unwrap_or_default();
        }
    }
    String::new()
}

fn percent_decode(text: &str) -> String {
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

/// A program (not a browser: it sends no `Origin`) that changes something through the agent API appears in the feed. The Builder's own calls carry an
/// `Origin`, and the MCP tools are reported by the MCP server, which calls the API without going through here.
pub async fn rest_activity(State(state): State<Arc<AppState>>, request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let from_a_program = request.headers().get(header::ORIGIN).is_none();
    let reported = from_a_program && matches!(method, Method::POST | Method::PUT | Method::PATCH | Method::DELETE) && api_path(&path).is_some_and(|rest| !rest.starts_with("/setup") && !rest.starts_with("/jobs"));
    if !reported {
        return next.run(request).await;
    }
    let client = request.headers().get(header::USER_AGENT).and_then(|v| v.to_str().ok()).map(|ua| ua.chars().take(80).collect::<String>()).unwrap_or_default();
    let started = Instant::now();
    let rest = api_path(&path).unwrap_or(&path).to_string();
    let title = crate::mcp::describe_endpoint(method.as_str(), &rest).unwrap_or_else(|| format!("{method} {rest}"));
    let call = format!("{method} {rest}");
    let project = after_segment(&rest, "projects");
    // The entry exists while the call runs, so a save it makes is listed as touched by it.
    let id = state.begin_call(
        &project,
        json!({
            "source": "api", "kind": "write", "tool": call, "title": title, "detail": call, "api": call, "status": "running", "client": client,
            "project_id": project, "scene_id": after_segment(&rest, "scenes"),
        }),
    );
    let response = next.run(request).await;
    let status = response.status();
    let mut patch = json!({"status": if status.is_success() { "done" } else { "failed" }, "duration_ms": started.elapsed().as_millis() as u64, "http_status": status.as_u16()});
    if !status.is_success() {
        patch["message"] = json!(format!("The app answered {status}."));
    }
    state.end_call(id, patch);
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_made_readable() {
        assert_eq!(humanize("scene_update"), "Scene update");
        assert_eq!(humanize("api_post_timeline_snap"), "Post timeline snap");
        assert_eq!(job_title("minimax.prompts"), "Writing MiniMax prompts");
        assert_eq!(job_title("some.new_kind"), "Some new kind");
    }

    #[test]
    fn a_summary_names_what_the_call_changes_and_hides_what_is_private() {
        let args = json!({
            "project_id": "Demo", "scene_id": "seg_1", "if_match_revision": 4,
            "patch": {"label": "Opening", "minimax_h3_prompt": "x"}, "lyrics": "a very long lyric line that goes on and on past the limit of forty characters",
            "api_key": "sk-secret", "audio_data": "AAAA", "ripple": true,
        });
        let (project, scene, detail) = summarize(args.as_object().unwrap());
        assert_eq!((project.as_str(), scene.as_str()), ("Demo", "seg_1"));
        assert!(detail.contains("patch: {label, minimax_h3_prompt}"), "{detail}");
        assert!(detail.contains("lyrics: 77 characters"), "{detail}");
        assert!(detail.contains("ripple: true"), "{detail}");
        assert!(!detail.contains("sk-secret") && !detail.contains("api_key") && !detail.contains("AAAA"), "{detail}");
    }

    #[test]
    fn a_single_scene_in_a_list_is_the_scene() {
        let args = json!({"project_id": "Demo", "scene_ids": ["seg_9"]});
        assert_eq!(summarize(args.as_object().unwrap()).1, "seg_9");
    }

    #[test]
    fn project_and_scene_come_out_of_an_api_path() {
        assert_eq!(api_path("/api/v1/projects/Demo/scenes/seg_1"), Some("/projects/Demo/scenes/seg_1"));
        assert_eq!(api_path("/vrgdg/api/v1/projects"), Some("/projects"));
        assert_eq!(api_path("/vrgdg/music_builder/save_session"), None);
        assert_eq!(after_segment("/projects/Demo%20Song/scenes/seg_1/video/render", "projects"), "Demo Song");
        assert_eq!(after_segment("/projects/Demo%20Song/scenes/seg_1/video/render", "scenes"), "seg_1");
        assert_eq!(after_segment("/projects", "scenes"), "");
    }

    #[test]
    fn an_answer_is_summed_up_without_paths_or_secrets() {
        let data = json!({
            "scene_id": "seg_3", "revision": 7, "updated": true, "video_path": "C:/secret/folder/video.mp4", "scenes": [1, 2, 3],
            "api_key": "sk-secret", "note": "a note that is much longer than thirty characters", "job": {"id": "x"},
        });
        let text = result_summary(&data);
        assert_eq!(text, "scene_id: seg_3, revision: 7, updated: true, scenes: 3");
        assert_eq!(result_summary(&json!("plain")), "");
    }

    #[test]
    fn only_jobs_that_measure_their_work_show_a_percent() {
        for kind in ["video.render", "videos.render_batch", "image.generate", "pipeline.build_full_video", "lyrics.align"] {
            assert!(measured(kind), "{kind}");
        }
        // One call to an LLM sets a marker (20%) and nothing more: showing it as progress is what looked stuck.
        for kind in ["llm.concepts", "llm.enhance_prompt", "video.trim", "post.lut", "minimax.prompts"] {
            assert!(!measured(kind), "{kind}");
        }
    }

    #[test]
    fn a_scene_is_in_work_while_its_job_renders_or_makes_it_a_picture() {
        assert_eq!(work_of("video.render", "rendering"), Some("video"));
        assert_eq!(work_of("videos.render_batch", "batch_videos"), Some("video"));
        assert_eq!(work_of("image.generate", "generating"), Some("image"));
        assert_eq!(work_of("pipeline.build_full_video", "generating_image"), Some("image"));
        assert_eq!(work_of("pipeline.build_full_video", "rendering_video"), Some("video"));
        assert_eq!(work_of("video.render", "completed"), None, "the scene is finished");
        assert_eq!(work_of("video.stitch", "stitching"), None);
        assert_eq!(work_of("llm.concepts", "generating"), None, "writing text is not working on a scene's picture");
    }

    #[test]
    fn what_a_save_touched_is_listed_by_scene_and_merged() {
        let mut items = Vec::new();
        let first = json!({"kind": "scene_fields", "scenes": {"seg_1": {"segment": ["label", "minimax_h3_prompt"], "references": ["locations"]}}, "marker_ids": []});
        assert!(merge_touched(&mut items, &first));
        assert_eq!(items, vec![json!({"scene_id": "seg_1", "fields": ["label", "minimax_h3_prompt", "location"]})]);
        // The same scene again adds only the new field. The same save twice changes nothing.
        let second = json!({"kind": "scene_fields", "scenes": {"seg_1": {"segment": ["label", "notes"], "references": []}, "seg_2": {"segment": ["start"], "references": []}}});
        assert!(merge_touched(&mut items, &second));
        assert_eq!(items[0]["fields"], json!(["label", "minimax_h3_prompt", "location", "notes"]));
        assert_eq!(items[1], json!({"scene_id": "seg_2", "fields": ["start"]}));
        assert!(!merge_touched(&mut items, &second));
        // Anything that is not scene fields is the project, listed once.
        assert!(merge_touched(&mut items, &json!({"kind": "project"})));
        assert!(!merge_touched(&mut items, &json!({"kind": "project"})));
        assert_eq!(items.last().unwrap(), &json!({"project": true}));
    }
}
