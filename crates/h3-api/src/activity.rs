//! What an agent is doing, for the open Video Builder to show. Three things feed it: the MCP tools an agent calls (`mcp/mod.rs`), the REST calls a program makes
//! to the agent API (`rest_activity`, below), and the jobs that run on the server (`follow_jobs`). Each becomes an entry the Builder lists as it happens,
//! sent over the event stream (`/vrgdg/events`) as `vrgdg.agent_activity`. The last entries are kept for a window that opens later (`GET /api/v1/agent/activity`).
//! The project changes themselves arrive separately, as `vrgdg.project_changed` (see `h3_project::changes`).

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::extract::{Query, Request, State};
use axum::http::{header, Method};
use axum::middleware::Next;
use axum::response::Response;
use serde_json::{json, Map, Value};

use crate::app::{AppState, Shared};
use crate::error::{success, ApiResult};

/// How many entries are kept for a window that opens later.
const KEEP: usize = 300;

/// The feed: the last entries, and what feeds it.
#[derive(Default)]
pub struct Feed {
    entries: Mutex<VecDeque<Value>>,
    last_id: AtomicU64,
    following_jobs: AtomicBool,
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
}

/// `GET /agent/activity?after=N`: what the agent did lately, for a window that opened after it started.
pub async fn recent(State(state): Shared, Query(query): Query<HashMap<String, String>>) -> ApiResult {
    let after = query.get("after").and_then(|a| a.parse().ok()).unwrap_or(0);
    let items = state.recent_activity(after);
    Ok(success(json!({"items": items, "last_id": state.activity.last_id.load(Ordering::SeqCst)}), None))
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

// -- jobs -------------------------------------------------------------------------------------------------------------------

/// Turn the server's job events into feed entries, one per job that changes as the job goes on. Called once.
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
            let status = match job["status"].as_str().unwrap_or("") {
                "succeeded" => "done",
                "failed" | "interrupted" => "failed",
                "cancelled" => "cancelled",
                _ => "running",
            };
            let progress = &job["progress"];
            let message = progress["message"].as_str().filter(|m| !m.is_empty()).map(str::to_string).unwrap_or_else(|| progress["stage"].as_str().unwrap_or("").to_string());
            let error = job["error"]["message"].as_str().unwrap_or("");
            let shown = if status == "failed" && !error.is_empty() { error.to_string() } else { message };
            let signature = format!("{status}|{shown}|{}", progress["percent"].as_f64().unwrap_or(0.0).round());
            if entries.get(job_id).is_some_and(|(_, last)| *last == signature) {
                continue;
            }
            let mut fields = json!({
                "source": "job", "tool": job["type"], "title": job_title(job["type"].as_str().unwrap_or("job")), "status": status, "message": shown,
                "job_id": job_id, "percent": progress["percent"],
            });
            if let Some(project) = job["project_id"].as_str() {
                fields["project_id"] = json!(project);
            }
            if let Some(scene) = progress["scene_id"].as_str().filter(|s| !s.is_empty()) {
                fields["scene_id"] = json!(scene);
            }
            let id = state.activity(entries.get(job_id).map(|(id, _)| *id), fields);
            if status == "running" {
                entries.insert(job_id.to_string(), (id, signature));
            } else {
                entries.remove(job_id);
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
    let started = Instant::now();
    let response = next.run(request).await;
    let rest = api_path(&path).unwrap_or(&path);
    let title = crate::mcp::describe_endpoint(method.as_str(), rest).unwrap_or_else(|| format!("{method} {rest}"));
    let status = response.status();
    let mut fields = json!({
        "source": "api", "tool": format!("{method} {rest}"), "title": title, "detail": format!("{method} {rest}"), "status": if status.is_success() { "done" } else { "failed" },
        "duration_ms": started.elapsed().as_millis() as u64, "http_status": status.as_u16(),
        "project_id": after_segment(rest, "projects"), "scene_id": after_segment(rest, "scenes"),
    });
    if !status.is_success() {
        fields["message"] = json!(format!("The app answered {status}."));
    }
    state.activity(None, fields);
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
}
