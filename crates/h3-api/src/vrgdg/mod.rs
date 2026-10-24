//! The pack's `/vrgdg/...` routes, which the Video Builder front end (copied from the pack) calls. The request and
//! reply shapes follow the pack's handlers (`builder/routes.py`, `runner/routes.py` and the others) so the front end runs
//! unchanged. A route that is not built yet answers 501 with a message that names it, and the server logs the call.
//!
//! Anything that is not a `/vrgdg` route goes to ComfyUI: the Builder uses `/prompt`, `/history`, `/queue`, `/view` and
//! `/interrupt` of ComfyUI itself.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderName, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde_json::json;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use crate::app::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/vrgdg/events", get(events))
}

/// Server events for the Builder (what ComfyUI sends over its WebSocket, for example that a project was changed from
/// outside). Each message is `{"type": ..., "data": ...}`.
async fn events(State(state): State<Arc<AppState>>) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let stream = BroadcastStream::new(state.builder_events.subscribe()).filter_map(|message| message.ok()).map(|message| Ok(Event::default().data(message)));
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

/// A `{"ok": false, "error": ...}` reply the Builder shows as an error message.
pub fn failure(status: StatusCode, message: impl Into<String>) -> Response {
    (status, axum::Json(json!({"ok": false, "error": message.into()}))).into_response()
}

const HOP_HEADERS: [&str; 8] = ["connection", "keep-alive", "proxy-authenticate", "proxy-authorization", "te", "trailers", "transfer-encoding", "upgrade"];

fn forwarded(headers: &HeaderMap) -> reqwest::header::HeaderMap {
    let mut out = reqwest::header::HeaderMap::new();
    for (name, value) in headers {
        let lower = name.as_str();
        if HOP_HEADERS.contains(&lower) || lower == "host" || lower == "origin" || lower == "referer" || lower == "content-length" {
            continue;
        }
        if let (Ok(n), Ok(v)) = (reqwest::header::HeaderName::from_bytes(name.as_str().as_bytes()), reqwest::header::HeaderValue::from_bytes(value.as_bytes())) {
            out.append(n, v);
        }
    }
    out
}

/// Everything that is not matched by a route: a `/vrgdg` route that is not built yet answers 501, anything else is a
/// ComfyUI route and goes to ComfyUI.
pub async fn fallback(State(state): State<Arc<AppState>>, request: Request) -> Response {
    let path = request.uri().path().to_string();
    if path.starts_with("/vrgdg/") {
        eprintln!("[VRGDG-RS Builder] Not built yet: {} {path}", request.method());
        return failure(StatusCode::NOT_IMPLEMENTED, format!("{path} is not available in H3 Studio yet."));
    }
    if let Some(dir) = state.web_dir.clone().filter(|_| matches!(request.method(), &axum::http::Method::GET | &axum::http::Method::HEAD)) {
        use tower::ServiceExt;
        let mut probe = Request::builder().method(request.method().clone()).uri(request.uri().clone()).body(Body::empty()).unwrap_or_default();
        *probe.headers_mut() = request.headers().clone();
        let Ok(reply) = tower_http::services::ServeDir::new(dir).append_index_html_on_directories(true).oneshot(probe).await;
        if reply.status() != StatusCode::NOT_FOUND {
            return reply.map(Body::new);
        }
    }
    proxy_to_comfy(&state, request).await
}

async fn proxy_to_comfy(state: &AppState, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let url = format!("{}{}", state.cfg.comfy_base(), parts.uri.path_and_query().map_or("/", |p| p.as_str()));
    let bytes = match axum::body::to_bytes(body, 256 * 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(e) => return failure(StatusCode::BAD_REQUEST, format!("The request body could not be read: {e}")),
    };
    let method = reqwest::Method::from_bytes(parts.method.as_str().as_bytes()).unwrap_or(reqwest::Method::GET);
    let reply = reqwest::Client::new().request(method, &url).headers(forwarded(&parts.headers)).body(bytes).send().await;
    match reply {
        Err(e) => failure(StatusCode::BAD_GATEWAY, format!("ComfyUI is not reachable at {}: {e}", state.cfg.comfy_base())),
        Ok(response) => {
            let status = StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let mut builder = Response::builder().status(status);
            for (name, value) in response.headers() {
                let lower = name.as_str();
                if HOP_HEADERS.contains(&lower) || lower == "content-length" || lower.starts_with("access-control-") {
                    continue;
                }
                if let (Ok(n), Ok(v)) = (HeaderName::from_bytes(name.as_str().as_bytes()), header::HeaderValue::from_bytes(value.as_bytes())) {
                    builder = builder.header(n, v);
                }
            }
            builder.body(Body::from_stream(response.bytes_stream())).unwrap_or_else(|e| failure(StatusCode::BAD_GATEWAY, e.to_string()))
        }
    }
}
