//! The RefMod server routes of the Builder: sign in to the self-hosted RefMod server (the `webhost` project), share a RefMod with it, list
//! what others shared, add one to the library, report one and take your own off. Port of `minimax/refmod_server.py`; the routes and
//! replies are the same, so the RefMods Viewer works the same in the pack and here.
//!
//! The browser never talks to the server. These routes do, so the file, the picture and the sign-in token stay on this computer. The server
//! needs a sign-in (GitHub or Discord): `signin` gives the Viewer an address to open in the browser and `signin/poll` waits until the
//! person has finished. Nothing secret ships with this code, the server keeps its own secrets and gives each person a personal token.
//! The settings (address, token, the address the token came from, account name) are in `refmod_server.json` next to `config.json`, and
//! a token is only ever sent to the address it came from.

use std::collections::HashMap;
use std::path::{Path as FsPath, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use h3_core::pyval::text_or_empty;
use h3_pipeline::context::refmod_dirs_of;
use h3_pipeline::refmod_library::{find_refmod, list_refmods, read_entry, PREVIEW_SUFFIX};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio_stream::StreamExt;

use super::{body_object, into_response, Fail, Out};
use crate::app::AppState;

/// The public RefMod server. Behind HTTPS on port 443, its own port 8787 is not open to the internet.
pub const DEFAULT_SERVER_URL: &str = "https://aidigitalcreations.com";
const SETTINGS_FILE: &str = "refmod_server.json";
const MAX_UPLOAD_BYTES: u64 = 64 * 1024 * 1024;
const MAX_DOWNLOAD_BYTES: usize = 256 * 1024 * 1024;
const MAX_PREVIEW_BYTES: usize = 8 * 1024 * 1024;
const MAX_JSON_BYTES: usize = 16 * 1024 * 1024;
const LIST_LIMIT: u32 = 2000;
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
/// Settings are read, changed and written whole, one request at a time.
static SETTINGS_LOCK: Mutex<()> = Mutex::new(());
/// The sign-in the Viewer started and has not finished, for each settings file. The secret the server gave for it stays here, not in
/// the browser.
static PENDING: Mutex<Option<HashMap<PathBuf, Pending>>> = Mutex::new(None);

#[derive(Debug, Clone)]
struct Pending {
    poll: String,
    url: String,
}

fn pending_of(path: &FsPath) -> Option<Pending> {
    PENDING.lock().unwrap_or_else(|p| p.into_inner()).as_ref().and_then(|all| all.get(path).cloned())
}

fn set_pending(path: &FsPath, pending: Pending) {
    PENDING.lock().unwrap_or_else(|p| p.into_inner()).get_or_insert_with(HashMap::new).insert(path.to_path_buf(), pending);
}

fn clear_pending(path: &FsPath) {
    if let Some(all) = PENDING.lock().unwrap_or_else(|p| p.into_inner()).as_mut() {
        all.remove(path);
    }
}

#[derive(Debug, Clone, Default)]
struct Settings {
    url: String,
    token: String,
    /// The server that gave the token.
    token_url: String,
    account: String,
}

fn settings_path(state: &AppState) -> PathBuf {
    state.config_path.with_file_name(SETTINGS_FILE)
}

fn read_settings(path: &FsPath) -> Settings {
    let value: Value = std::fs::read_to_string(path).ok().and_then(|text| serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()).unwrap_or(Value::Null);
    let text = |key: &str| value.get(key).and_then(Value::as_str).unwrap_or("").to_string();
    Settings { url: text("url"), token: text("token"), token_url: text("token_url"), account: text("account") }
}

fn write_settings(path: &FsPath, settings: &Settings) -> Result<(), String> {
    let value = json!({"url": settings.url, "token": settings.token, "token_url": settings.token_url, "account": settings.account});
    h3_project::atomic_write_json(path, &value).map_err(|e| format!("Could not save the server settings: {e}"))
}

/// `http(s)://host[:port]` without a trailing slash or `/api/v1`, or `""` when the text is not a web address.
pub fn normalize_server_url(value: &str) -> String {
    let mut text = value.trim().trim_end_matches('/').to_string();
    if let Some(rest) = text.strip_suffix("/api/v1") {
        text = rest.trim_end_matches('/').to_string();
    }
    let valid = (text.starts_with("http://") || text.starts_with("https://"))
        && !text.chars().any(char::is_whitespace)
        && reqwest::Url::parse(&text).is_ok_and(|url| url.host_str().is_some());
    if valid {
        text
    } else {
        String::new()
    }
}

/// The address in the settings, else `H3_REFMOD_SERVER`, else the default.
fn server_url(settings: &Settings) -> String {
    let from_env = std::env::var("H3_REFMOD_SERVER").map(|v| normalize_server_url(&v)).unwrap_or_default();
    [normalize_server_url(&settings.url), from_env].into_iter().find(|url| !url.is_empty()).unwrap_or_else(|| DEFAULT_SERVER_URL.to_string())
}

/// A token is only good for the server that gave it.
fn signed_in(settings: &Settings) -> bool {
    !settings.token.is_empty() && settings.token_url == server_url(settings)
}

fn public_settings(settings: &Settings) -> Value {
    let signed_in = signed_in(settings);
    json!({"url": server_url(settings), "default_url": DEFAULT_SERVER_URL, "signed_in": signed_in, "account": if signed_in { settings.account.as_str() } else { "" }})
}

/// Changes the server address. An empty address goes back to the default. A different server needs a new sign-in.
fn save_settings(path: &FsPath, url: Option<&str>) -> Result<Value, String> {
    let _guard = SETTINGS_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut settings = read_settings(path);
    if let Some(url) = url {
        let text = url.trim();
        if !text.is_empty() && normalize_server_url(text).is_empty() {
            return Err("The server address must start with http:// or https://.".into());
        }
        settings.url = normalize_server_url(text);
        if !signed_in(&settings) {
            settings.token.clear();
            settings.token_url.clear();
            settings.account.clear();
        }
    }
    write_settings(path, &settings)?;
    Ok(public_settings(&settings))
}

fn forget_token(path: &FsPath) {
    let _guard = SETTINGS_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut settings = read_settings(path);
    settings.token.clear();
    settings.token_url.clear();
    settings.account.clear();
    if let Err(error) = write_settings(path, &settings) {
        eprintln!("[VRGDG-RS RefMod] {error}");
    }
}

/// A RefMod type that is safe as a folder name: lowercase letters, numbers, `_` and `-`.
pub fn clean_type(text: &str) -> String {
    let kind: String = text.trim().to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).take(40).collect();
    if kind.chars().any(|c| c.is_ascii_alphanumeric()) {
        kind
    } else {
        "generic".into()
    }
}

/// A RefMod name that is safe as a file name. The server's own cleaning is not relied on.
pub fn clean_stem(text: &str) -> String {
    let name: String = text.chars().map(|c| if matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() { '_' } else { c }).collect();
    let name: String = name.trim_matches([' ', '.']).chars().take(80).collect();
    let name = name.trim_matches([' ', '.']).to_string();
    if name.is_empty() {
        "shared".into()
    } else {
        name
    }
}

fn check_id(id: &str) -> Result<&str, Fail> {
    if id.len() == 24 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        Ok(id)
    } else {
        Err(Fail::from("That is not a RefMod id from the server."))
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// An error with the server's machine-readable `code` (`signin_required`, `banned`, `signin_failed`, `signin_expired`) the Viewer acts on.
fn fail_with(status: StatusCode, message: impl Into<String>, code: &str) -> Fail {
    let mut fail = Fail::new(status, message);
    if !code.is_empty() {
        fail.body["code"] = json!(code);
    }
    fail
}

fn code_of(fail: &Fail) -> &str {
    fail.body.get("code").and_then(Value::as_str).unwrap_or("")
}

/// The server, as this computer talks to it.
struct Remote {
    base: String,
    /// The token, when there is one for this server.
    token: Option<String>,
    settings_path: PathBuf,
    client: reqwest::Client,
}

impl Remote {
    fn open(state: &AppState) -> Result<Self, Fail> {
        let path = settings_path(state);
        let settings = read_settings(&path);
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(60))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| Fail::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let token = signed_in(&settings).then(|| settings.token.clone());
        Ok(Self { base: server_url(&settings), token, settings_path: path, client })
    }

    /// Asks the server with the sign-in token. Returns the body, or the server's own message for an error.
    async fn send(&self, request: reqwest::RequestBuilder, limit: usize) -> Result<Vec<u8>, Fail> {
        let Some(token) = &self.token else {
            return Err(fail_with(StatusCode::UNAUTHORIZED, "Sign in to use the server library.", "signin_required"));
        };
        self.ask(request.bearer_auth(token), limit, true).await
    }

    /// Asks the server without a token: its health, and the steps of signing in.
    async fn send_open(&self, request: reqwest::RequestBuilder, limit: usize) -> Result<Vec<u8>, Fail> {
        self.ask(request, limit, false).await
    }

    async fn ask(&self, request: reqwest::RequestBuilder, limit: usize, with_token: bool) -> Result<Vec<u8>, Fail> {
        let unreachable = || Fail::new(StatusCode::BAD_GATEWAY, format!("Could not reach the RefMod server at {}. Is it running, and is the address right?", self.base));
        let response = request.send().await.map_err(|_| unreachable())?;
        let status = response.status();
        let mut body: Vec<u8> = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| unreachable())?;
            if body.len() + chunk.len() > limit {
                return Err(Fail::new(StatusCode::BAD_GATEWAY, "The server sent a file that is too large."));
            }
            body.extend_from_slice(&chunk);
        }
        if status.is_redirection() {
            return Err(Fail::new(StatusCode::BAD_GATEWAY, format!("The RefMod server at {} redirects somewhere else. Use its final address in the server settings.", self.base)));
        }
        if status.is_client_error() || status.is_server_error() {
            let answer = serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null);
            let message = answer.get("error").and_then(Value::as_str).filter(|m| !m.is_empty()).map(str::to_string);
            let code = answer.get("code").and_then(Value::as_str).unwrap_or("").to_string();
            if with_token && status == StatusCode::UNAUTHORIZED && code == "signin_required" {
                forget_token(&self.settings_path); // the server no longer knows this token
            }
            let shown = if status.is_server_error() { StatusCode::BAD_GATEWAY } else { StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY) };
            return Err(fail_with(shown, message.unwrap_or_else(|| format!("The RefMod server answered {}.", status.as_u16())), &code));
        }
        Ok(body)
    }

    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.client.get(format!("{}{path}", self.base))
    }

    fn post(&self, path: &str) -> reqwest::RequestBuilder {
        self.client.post(format!("{}{path}", self.base))
    }

    fn reply(&self, body: &[u8]) -> Result<Value, Fail> {
        let value: Value = serde_json::from_slice(body).map_err(|_| Fail::new(StatusCode::BAD_GATEWAY, "That address did not answer like a RefMod server."))?;
        if value.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err(Fail::new(StatusCode::BAD_GATEWAY, value.get("error").and_then(Value::as_str).unwrap_or("That address did not answer like a RefMod server.").to_string()));
        }
        Ok(value)
    }

    async fn json(&self, request: reqwest::RequestBuilder) -> Result<Value, Fail> {
        let body = self.send(request, MAX_JSON_BYTES).await?;
        self.reply(&body)
    }

    async fn json_open(&self, request: reqwest::RequestBuilder) -> Result<Value, Fail> {
        let body = self.send_open(request, MAX_JSON_BYTES).await?;
        self.reply(&body)
    }

    async fn preview(&self, id: &str) -> Result<Vec<u8>, Fail> {
        let body = self.send(self.get(&format!("/api/v1/refmods/{}/preview", check_id(id)?)), MAX_PREVIEW_BYTES).await?;
        if body.starts_with(PNG_SIGNATURE) {
            Ok(body)
        } else {
            Err(Fail::new(StatusCode::BAD_GATEWAY, "The server sent a preview that is not a PNG picture."))
        }
    }
}

async fn settings_get(State(state): State<Arc<AppState>>) -> Response {
    let settings = read_settings(&settings_path(&state));
    into_response(Ok(Out::Fields(public_settings(&settings))))
}

async fn settings_post(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let payload = match body_object(&body) {
        Ok(payload) => payload,
        Err(fail) => return into_response(Err(fail)),
    };
    let url = payload.get("url").filter(|v| !v.is_null()).map(|v| text_or_empty(Some(v)));
    let path = settings_path(&state);
    let result = tokio::task::spawn_blocking(move || save_settings(&path, url.as_deref())).await.unwrap_or_else(|e| Err(e.to_string()));
    into_response(result.map(Out::Fields).map_err(Fail::from))
}

/// The ways the server offers to sign in (`github`, `discord`), asked without a token.
async fn signin_options(State(state): State<Arc<AppState>>) -> Response {
    let result: Result<Value, Fail> = async {
        let remote = Remote::open(&state)?;
        let reply = remote.json_open(remote.get("/api/v1/health")).await?;
        let providers: Vec<Value> = reply.get("sign_in").and_then(Value::as_array).map(|list| list.iter().filter_map(Value::as_str).map(|name| json!(name)).collect()).unwrap_or_default();
        Ok(json!({"url": remote.base, "providers": providers}))
    }
    .await;
    into_response(result.map(Out::Fields))
}

/// Begins signing in. Returns the address to open in the person's browser.
async fn signin_start(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let payload = match body_object(&body) {
        Ok(payload) => payload,
        Err(fail) => return into_response(Err(fail)),
    };
    let result: Result<Value, Fail> = async {
        let provider = text_or_empty(payload.get("provider"));
        let remote = Remote::open(&state)?;
        let reply = remote.json_open(remote.post("/api/v1/auth/start").json(&json!({"provider": provider}))).await?;
        let (url, poll) = (text_or_empty(reply.get("url")), text_or_empty(reply.get("poll")));
        if url.is_empty() || poll.is_empty() {
            return Err(Fail::new(StatusCode::BAD_GATEWAY, "That address did not answer like a RefMod server."));
        }
        set_pending(&remote.settings_path, Pending { poll, url: remote.base.clone() });
        let number = |key: &str, default: i64| reply.get(key).and_then(Value::as_i64).filter(|n| *n > 0).unwrap_or(default);
        Ok(json!({"url": url, "interval": number("interval", 2), "expires_in": number("expires_in", 600)}))
    }
    .await;
    into_response(result.map(Out::Fields))
}

/// Asks whether the person has finished signing in. Saves the token once they have.
async fn signin_poll(State(state): State<Arc<AppState>>) -> Response {
    let result: Result<Value, Fail> = async {
        let path = settings_path(&state);
        let pending = pending_of(&path).ok_or_else(|| Fail::from("Start the sign-in first."))?;
        let remote = Remote::open(&state)?;
        if pending.url != remote.base {
            clear_pending(&path);
            return Err(Fail::from("The server address changed. Start the sign-in again."));
        }
        let reply = match remote.json_open(remote.post("/api/v1/auth/poll").json(&json!({"poll": pending.poll}))).await {
            Ok(reply) => reply,
            Err(fail) => {
                if matches!(code_of(&fail), "signin_failed" | "signin_expired") {
                    clear_pending(&path);
                }
                return Err(fail);
            }
        };
        if reply.get("status").and_then(Value::as_str) != Some("signed_in") {
            return Ok(json!({"status": "waiting"}));
        }
        clear_pending(&path);
        let (token, name) = (text_or_empty(reply.get("token")), text_or_empty(reply.get("name")));
        let account = name.clone();
        let saved = tokio::task::spawn_blocking(move || {
            let _guard = SETTINGS_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            let mut settings = read_settings(&path);
            settings.token = token;
            settings.token_url = pending.url;
            settings.account = account;
            write_settings(&path, &settings)?;
            Ok::<_, String>(public_settings(&settings))
        })
        .await
        .unwrap_or_else(|e| Err(e.to_string()))
        .map_err(Fail::from)?;
        eprintln!("[VRGDG-RS RefMod] Signed in to {} as {name}", remote.base);
        let mut answer = saved;
        answer["status"] = json!("signed_in");
        Ok(answer)
    }
    .await;
    into_response(result.map(Out::Fields))
}

/// Forgets the token here and asks the server to end it.
async fn signout(State(state): State<Arc<AppState>>) -> Response {
    let result: Result<Value, Fail> = async {
        let remote = Remote::open(&state)?;
        let _ = remote.json(remote.post("/api/v1/auth/signout")).await; // the token is forgotten here either way
        forget_token(&remote.settings_path);
        Ok(public_settings(&read_settings(&remote.settings_path)))
    }
    .await;
    into_response(result.map(Out::Fields))
}

async fn library(State(state): State<Arc<AppState>>) -> Response {
    let result: Result<Value, Fail> = async {
        let remote = Remote::open(&state)?;
        let reply = remote.json(remote.get("/api/v1/refmods").query(&[("limit", LIST_LIMIT)])).await?;
        let account = public_settings(&read_settings(&remote.settings_path))["account"].clone();
        Ok(json!({"url": remote.base, "account": account, "refmods": reply.get("refmods").cloned().unwrap_or(json!([])), "types": reply.get("types").cloned().unwrap_or(json!([]))}))
    }
    .await;
    into_response(result.map(Out::Fields))
}

async fn preview(State(state): State<Arc<AppState>>, Query(query): Query<HashMap<String, String>>) -> Response {
    let result: Result<Vec<u8>, Fail> = async {
        let id = query.get("id").cloned().unwrap_or_default();
        check_id(&id)?;
        Remote::open(&state)?.preview(&id).await
    }
    .await;
    match result {
        Ok(picture) => ([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "private, max-age=3600")], picture).into_response(),
        Err(fail) if matches!(fail.status, StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND | StatusCode::UNAUTHORIZED) => (fail.status, axum::Json(fail.body)).into_response(),
        Err(fail) => (StatusCode::BAD_GATEWAY, axum::Json(fail.body)).into_response(),
    }
}

async fn upload(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let payload = match body_object(&body) {
        Ok(payload) => payload,
        Err(fail) => return into_response(Err(fail)),
    };
    let result: Result<Value, Fail> = async {
        let name = text_or_empty(payload.get("name"));
        let dirs = refmod_dirs_of(&state.cfg).await;
        let entry = find_refmod(&dirs, &name).ok_or_else(|| Fail::from(format!("No RefMod named '{name}'.")))?;
        if entry.size > MAX_UPLOAD_BYTES {
            return Err(Fail::from(format!("'{}' is larger than the {} MB the server takes.", entry.name, MAX_UPLOAD_BYTES / (1024 * 1024))));
        }
        let preview_path = entry.path.with_extension("").with_extension(&PREVIEW_SUFFIX[1..]);
        let path = entry.path.clone();
        let (file, preview) = tokio::task::spawn_blocking(move || {
            let file = std::fs::read(&path).map_err(|e| e.to_string())?;
            let preview = std::fs::metadata(&preview_path).ok().filter(|m| m.len() as usize <= MAX_PREVIEW_BYTES).and_then(|_| std::fs::read(&preview_path).ok());
            Ok::<_, String>((file, preview))
        })
        .await
        .map_err(|e| Fail::from(e.to_string()))??;

        let remote = Remote::open(&state)?;
        let file_name = entry.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "refmod.safetensors".into());
        let part = |bytes: Vec<u8>, name: String, mime: &str| reqwest::multipart::Part::bytes(bytes).file_name(name).mime_str(mime).map_err(|e| Fail::from(e.to_string()));
        let mut form = reqwest::multipart::Form::new().part("file", part(file, file_name, "application/octet-stream")?).text("name", entry.name.clone());
        if let Some(preview) = preview {
            form = form.part("preview", part(preview, "preview.png".into(), "image/png")?);
        }
        let reply = remote.json(remote.post("/api/v1/refmods").multipart(form)).await?;
        eprintln!("[VRGDG-RS RefMod] Shared {} with {}", entry.name, remote.base);
        Ok(json!({
            "refmod": reply.get("refmod").cloned().unwrap_or(Value::Null),
            "duplicate": reply.get("duplicate").and_then(Value::as_bool).unwrap_or(false),
            "replaced": reply.get("replaced").and_then(Value::as_bool).unwrap_or(false),
            "status": reply.get("status").and_then(Value::as_str).unwrap_or("approved"),
        }))
    }
    .await;
    into_response(result.map(Out::Fields))
}

async fn delete(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let payload = match body_object(&body) {
        Ok(payload) => payload,
        Err(fail) => return into_response(Err(fail)),
    };
    let result: Result<Value, Fail> = async {
        let id = text_or_empty(payload.get("id"));
        check_id(&id)?;
        let remote = Remote::open(&state)?;
        remote.json(remote.client.delete(format!("{}/api/v1/refmods/{id}", remote.base))).await?;
        Ok(json!({"id": id}))
    }
    .await;
    into_response(result.map(Out::Fields))
}

/// Tells the server a shared RefMod should not be there. The reason is cut to one short line.
async fn report(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let payload = match body_object(&body) {
        Ok(payload) => payload,
        Err(fail) => return into_response(Err(fail)),
    };
    let result: Result<Value, Fail> = async {
        let id = text_or_empty(payload.get("id"));
        check_id(&id)?;
        let reason: String = text_or_empty(payload.get("reason")).split_whitespace().collect::<Vec<_>>().join(" ").chars().take(200).collect();
        let remote = Remote::open(&state)?;
        remote.json(remote.post(&format!("/api/v1/refmods/{id}/report")).json(&json!({"reason": reason}))).await?;
        Ok(json!({"id": id}))
    }
    .await;
    into_response(result.map(Out::Fields))
}

/// Where a RefMod added from the server is saved: the first RefMod folder that is not the MiniMaxH3Mod pack's own.
fn library_root(dirs: &[PathBuf]) -> Option<&PathBuf> {
    dirs.iter().find(|dir| !dir.parent().and_then(|p| p.file_name()).is_some_and(|n| n == "ComfyUI-MiniMaxH3Mod"))
}

/// Writes next to the target first, so a half-written file is never taken for a RefMod.
fn write_beside(path: &FsPath, content: &[u8]) -> Result<(), String> {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".part");
    let temporary = PathBuf::from(temporary);
    std::fs::write(&temporary, content).map_err(|e| e.to_string())?;
    std::fs::rename(&temporary, path).map_err(|e| {
        let _ = std::fs::remove_file(&temporary);
        e.to_string()
    })
}

/// Saves a downloaded RefMod. Nothing of yours is overwritten: a taken name gets `(2)`, `(3)` and so on.
fn save_download(dirs: &[PathBuf], kind: &str, stem: &str, digest: &str, payload: &[u8], preview: Option<&[u8]>) -> Result<Value, Fail> {
    let root = library_root(dirs).ok_or_else(|| Fail::from("ComfyUI did not say where it keeps RefMods. Is ComfyUI running?"))?;
    for existing in list_refmods(dirs, kind) {
        if std::fs::read(&existing.path).is_ok_and(|bytes| sha256_hex(&bytes) == digest) {
            return Ok(json!({"name": existing.name, "already": true, "renamed": false}));
        }
    }
    let folder = root.join(kind);
    std::fs::create_dir_all(&folder).map_err(|e| Fail::from(e.to_string()))?;
    let (mut chosen, mut number) = (stem.to_string(), 2);
    while find_refmod(dirs, &format!("{kind}/{chosen}")).is_some() || folder.join(format!("{chosen}.safetensors")).exists() {
        chosen = format!("{stem} ({number})");
        number += 1;
    }
    let target = folder.join(format!("{chosen}.safetensors"));
    let check = folder.join(format!("{chosen}.safetensors.check"));
    write_beside(&check, payload).map_err(Fail::from)?;
    if read_entry(&format!("{kind}/{chosen}"), &check, root).is_none() {
        let _ = std::fs::remove_file(&check);
        return Err(Fail::new(StatusCode::BAD_GATEWAY, "The file from the server is not a picture or video RefMod."));
    }
    std::fs::rename(&check, &target).map_err(|e| {
        let _ = std::fs::remove_file(&check);
        Fail::from(e.to_string())
    })?;
    if let Some(preview) = preview {
        write_beside(&folder.join(format!("{chosen}{PREVIEW_SUFFIX}")), preview).map_err(Fail::from)?;
    }
    eprintln!("[VRGDG-RS RefMod] Added {kind}/{chosen}");
    Ok(json!({"name": format!("{kind}/{chosen}"), "already": false, "renamed": chosen != stem}))
}

async fn download(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let payload = match body_object(&body) {
        Ok(payload) => payload,
        Err(fail) => return into_response(Err(fail)),
    };
    let result: Result<Value, Fail> = async {
        let id = text_or_empty(payload.get("id"));
        check_id(&id)?;
        let remote = Remote::open(&state)?;
        let reply = remote.json(remote.get(&format!("/api/v1/refmods/{id}"))).await?;
        let meta = reply.get("refmod").cloned().unwrap_or(Value::Null);
        let kind = clean_type(&text_or_empty(meta.get("type")));
        let stem = clean_stem(&text_or_empty(meta.get("stem")));
        let file = remote.send(remote.get(&format!("/api/v1/refmods/{id}/file")), MAX_DOWNLOAD_BYTES).await?;
        let digest = sha256_hex(&file);
        if !digest.starts_with(&id) {
            return Err(Fail::new(StatusCode::BAD_GATEWAY, "The file from the server is damaged. Try again."));
        }
        let preview = if meta.get("has_preview").and_then(Value::as_bool).unwrap_or(false) { remote.preview(&id).await.ok() } else { None };
        let dirs = refmod_dirs_of(&state.cfg).await;
        tokio::task::spawn_blocking(move || save_download(&dirs, &kind, &stem, &digest, &file, preview.as_deref()))
            .await
            .unwrap_or_else(|e| Err(Fail::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())))
    }
    .await;
    into_response(result.map(Out::Fields))
}

pub fn routes(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route("/vrgdg/refmod/server/settings", get(settings_get).post(settings_post))
        .route("/vrgdg/refmod/server/signin", get(signin_options).post(signin_start))
        .route("/vrgdg/refmod/server/signin/poll", post(signin_poll))
        .route("/vrgdg/refmod/server/signout", post(signout))
        .route("/vrgdg/refmod/server/library", get(library))
        .route("/vrgdg/refmod/server/preview", get(preview))
        .route("/vrgdg/refmod/server/upload", post(upload))
        .route("/vrgdg/refmod/server/download", post(download))
        .route("/vrgdg/refmod/server/delete", post(delete))
        .route("/vrgdg/refmod/server/report", post(report))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_cleaned_and_bad_ones_refused() {
        assert_eq!(normalize_server_url(" https://aidigitalcreations.com/ "), "https://aidigitalcreations.com");
        assert_eq!(normalize_server_url("http://127.0.0.1:8787/api/v1/"), "http://127.0.0.1:8787");
        for bad in ["", "ftp://x.com", "javascript:alert(1)", "not a url", "file:///etc/passwd", "http://"] {
            assert_eq!(normalize_server_url(bad), "", "{bad}");
        }
    }

    #[test]
    fn hostile_names_stay_names() {
        assert_eq!(clean_type("../../Evil Type"), "______evil_type");
        assert_eq!(clean_type(".."), "generic");
        assert_eq!(clean_stem("..\\..\\x/y:z"), "_.._x_y_z");
        assert_eq!(clean_stem(".."), "shared");
        assert!(!clean_stem("a/b").contains('/'));
    }

    #[test]
    fn ids_must_be_ids() {
        assert!(check_id(&"a1".repeat(12)).is_ok());
        for bad in ["", "../x", &"B".repeat(24), &"a".repeat(23)] {
            assert!(check_id(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_token_is_only_good_for_the_server_that_gave_it() {
        let own = Settings { url: "http://one.test:1".into(), token: "t".into(), token_url: "http://one.test:1".into(), account: "Alex".into() };
        assert!(signed_in(&own));
        assert_eq!(public_settings(&own)["account"], "Alex");
        let moved = Settings { url: "http://two.test:2".into(), ..own.clone() };
        assert!(!signed_in(&moved));
        assert_eq!(public_settings(&moved)["account"], "");
        assert!(!signed_in(&Settings { token: String::new(), ..own }));
    }
}
