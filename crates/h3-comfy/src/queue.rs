//! Running a graph: queue it, wait for it, read what it produced. Port of `agent_api/orchestrator/comfy_client.py`.
//! Waiting polls `/history/{id}` once a second, like the pack, so it needs no WebSocket and survives a restart of the
//! app mid render. Live step progress through the WebSocket comes with the job system.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::client::{ComfyClient, ComfyError};

/// What `POST /prompt` answered.
#[derive(Debug, Clone)]
pub struct Queued {
    pub prompt_id: String,
    pub number: i64,
}

const CLIENT_ID: &str = "vrgdg_h3_studio";

impl ComfyClient {
    /// Send a prompt graph to ComfyUI. A graph ComfyUI cannot validate comes back as [`ComfyError::Queue`].
    pub async fn queue_prompt(&self, prompt: &Value) -> Result<Queued, ComfyError> {
        let body = json!({"prompt": prompt, "client_id": CLIENT_ID});
        let (status, text) = self.post_text("/prompt", &body).await?;
        let parsed: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if status >= 400 {
            let message = parsed
                .get("error")
                .map(|e| e.get("message").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| e.to_string()))
                .unwrap_or_else(|| format!("HTTP {status}: {}", text.chars().take(500).collect::<String>()));
            let node_errors = parsed.get("node_errors").cloned().unwrap_or(Value::Null);
            return Err(ComfyError::Queue { message: describe_node_errors(&message, &node_errors), node_errors });
        }
        if let Some(errors) = parsed.get("node_errors").filter(|e| e.as_object().is_some_and(|o| !o.is_empty())) {
            return Err(ComfyError::Queue { message: describe_node_errors("Workflow node validation failed", errors), node_errors: errors.clone() });
        }
        let prompt_id = parsed.get("prompt_id").and_then(Value::as_str).unwrap_or("").to_string();
        if prompt_id.is_empty() {
            return Err(ComfyError::Queue { message: "ComfyUI did not return a prompt id.".into(), node_errors: Value::Null });
        }
        Ok(Queued { prompt_id, number: parsed.get("number").and_then(Value::as_i64).unwrap_or(0) })
    }

    /// `GET /history/{id}`. Empty until the prompt has run.
    pub async fn history(&self, prompt_id: &str) -> Value {
        self.get_value(&format!("/history/{prompt_id}")).await.unwrap_or(Value::Null)
    }

    /// `GET /queue`: running and pending prompts.
    pub async fn queue_state(&self) -> Value {
        self.get_value("/queue").await.unwrap_or_else(|_| json!({"queue_running": [], "queue_pending": []}))
    }

    /// `POST /interrupt`: stop the prompt that is running.
    pub async fn interrupt(&self) -> bool {
        self.post_text("/interrupt", &Value::Null).await.map(|(status, _)| status == 200).unwrap_or(false)
    }

    /// Remove pending prompts from the queue.
    pub async fn delete_queue_items(&self, prompt_ids: &[String]) -> bool {
        if prompt_ids.is_empty() {
            return true;
        }
        self.post_text("/queue", &json!({"delete": prompt_ids})).await.map(|(status, _)| status == 200).unwrap_or(false)
    }

    /// Poll until the prompt finishes. Returns its history entry. Setting `cancel` interrupts ComfyUI and returns
    /// [`ComfyError::Cancelled`]. `on_status` hears each poll.
    pub async fn wait_for_prompt(
        &self,
        prompt_id: &str,
        timeout: Duration,
        poll: Duration,
        cancel: &Arc<AtomicBool>,
        mut on_status: impl FnMut(&str),
    ) -> Result<Value, ComfyError> {
        let started = Instant::now();
        while started.elapsed() < timeout {
            if cancel.load(Ordering::Relaxed) {
                self.interrupt().await;
                self.delete_queue_items(&[prompt_id.to_string()]).await;
                return Err(ComfyError::Cancelled);
            }
            let history = self.history(prompt_id).await;
            if history.as_object().is_some_and(|o| !o.is_empty()) {
                if let Some(error) = prompt_error(&history, prompt_id) {
                    return Err(ComfyError::Execution(error));
                }
                if prompt_finished(&history, prompt_id) {
                    return Ok(history);
                }
            }
            on_status("Waiting for ComfyUI to finish...");
            tokio::time::sleep(poll).await;
        }
        Err(ComfyError::Timeout { prompt_id: prompt_id.to_string(), seconds: timeout.as_secs_f64() })
    }
}

/// Text that nodes such as Show Text put in the prompt's outputs.
pub fn extract_texts(history: &Value, prompt_id: &str) -> Vec<String> {
    let mut texts = Vec::new();
    let Some(outputs) = entry(history, prompt_id).get("outputs").and_then(Value::as_object) else { return texts };
    for out in outputs.values().filter(|o| o.is_object()) {
        let direct = out.get("text").filter(|t| !t.is_null() && !t.as_str().is_some_and(str::is_empty) && !t.as_array().is_some_and(Vec::is_empty));
        let value = direct.or_else(|| out.get("ui").and_then(|u| u.get("text")));
        match value {
            Some(Value::Array(items)) => texts.extend(items.iter().filter_map(|i| match i {
                Value::String(s) if !s.is_empty() => Some(s.clone()),
                Value::Null | Value::Bool(false) | Value::String(_) => None,
                other => Some(other.to_string()),
            })),
            Some(Value::String(s)) if !s.trim().is_empty() => texts.push(s.clone()),
            _ => {}
        }
    }
    texts
}

/// ComfyUI's per node errors as readable lines under the headline.
fn describe_node_errors(message: &str, node_errors: &Value) -> String {
    let Some(map) = node_errors.as_object().filter(|m| !m.is_empty()) else { return message.to_string() };
    let mut lines = vec![message.to_string()];
    for (node_id, info) in map {
        let class = info.get("class_type").and_then(Value::as_str).unwrap_or("?");
        for error in info.get("errors").and_then(Value::as_array).into_iter().flatten() {
            let text = error.get("message").and_then(Value::as_str).unwrap_or("");
            let details = error.get("details").and_then(Value::as_str).unwrap_or("");
            lines.push(format!("- node {node_id} ({class}): {text} {details}").trim_end().to_string());
        }
    }
    lines.join("\n")
}

fn entry<'a>(history: &'a Value, prompt_id: &str) -> &'a Value {
    history.get(prompt_id).unwrap_or(history)
}

/// Execution exceptions and node errors in a history entry, as text. `None` when the prompt is fine.
pub fn prompt_error(history: &Value, prompt_id: &str) -> Option<String> {
    let root = entry(history, prompt_id).as_object()?;
    let mut messages: Vec<String> = Vec::new();
    if let Some(status) = root.get("status").and_then(Value::as_object) {
        let state = status.get("status_str").and_then(Value::as_str).unwrap_or("").to_lowercase();
        if !state.is_empty() && state != "success" && state != "completed" {
            messages.push(format!("status: {state}"));
        }
        for message in status.get("messages").and_then(Value::as_array).into_iter().flatten() {
            if let Some(text) = message.as_str() {
                messages.push(text.to_string());
            } else if message.is_object() {
                messages.push(message.to_string());
            } else if let Some(pair) = message.as_array() {
                // ComfyUI's own events are `[name, data]` pairs. Only a failed node says anything useful.
                if pair.first().and_then(Value::as_str) == Some("execution_error") {
                    let data = pair.get(1).cloned().unwrap_or(Value::Null);
                    let text = |key: &str| data.get(key).and_then(Value::as_str).unwrap_or("").to_string();
                    let reason: String = text("exception_message").chars().take(600).collect();
                    messages.push(format!("node {} ({}): {}", text("node_id"), text("node_type"), reason.trim()));
                }
            }
        }
        if let Some(text) = status.get("exception_message").filter(|v| !v.is_null()) {
            messages.push(text.as_str().map(str::to_string).unwrap_or_else(|| text.to_string()));
        }
    }
    if let Some(error) = root.get("error").filter(|v| !v.is_null() && *v != &json!("")) {
        messages.push(error.as_str().map(str::to_string).unwrap_or_else(|| error.to_string()));
    }
    if messages.is_empty() {
        return None;
    }
    // A failed node can print whole tensors. The start of the text names the problem.
    let text = messages.join("\n");
    if text.chars().count() > 3000 {
        return Some(format!("{}\n... (cut, {} characters in all)", text.chars().take(3000).collect::<String>(), text.chars().count()));
    }
    Some(text)
}

/// Whether the prompt has stopped running, successfully or not.
pub fn prompt_finished(history: &Value, prompt_id: &str) -> bool {
    let Some(root) = entry(history, prompt_id).as_object() else { return false };
    if root.is_empty() {
        return false;
    }
    if let Some(status) = root.get("status").and_then(Value::as_object) {
        let state = status.get("status_str").and_then(Value::as_str).unwrap_or("").to_lowercase();
        if matches!(state.as_str(), "success" | "completed" | "error" | "failed") {
            return true;
        }
    }
    root.get("outputs").and_then(Value::as_object).is_some_and(|o| !o.is_empty()) || root.get("error").is_some_and(|e| !e.is_null())
}

/// Videos a prompt saved. Some graphs save more than one (2 Pass Advanced also saves its Pass 1 backup), so `node_id`
/// limits the result to the video the caller wants when that node produced one.
pub fn extract_videos(history: &Value, prompt_id: &str, node_id: Option<&str>) -> Vec<Value> {
    let root = entry(history, prompt_id);
    let Some(outputs) = root.get("outputs").and_then(Value::as_object) else { return vec![] };
    let from_node = |out: &Value| -> Vec<Value> {
        let mut found = Vec::new();
        for key in ["videos", "gifs", "animated"] {
            if let Some(list) = out.get(key).and_then(Value::as_array) {
                found.extend(list.iter().cloned());
            }
        }
        found
    };
    if let Some(id) = node_id {
        if let Some(out) = outputs.get(id) {
            let wanted = from_node(out);
            if !wanted.is_empty() {
                return wanted;
            }
        }
    }
    outputs.values().flat_map(from_node).collect()
}

/// Images a prompt produced, in node order (`filename`, `subfolder`, `type`). `nodes` limits the result to those nodes
/// when any of them produced an image.
pub fn extract_images(history: &Value, prompt_id: &str, nodes: &[String]) -> Vec<Value> {
    let root = entry(history, prompt_id);
    let Some(outputs) = root.get("outputs").and_then(Value::as_object) else { return vec![] };
    let images_of = |out: &Value| -> Vec<Value> { out.get("images").and_then(Value::as_array).cloned().unwrap_or_default() };
    let wanted: Vec<Value> = nodes.iter().filter_map(|id| outputs.get(id)).flat_map(images_of).collect();
    if !wanted.is_empty() {
        return wanted;
    }
    outputs.values().flat_map(images_of).collect()
}

/// Where ComfyUI keeps its files.
#[derive(Debug, Clone)]
pub struct ComfyDirs {
    pub output: String,
    pub input: String,
    pub temp: String,
}

impl ComfyDirs {
    /// Work the folders out from how ComfyUI was started and where it keeps its custom nodes.
    ///
    /// ComfyUI reports its command line, but `main.py` is often relative (`ComfyUI\main.py` in the portable build). The
    /// absolute `custom_nodes` folder from `/internal/folder_paths` anchors it: `main.py` lives next to that folder, and
    /// the folder ComfyUI was started in is what is left above it. `--output-directory` and friends are resolved against
    /// that folder, else the folders are `output`, `input` and `temp` next to `main.py`. Returns `None` when neither
    /// source gives an absolute location.
    pub fn detect(argv: &[String], folder_paths: Option<&Value>) -> Option<Self> {
        use std::path::{Path, PathBuf};
        let from_nodes: Option<PathBuf> = folder_paths
            .and_then(|f| f.get("custom_nodes"))
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(Value::as_str)
            .and_then(|p| Path::new(p).parent().map(Path::to_path_buf))
            .filter(|p| p.is_absolute());
        let main = argv.first().map(PathBuf::from);
        let main_dir = from_nodes.or_else(|| main.as_ref().filter(|m| m.is_absolute()).and_then(|m| m.parent().map(Path::to_path_buf)))?;
        // The folder ComfyUI was started in: `main_dir` without the relative folder part of `main.py`'s path.
        let start_dir = {
            let relative = main.as_ref().filter(|m| m.is_relative()).and_then(|m| m.parent()).filter(|p| !p.as_os_str().is_empty());
            match relative {
                Some(rel) if main_dir.ends_with(rel) => {
                    let mut up = main_dir.clone();
                    for _ in rel.components() {
                        up.pop();
                    }
                    up
                }
                _ => main_dir.clone(),
            }
        };
        let option = |name: &str| -> Option<String> {
            let mut it = argv.iter();
            while let Some(arg) = it.next() {
                if arg == name {
                    return it.next().cloned();
                }
                if let Some(value) = arg.strip_prefix(&format!("{name}=")) {
                    return Some(value.to_string());
                }
            }
            None
        };
        let resolve = |value: &str| -> String {
            let path = Path::new(value);
            if path.is_absolute() { value.to_string() } else { start_dir.join(path).display().to_string() }
        };
        let base = option("--base-directory").map(|b| resolve(&b));
        let pick = |flag: &str, folder: &str| -> String {
            if let Some(v) = option(flag) {
                return resolve(&v);
            }
            match &base {
                Some(b) => Path::new(b).join(folder).display().to_string(),
                None => main_dir.join(folder).display().to_string(),
            }
        };
        Some(Self { output: pick("--output-directory", "output"), input: pick("--input-directory", "input"), temp: pick("--temp-directory", "temp") })
    }

    /// The full path of a video ComfyUI reported (`filename`, `subfolder`, `type`).
    pub fn resolve_output(&self, info: &Value) -> String {
        let filename = std::path::Path::new(info.get("filename").and_then(Value::as_str).unwrap_or(""))
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let subfolder = info.get("subfolder").and_then(Value::as_str).unwrap_or("");
        let base = match info.get("type").and_then(Value::as_str).unwrap_or("output").to_lowercase().as_str() {
            "temp" => &self.temp,
            "input" => &self.input,
            _ => &self.output,
        };
        std::path::absolute(std::path::Path::new(base).join(subfolder).join(filename))
            .map(|p| p.display().to_string())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finished_and_failed_histories() {
        let ok = json!({"p1": {"status": {"status_str": "success", "messages": []}, "outputs": {"142": {"gifs": [{"filename": "a.mp4"}]}}}});
        assert!(prompt_finished(&ok, "p1"));
        assert_eq!(prompt_error(&ok, "p1"), None);
        let failed = json!({"p1": {"status": {"status_str": "error", "messages": ["boom"], "exception_message": "out of memory"}}});
        assert!(prompt_finished(&failed, "p1"));
        assert_eq!(prompt_error(&failed, "p1").unwrap(), "status: error\nboom\nout of memory");
        assert!(!prompt_finished(&json!({}), "p1"));
        assert!(!prompt_finished(&json!({"p1": {"status": {"status_str": "running"}}}), "p1"));
    }

    #[test]
    fn comfyui_event_pairs_are_not_errors_unless_a_node_failed() {
        let ok = json!({"p": {"status": {"status_str": "success", "messages": [
            ["execution_start", {"prompt_id": "p"}], ["execution_cached", {"nodes": []}], ["execution_success", {"prompt_id": "p"}]]}}});
        assert_eq!(prompt_error(&ok, "p"), None);
        let failed = json!({"p": {"status": {"status_str": "error", "messages": [
            ["execution_start", {"prompt_id": "p"}],
            ["execution_error", {"node_id": "125", "node_type": "SamplerCustomAdvanced", "exception_message": "addmm: shapes cannot be multiplied"}]]}}});
        assert_eq!(prompt_error(&failed, "p").unwrap(), "status: error\nnode 125 (SamplerCustomAdvanced): addmm: shapes cannot be multiplied");
        let long = json!({"p": {"status": {"status_str": "error", "exception_message": "x".repeat(5000)}}});
        assert!(prompt_error(&long, "p").unwrap().contains("(cut, "));
    }

    #[test]
    fn the_final_video_is_taken_from_node_142() {
        let history = json!({"p": {"outputs": {
            "9308": {"gifs": [{"filename": "backup_stage1.mp4", "subfolder": "x", "type": "output"}]},
            "142": {"gifs": [{"filename": "final.mp4", "subfolder": "x", "type": "output"}]},
        }}});
        let videos = extract_videos(&history, "p", Some("142"));
        assert_eq!(videos.len(), 1);
        assert_eq!(videos[0]["filename"], "final.mp4");
        assert_eq!(extract_videos(&history, "p", None).len(), 2);
        assert_eq!(extract_videos(&history, "p", Some("nope")).len(), 2);
    }

    #[test]
    fn folders_come_from_the_command_line() {
        let argv: Vec<String> = ["C:\\Comfy\\ComfyUI\\main.py", "--output-directory", "D:\\out", "--disable-auto-launch"].iter().map(|s| s.to_string()).collect();
        let dirs = ComfyDirs::detect(&argv, None).unwrap();
        assert_eq!(dirs.output, "D:\\out");
        assert!(dirs.input.ends_with("input"));
        let video = json!({"filename": "../evil.mp4", "subfolder": "VRGDG_MiniMaxH3\\p_1\\scene_0001", "type": "output"});
        let path = dirs.resolve_output(&video);
        assert!(path.starts_with("D:\\out") && path.ends_with("evil.mp4") && !path.contains(".."));
    }

    #[test]
    fn a_relative_main_py_is_anchored_by_the_custom_nodes_folder() {
        let argv: Vec<String> = ["ComfyUI\\main.py", "--output-directory", "render_out"].iter().map(|s| s.to_string()).collect();
        let folders = json!({"custom_nodes": ["C:\\Portable\\ComfyUI\\custom_nodes"]});
        let dirs = ComfyDirs::detect(&argv, Some(&folders)).unwrap();
        assert_eq!(dirs.output, "C:\\Portable\\render_out");
        assert_eq!(dirs.input, "C:\\Portable\\ComfyUI\\input");
        let plain = ComfyDirs::detect(&["ComfyUI\\main.py".to_string()], Some(&folders)).unwrap();
        assert_eq!(plain.output, "C:\\Portable\\ComfyUI\\output");
        assert!(ComfyDirs::detect(&["ComfyUI\\main.py".to_string()], None).is_none());
    }

    #[test]
    fn node_errors_become_readable_lines() {
        let errors = json!({"9": {"class_type": "VAELoader", "errors": [{"message": "Value not in list", "details": "vae_name: 'x'"}]}});
        let text = describe_node_errors("Prompt outputs failed validation", &errors);
        assert!(text.contains("node 9 (VAELoader): Value not in list vae_name: 'x'"));
    }
}
