//! Live progress of a running prompt, from ComfyUI's WebSocket (`/ws`).
//!
//! ComfyUI tells the client that queued a prompt which node it runs and how far that node is (`progress`, `value` of `max`). Waiting on `/history` still
//! decides when the prompt is done. This only adds what happens meanwhile. Without it (an https address, a socket that closed) the wait works as before and
//! only the elapsed time moves.
//!
//! The estimate counts the nodes that take the time of a render (the samplers, upscalers, decoders and any node that has reported steps), because most nodes of a
//! graph finish at once and would make a count of all nodes jump to 90% before the first step. A node that only picks a sampler (`KSamplerSelect`) is not one.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{Stream, StreamExt};
use serde_json::Value;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::{Error as SocketError, Message};

use crate::client::ComfyClient;

/// ComfyUI's nodes that sample. Other `Sampler...` nodes (`KSamplerSelect`, `SamplerEulerAncestral`) only make the sampler object and finish at once.
const SAMPLERS: [&str; 4] = ["ksampler", "ksampleradvanced", "samplercustom", "samplercustomadvanced"];

/// Whether a node of this class takes the time of a render: a sampler, or a node that upscales or decodes.
fn heavy_class(class: &str) -> bool {
    let class = class.to_lowercase();
    SAMPLERS.contains(&class.as_str()) || class.contains("upscal") || class.contains("decode")
}

/// A client id that no other wait in this program uses. ComfyUI sends the progress of a prompt to the one socket that has the client id the prompt was
/// queued with, so two waits sharing an id would take each other's messages.
pub fn unique_client_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!("vrgdg_h3_studio_{}_{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst))
}

/// How far a prompt is, as far as ComfyUI has said.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Live {
    /// About how much of the whole graph is done, 0 to 1. It never goes down.
    pub fraction: f64,
    /// The node that runs now (its title, else its class).
    pub node: Option<String>,
    /// The step of that node: `(value, max)`.
    pub step: Option<(u64, u64)>,
    /// Seconds left for the running node, from the pace of its steps.
    pub eta_seconds: Option<f64>,
}

/// `4:21` or `1:02:03`.
pub fn clock(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as u64;
    let (h, m, s) = (total / 3600, total % 3600 / 60, total % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

impl Live {
    /// One line for a job's message: what runs, how far, how long.
    pub fn describe(&self, label: &str, elapsed: Duration) -> String {
        let mut parts = vec![label.to_string()];
        if let Some(node) = &self.node {
            match self.step {
                Some((value, max)) if max > 0 => parts.push(format!("{node} step {value} of {max}")),
                _ => parts.push(format!("running {node}")),
            }
        }
        if let Some(eta) = self.eta_seconds {
            parts.push(format!("about {} left", clock(eta)));
        }
        parts.push(format!("{} elapsed", clock(elapsed.as_secs_f64())));
        parts.join(" - ")
    }
}

/// What ComfyUI said about one prompt so far.
#[derive(Debug, Clone, Default)]
struct PromptState {
    cached: HashSet<String>,
    /// Nodes in the order they started, each once.
    started: Vec<String>,
    current: Option<String>,
    value: u64,
    max: u64,
    /// When the running node first reported a step, and which step that was, for the time left.
    first_step: Option<(Instant, u64)>,
    /// Nodes that reported steps.
    reported: HashSet<String>,
    finished: bool,
    peak: f64,
}

impl PromptState {
    fn start(&mut self, node: String) {
        if self.current.as_deref() == Some(node.as_str()) {
            return;
        }
        if !self.started.contains(&node) {
            self.started.push(node.clone());
        }
        self.current = Some(node);
        self.value = 0;
        self.max = 0;
        self.first_step = None;
    }

    fn step(&mut self, value: u64, max: u64) {
        if max == 0 {
            return;
        }
        if self.first_step.is_none() {
            self.first_step = Some((Instant::now(), value));
        }
        self.value = value.min(max);
        self.max = max;
        if let Some(node) = &self.current {
            self.reported.insert(node.clone());
        }
    }

    fn finish(&mut self) {
        self.finished = true;
        self.current = None;
    }

    fn live(&mut self, graph: &Value) -> Live {
        let nodes = graph.as_object();
        let class_of = |id: &str| nodes.and_then(|n| n.get(id)).and_then(|n| n.get("class_type")).and_then(Value::as_str).unwrap_or("").to_string();
        let is_heavy = |id: &str| self.reported.contains(id) || heavy_class(&class_of(id));
        let to_run: Vec<&String> = nodes.map(|n| n.keys().filter(|k| !self.cached.contains(*k)).collect()).unwrap_or_default();
        let heavy_total = to_run.iter().filter(|id| is_heavy(id)).count();
        let inner = if self.max > 0 { (self.value as f64 / self.max as f64).clamp(0.0, 1.0) } else { 0.0 };
        let behind = || self.started.iter().filter(|id| Some(id.as_str()) != self.current.as_deref());
        let raw = if self.finished {
            1.0
        } else if heavy_total > 0 {
            let done = behind().filter(|id| is_heavy(id)).count();
            let running = if self.current.as_deref().is_some_and(is_heavy) { inner } else { 0.0 };
            ((done as f64 + running) / heavy_total as f64).min(0.99)
        } else if !to_run.is_empty() {
            ((behind().count() as f64 + inner) / to_run.len() as f64).min(0.99)
        } else {
            0.0
        };
        self.peak = self.peak.max(raw);

        let node = self.current.as_deref().map(|id| {
            let title = nodes.and_then(|n| n.get(id)).and_then(|n| n.get("_meta")).and_then(|m| m.get("title")).and_then(Value::as_str).unwrap_or("").trim().to_string();
            if !title.is_empty() { title } else { Some(class_of(id)).filter(|c| !c.is_empty()).unwrap_or_else(|| format!("node {id}")) }
        });
        let step = (self.max > 0).then_some((self.value, self.max));
        let eta_seconds = self.first_step.and_then(|(at, first)| {
            let done = self.value.checked_sub(first)?;
            let seconds = at.elapsed().as_secs_f64();
            (done >= 2 && seconds >= 2.0 && self.max > self.value).then(|| seconds / done as f64 * (self.max - self.value) as f64)
        });
        Live { fraction: self.peak, node, step, eta_seconds }
    }
}

fn text_of(value: &Value) -> Option<String> {
    match value {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// One message of ComfyUI's socket. Messages that are not about a prompt (status, previews) are left out.
fn apply(states: &mut HashMap<String, PromptState>, message: &Value) {
    let Some(kind) = message.get("type").and_then(Value::as_str) else { return };
    let data = message.get("data").unwrap_or(&Value::Null);
    let Some(prompt_id) = data.get("prompt_id").and_then(Value::as_str) else { return };
    let state = states.entry(prompt_id.to_string()).or_default();
    match kind {
        "execution_cached" => {
            state.cached = data.get("nodes").and_then(Value::as_array).map(|nodes| nodes.iter().filter_map(text_of).collect()).unwrap_or_default();
        }
        // The node is null when the whole graph has run.
        "executing" => match data.get("node").and_then(text_of) {
            Some(node) => state.start(node),
            None => state.finish(),
        },
        "progress" => {
            if let Some(node) = data.get("node").and_then(text_of) {
                state.start(node);
            }
            state.step(data.get("value").and_then(Value::as_u64).unwrap_or(0), data.get("max").and_then(Value::as_u64).unwrap_or(0));
        }
        // Newer ComfyUI lists the progress of every node that reports it.
        "progress_state" => {
            let running = data.get("nodes").and_then(Value::as_object).and_then(|nodes| nodes.values().find(|n| n.get("state").and_then(Value::as_str) == Some("running")));
            if let Some(running) = running {
                if let Some(node) = running.get("node_id").or_else(|| running.get("display_node_id")).and_then(text_of) {
                    state.start(node);
                }
                state.step(running.get("value").and_then(Value::as_u64).unwrap_or(0), running.get("max").and_then(Value::as_u64).unwrap_or(0));
            }
        }
        "execution_success" | "execution_error" | "execution_interrupted" => state.finish(),
        _ => {}
    }
}

async fn read_messages<S>(mut socket: S, states: Arc<Mutex<HashMap<String, PromptState>>>)
where
    S: Stream<Item = Result<Message, SocketError>> + Unpin,
{
    while let Some(Ok(message)) = socket.next().await {
        // Binary messages are preview pictures.
        let Message::Text(text) = message else { continue };
        let Ok(value) = serde_json::from_str::<Value>(text.as_str()) else { continue };
        apply(&mut states.lock().unwrap_or_else(|e| e.into_inner()), &value);
    }
}

/// An open socket to ComfyUI that collects the progress of the prompts queued with its client id. Dropping it closes the socket.
pub struct ProgressFeed {
    states: Arc<Mutex<HashMap<String, PromptState>>>,
    reader: JoinHandle<()>,
}

impl ProgressFeed {
    /// What is known about a prompt. `graph` is the prompt that was queued, which names the nodes. `None` until ComfyUI has said something about it.
    pub fn live(&self, prompt_id: &str, graph: &Value) -> Option<Live> {
        self.states.lock().unwrap_or_else(|e| e.into_inner()).get_mut(prompt_id).map(|state| state.live(graph))
    }
}

impl Drop for ProgressFeed {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

impl ComfyClient {
    /// `ws://host/ws?clientId=...` for an `http://` address. An `https://` address has none: this build has no TLS for sockets.
    fn socket_url(&self, client_id: &str) -> Option<String> {
        let rest = self.base_url().strip_prefix("http://")?;
        Some(format!("ws://{rest}/ws?clientId={client_id}"))
    }

    /// Open the progress socket. Call it before queueing the prompt, with the client id the prompt will carry, so no message is missed. `None` when the
    /// socket cannot be opened: the caller then waits without live progress.
    pub async fn progress_feed(&self, client_id: &str) -> Option<ProgressFeed> {
        let url = self.socket_url(client_id)?;
        let (socket, _) = tokio::time::timeout(Duration::from_secs(3), tokio_tungstenite::connect_async(url)).await.ok()?.ok()?;
        let states = Arc::new(Mutex::new(HashMap::new()));
        let reader = tokio::spawn(read_messages(socket, Arc::clone(&states)));
        Some(ProgressFeed { states, reader })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::SinkExt;
    use serde_json::json;

    /// A graph with a loader, two samplers (a first and a second pass) and a decoder, as a prompt looks.
    fn graph() -> Value {
        json!({
            "1": {"class_type": "CheckpointLoader", "inputs": {}},
            "2": {"class_type": "CLIPTextEncode", "inputs": {}},
            "3": {"class_type": "SamplerCustomAdvanced", "inputs": {}, "_meta": {"title": "Pass 1 sampler"}},
            "4": {"class_type": "SamplerCustomAdvanced", "inputs": {}},
            "5": {"class_type": "VAEDecode", "inputs": {}},
            "6": {"class_type": "SaveVideo", "inputs": {}},
        })
    }

    fn say(states: &mut HashMap<String, PromptState>, kind: &str, data: Value) {
        let mut data = data;
        data["prompt_id"] = json!("p1");
        apply(states, &json!({"type": kind, "data": data}));
    }

    fn live(states: &mut HashMap<String, PromptState>) -> Live {
        states.get_mut("p1").unwrap().live(&graph())
    }

    #[test]
    fn cheap_nodes_do_not_move_the_estimate_but_sampler_steps_do() {
        let mut states = HashMap::new();
        say(&mut states, "execution_start", json!({}));
        say(&mut states, "executing", json!({"node": "1"}));
        say(&mut states, "executing", json!({"node": "2"}));
        assert_eq!(live(&mut states).fraction, 0.0, "loaders and encoders are not the render");
        say(&mut states, "executing", json!({"node": "3"}));
        say(&mut states, "progress", json!({"node": "3", "value": 15, "max": 30}));
        let half_of_pass_one = live(&mut states);
        // Three heavy nodes (two samplers and the decoder): half of the first is a sixth of the whole.
        assert!((half_of_pass_one.fraction - 0.5 / 3.0).abs() < 1e-9, "{half_of_pass_one:?}");
        assert_eq!(half_of_pass_one.node.as_deref(), Some("Pass 1 sampler"));
        assert_eq!(half_of_pass_one.step, Some((15, 30)));
        say(&mut states, "executing", json!({"node": "4"}));
        say(&mut states, "progress", json!({"node": "4", "value": 30, "max": 30}));
        let second_pass_done = live(&mut states);
        assert!((second_pass_done.fraction - 2.0 / 3.0).abs() < 1e-9, "{second_pass_done:?}");
        assert_eq!(second_pass_done.node.as_deref(), Some("SamplerCustomAdvanced"), "a node without a title shows its class");
    }

    #[test]
    fn a_node_that_only_picks_a_sampler_is_not_the_render() {
        let mut states = HashMap::new();
        let with_selectors = json!({
            "1": {"class_type": "KSamplerSelect"}, "2": {"class_type": "SamplerEulerAncestral"},
            "3": {"class_type": "SamplerCustomAdvanced"}, "4": {"class_type": "VAEDecode"},
        });
        say(&mut states, "executing", json!({"node": "1"}));
        say(&mut states, "executing", json!({"node": "2"}));
        say(&mut states, "executing", json!({"node": "3"}));
        // Two selectors have run, and the sampler has not taken its first step: the render has not started.
        assert_eq!(states.get_mut("p1").unwrap().live(&with_selectors).fraction, 0.0);
        say(&mut states, "progress", json!({"node": "3", "value": 10, "max": 20}));
        // Half of one of two heavy nodes.
        assert!((states.get_mut("p1").unwrap().live(&with_selectors).fraction - 0.25).abs() < 1e-9);
    }

    #[test]
    fn the_estimate_never_goes_down_and_ends_at_one() {
        let mut states = HashMap::new();
        say(&mut states, "executing", json!({"node": "3"}));
        say(&mut states, "progress", json!({"node": "3", "value": 30, "max": 30}));
        let high = live(&mut states).fraction;
        // A node that reports again from step 0 (a retry, a second batch) must not pull the bar back.
        say(&mut states, "progress", json!({"node": "3", "value": 1, "max": 30}));
        assert!(live(&mut states).fraction >= high);
        say(&mut states, "executing", json!({"node": null}));
        assert_eq!(live(&mut states).fraction, 1.0);
        assert_eq!(live(&mut states).node, None);
    }

    #[test]
    fn cached_nodes_are_not_counted() {
        let mut states = HashMap::new();
        // The first sampler's result is cached: only the second sampler and the decoder run.
        say(&mut states, "execution_cached", json!({"nodes": ["1", "2", "3"]}));
        say(&mut states, "executing", json!({"node": "4"}));
        say(&mut states, "progress", json!({"node": "4", "value": 10, "max": 20}));
        assert!((live(&mut states).fraction - 0.25).abs() < 1e-9, "half of one of two heavy nodes");
    }

    #[test]
    fn the_newer_progress_state_message_counts_like_a_progress_message() {
        let mut states = HashMap::new();
        say(&mut states, "progress_state", json!({"nodes": {
            "3": {"node_id": "3", "state": "finished", "value": 30, "max": 30},
            "4": {"node_id": "4", "state": "running", "value": 6, "max": 12},
        }}));
        let seen = live(&mut states);
        assert_eq!(seen.step, Some((6, 12)));
        assert_eq!(seen.node.as_deref(), Some("SamplerCustomAdvanced"));
    }

    #[test]
    fn a_graph_with_no_heavy_nodes_counts_every_node() {
        let mut states = HashMap::new();
        let plain = json!({"1": {"class_type": "A"}, "2": {"class_type": "B"}, "3": {"class_type": "C"}, "4": {"class_type": "D"}});
        say(&mut states, "executing", json!({"node": "1"}));
        say(&mut states, "executing", json!({"node": "2"}));
        // One node is done, the second runs: a quarter.
        assert!((states.get_mut("p1").unwrap().live(&plain).fraction - 0.25).abs() < 1e-9);
    }

    #[test]
    fn messages_about_nothing_or_for_other_prompts_are_ignored() {
        let mut states = HashMap::new();
        apply(&mut states, &json!({"type": "status", "data": {"status": {"exec_info": {"queue_remaining": 1}}}}));
        apply(&mut states, &json!({"type": "executing", "data": {"node": "1"}}));
        apply(&mut states, &json!("not an object"));
        assert!(states.is_empty());
    }

    #[test]
    fn the_time_left_comes_from_the_pace_of_the_steps() {
        let mut state = PromptState::default();
        state.start("3".into());
        state.step(0, 10);
        // Pretend the first step was seen 10 seconds ago and four steps have run since.
        state.first_step = Some((Instant::now() - Duration::from_secs(10), 0));
        state.step(4, 10);
        let eta = state.live(&graph()).eta_seconds.expect("an estimate");
        assert!((eta - 15.0).abs() < 0.5, "10 s for 4 steps leaves 15 s for 6: {eta}");
    }

    #[test]
    fn the_message_says_what_runs_and_how_long() {
        let live = Live { fraction: 0.4, node: Some("Pass 1 sampler".into()), step: Some((12, 30)), eta_seconds: Some(130.0) };
        assert_eq!(live.describe("Rendering with ComfyUI", Duration::from_secs(261)), "Rendering with ComfyUI - Pass 1 sampler step 12 of 30 - about 2:10 left - 4:21 elapsed");
        assert_eq!(Live::default().describe("Rendering with ComfyUI", Duration::from_secs(3725)), "Rendering with ComfyUI - 1:02:05 elapsed");
        assert_eq!(clock(59.6), "1:00");
    }

    #[test]
    fn a_client_id_is_never_used_twice() {
        assert_ne!(unique_client_id(), unique_client_id());
    }

    #[test]
    fn only_a_plain_address_has_a_socket() {
        assert_eq!(ComfyClient::new("http://127.0.0.1:8188/").socket_url("abc").as_deref(), Some("ws://127.0.0.1:8188/ws?clientId=abc"));
        assert_eq!(ComfyClient::new("https://comfy.example").socket_url("abc"), None);
    }

    #[tokio::test]
    async fn a_feed_reads_what_a_comfyui_socket_sends() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            for (kind, data) in [
                ("status", json!({"status": {"exec_info": {"queue_remaining": 1}}})),
                ("executing", json!({"node": "3", "prompt_id": "p1"})),
                ("progress", json!({"node": "3", "value": 15, "max": 30, "prompt_id": "p1"})),
            ] {
                socket.send(Message::text(json!({"type": kind, "data": data}).to_string())).await.unwrap();
            }
            // A preview picture is binary and must not stop the reader.
            socket.send(Message::binary(vec![1u8, 0, 0, 0, 1])).await.unwrap();
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
        let client = ComfyClient::new(&format!("http://{address}"));
        let feed = client.progress_feed("test").await.expect("the socket opens");
        let mut seen = None;
        for _ in 0..50 {
            seen = feed.live("p1", &graph());
            if seen.as_ref().is_some_and(|l| l.step.is_some()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let seen = seen.expect("progress arrived");
        assert_eq!(seen.step, Some((15, 30)));
        assert_eq!(seen.node.as_deref(), Some("Pass 1 sampler"));
        assert!(feed.live("another prompt", &graph()).is_none());
    }

    #[tokio::test]
    async fn no_socket_means_no_feed_and_no_wait() {
        // Nothing listens on this port.
        let started = Instant::now();
        assert!(ComfyClient::new("http://127.0.0.1:9").progress_feed("test").await.is_none());
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
