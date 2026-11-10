//! A small Chrome DevTools Protocol client: one WebSocket to the browser, calls matched to their answers by id, and the browser's
//! events passed on to whoever listens. Pages are reached through sessions (`Target.attachToTarget` with `flatten`), so one
//! connection serves every tab.
//!
//! Plain `ws://` to `127.0.0.1` only. Chrome is on the same computer, so there is no TLS here.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

use crate::error::{BrowserError, Result};

/// How long a call may take before it is given up. A call that waits for a promise in the page (`awaitPromise`) is the slowest.
const CALL_TIMEOUT: Duration = Duration::from_secs(120);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>>;

/// One event from the browser. `session` is empty for events of the browser itself.
#[derive(Clone, Debug)]
pub struct Event {
    pub session: Option<String>,
    pub method: String,
    pub params: Value,
}

pub struct Cdp {
    out: mpsc::UnboundedSender<String>,
    pending: Pending,
    next: AtomicU64,
    events: broadcast::Sender<Event>,
    /// Set when the socket has closed, so a call made afterwards ends at once instead of waiting for its time limit.
    closed: Arc<AtomicBool>,
}

impl Cdp {
    /// Connect to the browser's WebSocket (`webSocketDebuggerUrl` of `/json/version`).
    pub async fn connect(url: &str) -> Result<Arc<Cdp>> {
        let (socket, _) = tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(url))
            .await
            .map_err(|_| BrowserError::Chrome("Chrome did not accept the DevTools connection in time.".into()))?
            .map_err(|e| BrowserError::Chrome(format!("Chrome refused the DevTools connection: {e}")))?;
        let (mut sink, mut stream) = socket.split();
        let (out, mut out_rx) = mpsc::unbounded_channel::<String>();
        let pending: Pending = Arc::default();
        let (events, _) = broadcast::channel(2048);

        tokio::spawn(async move {
            while let Some(text) = out_rx.recv().await {
                if sink.send(Message::text(text)).await.is_err() {
                    break;
                }
            }
            let _ = sink.close().await;
        });

        let closed = Arc::new(AtomicBool::new(false));
        let reader_closed = Arc::clone(&closed);
        let reader_pending = Arc::clone(&pending);
        let reader_events = events.clone();
        tokio::spawn(async move {
            while let Some(Ok(message)) = stream.next().await {
                let Message::Text(text) = message else { continue };
                let Ok(value) = serde_json::from_str::<Value>(text.as_str()) else { continue };
                if let Some(id) = value.get("id").and_then(Value::as_u64) {
                    let answer = match value.get("error") {
                        Some(error) => Err(BrowserError::Protocol { method: String::new(), message: error.get("message").and_then(Value::as_str).unwrap_or("unknown error").to_string() }),
                        None => Ok(value.get("result").cloned().unwrap_or(Value::Null)),
                    };
                    if let Some(sender) = reader_pending.lock().unwrap().remove(&id) {
                        let _ = sender.send(answer);
                    }
                } else if let Some(method) = value.get("method").and_then(Value::as_str) {
                    let _ = reader_events.send(Event {
                        session: value.get("sessionId").and_then(Value::as_str).map(str::to_string),
                        method: method.to_string(),
                        params: value.get("params").cloned().unwrap_or(Value::Null),
                    });
                }
            }
            // The socket closed: every call still waiting is answered with that, and so is every call made from now on.
            reader_closed.store(true, Ordering::SeqCst);
            let waiting: Vec<_> = reader_pending.lock().unwrap().drain().collect();
            for (_, sender) in waiting {
                let _ = sender.send(Err(BrowserError::Closed));
            }
        });

        Ok(Arc::new(Cdp { out, pending, next: AtomicU64::new(1), events, closed }))
    }

    /// Connect to the Chrome that listens on `port` (started with `--remote-debugging-port`).
    pub async fn connect_port(port: u16) -> Result<Arc<Cdp>> {
        let client = reqwest::Client::builder().timeout(Duration::from_secs(5)).build().map_err(|e| BrowserError::Chrome(e.to_string()))?;
        let answer: Value = client
            .get(format!("http://127.0.0.1:{port}/json/version"))
            .send()
            .await
            .map_err(|_| BrowserError::Chrome(format!("No Chrome is listening on debug port {port}. Use \"Open and sign in\" first.")))?
            .json()
            .await
            .map_err(|e| BrowserError::Chrome(format!("Chrome on port {port} gave an unreadable answer: {e}")))?;
        let url = answer.get("webSocketDebuggerUrl").and_then(Value::as_str).ok_or_else(|| BrowserError::Chrome(format!("Chrome on port {port} gave no DevTools address.")))?;
        Cdp::connect(url).await
    }

    /// Call a DevTools method. `session` is the tab's session, none for the browser itself.
    pub async fn send(&self, session: Option<&str>, method: &str, params: Value) -> Result<Value> {
        self.send_within(session, method, params, CALL_TIMEOUT).await
    }

    pub async fn send_within(&self, session: Option<&str>, method: &str, params: Value, limit: Duration) -> Result<Value> {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, sender);
        // Checked after the call is registered: a socket that closes in between has either drained it or is seen here.
        if self.closed.load(Ordering::SeqCst) {
            self.pending.lock().unwrap().remove(&id);
            return Err(BrowserError::Closed);
        }
        let mut message = json!({"id": id, "method": method, "params": params});
        if let Some(session) = session {
            message["sessionId"] = json!(session);
        }
        if self.out.send(message.to_string()).is_err() {
            self.pending.lock().unwrap().remove(&id);
            return Err(BrowserError::Closed);
        }
        match tokio::time::timeout(limit, receiver).await {
            Ok(Ok(Err(BrowserError::Protocol { message, .. }))) => Err(BrowserError::Protocol { method: method.to_string(), message }),
            Ok(Ok(answer)) => answer,
            Ok(Err(_)) => Err(BrowserError::Closed),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(BrowserError::timeout(format!("Chrome did not answer {method} in time.")))
            }
        }
    }

    /// The events of the browser from now on. Subscribe before the action that causes the event, or it can be missed.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    /// Wait for the first event the test accepts. `None` after `limit`.
    pub async fn wait_event(receiver: &mut broadcast::Receiver<Event>, limit: Duration, mut accept: impl FnMut(&Event) -> bool) -> Option<Event> {
        let waiting = async {
            loop {
                match receiver.recv().await {
                    Ok(event) if accept(&event) => return Some(event),
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return None,
                }
            }
        };
        tokio::time::timeout(limit, waiting).await.ok().flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    /// A stand-in for Chrome: answers `Test.echo` with its params, `Test.fail` with an error, and sends `Test.later` as an event first
    /// when asked to.
    async fn fake_chrome() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            while let Some(Ok(Message::Text(text))) = socket.next().await {
                let call: Value = serde_json::from_str(text.as_str()).unwrap();
                let id = call["id"].clone();
                match call["method"].as_str().unwrap() {
                    "Test.echo" => {
                        socket.send(Message::text(json!({"id": id, "result": {"echo": call["params"], "session": call["sessionId"]}}).to_string())).await.unwrap();
                    }
                    "Test.fail" => {
                        socket.send(Message::text(json!({"id": id, "error": {"code": -32000, "message": "No such thing"}}).to_string())).await.unwrap();
                    }
                    "Test.event" => {
                        socket.send(Message::text(json!({"method": "Page.loadEventFired", "sessionId": "s1", "params": {"timestamp": 1.5}}).to_string())).await.unwrap();
                        socket.send(Message::text(json!({"id": id, "result": {}}).to_string())).await.unwrap();
                    }
                    "Test.hang" => {}
                    "Test.close" => {
                        socket.close(None).await.ok();
                        return;
                    }
                    other => panic!("unexpected call {other}"),
                }
            }
        });
        url
    }

    #[tokio::test]
    async fn a_call_gets_its_own_answer_and_the_session_is_sent_along() {
        let cdp = Cdp::connect(&fake_chrome().await).await.unwrap();
        let (a, b) = tokio::join!(cdp.send(Some("s9"), "Test.echo", json!({"n": 1})), cdp.send(None, "Test.echo", json!({"n": 2})));
        let (a, b) = (a.unwrap(), b.unwrap());
        assert_eq!((a["echo"]["n"].clone(), a["session"].clone()), (json!(1), json!("s9")));
        assert_eq!((b["echo"]["n"].clone(), b["session"].clone()), (json!(2), Value::Null));
    }

    #[tokio::test]
    async fn a_refused_call_names_the_method_and_the_reason() {
        let cdp = Cdp::connect(&fake_chrome().await).await.unwrap();
        let error = cdp.send(None, "Test.fail", json!({})).await.unwrap_err();
        assert_eq!(error, BrowserError::Protocol { method: "Test.fail".into(), message: "No such thing".into() });
        assert_eq!(error.to_string(), "Chrome refused Test.fail: No such thing");
    }

    #[tokio::test]
    async fn events_reach_a_listener_that_subscribed_before_the_call() {
        let cdp = Cdp::connect(&fake_chrome().await).await.unwrap();
        let mut events = cdp.subscribe();
        cdp.send(None, "Test.event", json!({})).await.unwrap();
        let event = Cdp::wait_event(&mut events, Duration::from_secs(2), |e| e.method == "Page.loadEventFired").await.expect("the event");
        assert_eq!((event.session.as_deref(), event.params["timestamp"].clone()), (Some("s1"), json!(1.5)));
        assert!(Cdp::wait_event(&mut events, Duration::from_millis(50), |_| true).await.is_none());
    }

    #[tokio::test]
    async fn a_call_that_is_never_answered_times_out_and_a_closed_socket_ends_the_calls() {
        let cdp = Cdp::connect(&fake_chrome().await).await.unwrap();
        let error = cdp.send_within(None, "Test.hang", json!({}), Duration::from_millis(100)).await.unwrap_err();
        assert!(matches!(error, BrowserError::Timeout(_)), "{error:?}");
        let error = cdp.send(None, "Test.close", json!({})).await.unwrap_err();
        assert_eq!(error, BrowserError::Closed);
        assert_eq!(cdp.send(None, "Test.echo", json!({})).await.unwrap_err(), BrowserError::Closed);
    }

    #[tokio::test]
    async fn no_chrome_on_the_port_says_to_open_it_first() {
        let error = Cdp::connect_port(9).await.err().expect("nothing listens on port 9");
        assert!(error.to_string().contains("No Chrome is listening on debug port 9"), "{error}");
    }
}
