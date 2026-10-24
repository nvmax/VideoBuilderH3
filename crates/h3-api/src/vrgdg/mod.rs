//! The pack's `/vrgdg/...` routes, which the Video Builder front end (copied from the pack) calls. The request and
//! reply shapes follow the pack's handlers (`builder/routes.py`, `runner/routes.py` and the others) so the front end runs
//! unchanged. A route that is not built yet answers 501 with a message that names it, and the server logs the call.
//!
//! Anything that is not a `/vrgdg` route goes to ComfyUI: the Builder uses `/prompt`, `/history`, `/queue`, `/view` and
//! `/interrupt` of ComfyUI itself.

mod audio_mask;
mod choices;
mod files;
mod projects;
mod settings;
mod system;

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{Query, Request, State};
use axum::http::{header, HeaderMap, HeaderName, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, MethodRouter};
use axum::Router;
use h3_builder::Env;
use h3_core::pyval::Obj;
use h3_project::builder_store::StoreError;
use serde_json::{json, Value};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use crate::app::AppState;

pub fn router() -> Router<Arc<AppState>> {
    let router = Router::new().route("/vrgdg/events", get(events));
    let router = projects::routes(router);
    let router = files::routes(router);
    let router = audio_mask::routes(router);
    let router = settings::routes(router);
    let router = system::routes(router);
    choices::routes(router)
}

/// What a Builder route answers with. `Fields` is wrapped as `{"ok": true, ...}` like the pack's handlers do, `Raw` is
/// sent as it is (a few of the pack's routes answer without the envelope) and `Status` is `Fields` with another status.
pub enum Out {
    Fields(Value),
    Raw(Value),
    Status(StatusCode, Value),
}

impl From<Value> for Out {
    fn from(value: Value) -> Self {
        Out::Fields(value)
    }
}

/// A failed route: the status and the body, `{"ok": false, "error": ...}` plus any extra fields.
pub struct Fail {
    pub status: StatusCode,
    pub body: Value,
}

impl Fail {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self { status, body: json!({"ok": false, "error": message.into()}) }
    }
}

impl From<String> for Fail {
    fn from(message: String) -> Self {
        Fail::new(StatusCode::BAD_REQUEST, message)
    }
}

impl From<&str> for Fail {
    fn from(message: &str) -> Self {
        Fail::new(StatusCode::BAD_REQUEST, message)
    }
}

impl From<StoreError> for Fail {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::Invalid(message) => Fail::new(StatusCode::BAD_REQUEST, message),
            StoreError::NotFound(message) => Fail::new(StatusCode::NOT_FOUND, message),
            StoreError::Exists { name, message } => Fail { status: StatusCode::CONFLICT, body: json!({"ok": false, "error": message, "exists": true, "name": name}) },
        }
    }
}

pub type Reply = Result<Out, Fail>;

fn into_response(reply: Reply) -> Response {
    match reply {
        Ok(Out::Raw(value)) => axum::Json(value).into_response(),
        Ok(Out::Fields(value)) => (StatusCode::OK, axum::Json(envelope(value))).into_response(),
        Ok(Out::Status(status, value)) => (status, axum::Json(envelope(value))).into_response(),
        Err(fail) => (fail.status, axum::Json(fail.body)).into_response(),
    }
}

fn envelope(value: Value) -> Value {
    let mut out = serde_json::Map::new();
    out.insert("ok".into(), json!(true));
    if let Value::Object(map) = value {
        out.extend(map);
    }
    Value::Object(out)
}

/// The request body as an object. An empty body is an empty object. Anything else that is not a JSON object is an error.
fn body_object(body: &Bytes) -> Result<Obj, Fail> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(Obj::new());
    }
    match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err(Fail::from("The request body must be a JSON object.")),
        Err(e) => Err(Fail::from(format!("Invalid JSON body: {e}"))),
    }
}

/// A POST route that does blocking work with the Builder's folders and the JSON body.
pub fn post<F>(f: F) -> MethodRouter<Arc<AppState>>
where
    F: Fn(Env, Obj) -> Reply + Clone + Send + Sync + 'static,
{
    axum::routing::post(move |State(state): State<Arc<AppState>>, body: Bytes| {
        let f = f.clone();
        async move {
            let payload = match body_object(&body) {
                Ok(payload) => payload,
                Err(fail) => return into_response(Err(fail)),
            };
            let env = state.builder_env();
            let reply = tokio::task::spawn_blocking(move || f(env, payload)).await.unwrap_or_else(|e| Err(Fail::new(StatusCode::INTERNAL_SERVER_ERROR, format!("The request failed: {e}"))));
            into_response(reply)
        }
    })
}

/// A GET route that does blocking work. The query string arrives as an object of strings.
pub fn get_query<F>(f: F) -> MethodRouter<Arc<AppState>>
where
    F: Fn(Env, Obj) -> Reply + Clone + Send + Sync + 'static,
{
    get(move |State(state): State<Arc<AppState>>, Query(query): Query<HashMap<String, String>>| {
        let f = f.clone();
        async move {
            let payload: Obj = query.into_iter().map(|(k, v)| (k, Value::String(v))).collect();
            let env = state.builder_env();
            let reply = tokio::task::spawn_blocking(move || f(env, payload)).await.unwrap_or_else(|e| Err(Fail::new(StatusCode::INTERNAL_SERVER_ERROR, format!("The request failed: {e}"))));
            into_response(reply)
        }
    })
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
    // The agent API never falls through to ComfyUI: an unknown route there is a plain 404.
    if path.starts_with("/api/v1") || path.starts_with("/vrgdg/api/v1") {
        return StatusCode::NOT_FOUND.into_response();
    }
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
            // The screens are local files that change with the app. The browser checks them each time (a cheap local
            // 304) instead of showing an older copy.
            let mut reply = reply.map(Body::new);
            reply.headers_mut().insert(header::CACHE_CONTROL, header::HeaderValue::from_static("no-cache"));
            return reply;
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
