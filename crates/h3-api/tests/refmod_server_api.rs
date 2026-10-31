//! H3 Studio's side of the RefMod server: settings, signing in, listing, sharing, adding to the library, reporting and removing, against a
//! stub server. The stub records what it is sent. The RefMod files are made here, not taken from anyone's library.

use std::path::Path;
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use h3_project::AppConfig;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n0000000000000000000000000000000000000000";
const TOKEN: &str = "token-from-the-server-0123456789";

fn refmod_bytes(kind: &str, concept: &str, seed: u8) -> Vec<u8> {
    let (frames, height, width) = (if kind == "video" { 3usize } else { 1 }, 4usize, 6usize);
    let length = 24 * frames * height * width * 2;
    let meta = json!({"name": "x", "kind": kind, "latent_h": height, "latent_w": width, "latent_t": frames, "concept_type": concept});
    let header = json!({
        "__metadata__": {"refmod_meta": meta.to_string()},
        "latent": {"dtype": "F16", "shape": [1, 24, frames, height, width], "data_offsets": [0, length]},
    })
    .to_string();
    let mut bytes = (header.len() as u64).to_le_bytes().to_vec();
    bytes.extend(header.as_bytes());
    let mut data = vec![0u8; length];
    data[0] = seed;
    bytes.extend(data);
    bytes
}

fn id_of(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect::<String>()[..24].to_string()
}

/// What the stub RefMod server holds and was asked.
#[derive(Default)]
struct Stub {
    files: Vec<(String, Value, Vec<u8>)>,
    /// Method, path, the `Authorization` header and the size of the body of each request.
    seen: Vec<(String, String, Option<String>, usize)>,
    uploads: Vec<Vec<(String, Vec<u8>)>>,
    reports: Vec<Value>,
    preview_status: u16,
    /// How many times the person has not finished signing in yet.
    waiting_polls: u32,
    /// Answers every signed-in request with this status and code, like a ban or a token the server forgot.
    forced_failure: Option<(u16, &'static str)>,
    /// What the poll answers instead of a token, like a sign-in the server refused.
    poll_failure: Option<(u16, &'static str)>,
    upload_status: String,
}

type Shared = Arc<Mutex<Stub>>;

fn bearer(headers: &HeaderMap) -> Option<String> {
    headers.get("authorization").and_then(|v| v.to_str().ok()).map(str::to_string)
}

fn failure(status: u16, error: &str, code: &str) -> Response {
    (StatusCode::from_u16(status).unwrap(), axum::Json(json!({"ok": false, "error": error, "code": code}))).into_response()
}

/// Notes the request. For a call that needs a sign-in it answers with the refusal the real server would give.
fn record(stub: &mut Stub, method: &str, path: &str, headers: &HeaderMap, length: usize, needs_token: bool) -> Option<Response> {
    let auth = bearer(headers);
    stub.seen.push((method.into(), path.into(), auth.clone(), length));
    if !needs_token {
        return None;
    }
    if let Some((status, code)) = stub.forced_failure {
        return Some(failure(status, "The server refused this.", code));
    }
    (auth.as_deref() != Some(&format!("Bearer {TOKEN}"))).then(|| failure(401, "Sign in to use the server library.", "signin_required"))
}

async fn stub_server(stub: Shared) -> String {
    async fn health(State(stub): State<Shared>, headers: HeaderMap) -> Response {
        record(&mut stub.lock().unwrap(), "GET", "/api/v1/health", &headers, 0, false);
        axum::Json(json!({"ok": true, "name": "h3-refmod-server", "sign_in": ["github", "discord"]})).into_response()
    }
    async fn start(State(stub): State<Shared>, headers: HeaderMap, body: Bytes) -> Response {
        record(&mut stub.lock().unwrap(), "POST", "/api/v1/auth/start", &headers, body.len(), false);
        let provider = serde_json::from_slice::<Value>(&body).ok().and_then(|v| v["provider"].as_str().map(str::to_string)).unwrap_or_default();
        if provider != "github" && provider != "discord" {
            return failure(400, "This server does not offer that sign-in.", "");
        }
        axum::Json(json!({"ok": true, "url": "https://github.com/login/oauth/authorize?state=s", "poll": "poll-secret", "interval": 2, "expires_in": 600})).into_response()
    }
    async fn poll(State(stub): State<Shared>, headers: HeaderMap, body: Bytes) -> Response {
        let mut stub = stub.lock().unwrap();
        record(&mut stub, "POST", "/api/v1/auth/poll", &headers, body.len(), false);
        let secret = serde_json::from_slice::<Value>(&body).ok().and_then(|v| v["poll"].as_str().map(str::to_string)).unwrap_or_default();
        if secret != "poll-secret" {
            return failure(404, "This sign-in ran out of time. Start it again.", "signin_expired");
        }
        if let Some((status, code)) = stub.poll_failure {
            return failure(status, "This account has been banned from the server.", code);
        }
        if stub.waiting_polls > 0 {
            stub.waiting_polls -= 1;
            return axum::Json(json!({"ok": true, "status": "waiting"})).into_response();
        }
        axum::Json(json!({"ok": true, "status": "signed_in", "token": TOKEN, "name": "alice-gh"})).into_response()
    }
    async fn signout(State(stub): State<Shared>, headers: HeaderMap) -> Response {
        if let Some(refusal) = record(&mut stub.lock().unwrap(), "POST", "/api/v1/auth/signout", &headers, 0, true) {
            return refusal;
        }
        axum::Json(json!({"ok": true})).into_response()
    }
    async fn list(State(stub): State<Shared>, headers: HeaderMap) -> Response {
        let mut stub = stub.lock().unwrap();
        if let Some(refusal) = record(&mut stub, "GET", "/api/v1/refmods", &headers, 0, true) {
            return refusal;
        }
        let refmods: Vec<Value> = stub.files.iter().map(|(_, meta, _)| meta.clone()).collect();
        axum::Json(json!({"ok": true, "total": refmods.len(), "types": [{"type": "identity", "count": refmods.len()}], "refmods": refmods})).into_response()
    }
    async fn show(State(stub): State<Shared>, headers: HeaderMap, UrlPath(id): UrlPath<String>) -> Response {
        let mut stub = stub.lock().unwrap();
        if let Some(refusal) = record(&mut stub, "GET", &format!("/api/v1/refmods/{id}"), &headers, 0, true) {
            return refusal;
        }
        match stub.files.iter().find(|(file_id, _, _)| *file_id == id) {
            Some((_, meta, _)) => axum::Json(json!({"ok": true, "refmod": meta})).into_response(),
            None => failure(404, "No RefMod with this id.", ""),
        }
    }
    async fn file(State(stub): State<Shared>, headers: HeaderMap, UrlPath(id): UrlPath<String>) -> Response {
        let mut stub = stub.lock().unwrap();
        if let Some(refusal) = record(&mut stub, "GET", &format!("/api/v1/refmods/{id}/file"), &headers, 0, true) {
            return refusal;
        }
        match stub.files.iter().find(|(file_id, _, _)| *file_id == id) {
            Some((_, _, bytes)) => bytes.clone().into_response(),
            None => failure(404, "No RefMod with this id.", ""),
        }
    }
    async fn preview(State(stub): State<Shared>, headers: HeaderMap, UrlPath(id): UrlPath<String>) -> Response {
        let mut stub = stub.lock().unwrap();
        if let Some(refusal) = record(&mut stub, "GET", &format!("/api/v1/refmods/{id}/preview"), &headers, 0, true) {
            return refusal;
        }
        if stub.preview_status == 404 {
            return failure(404, "No preview.", "");
        }
        PNG.to_vec().into_response()
    }
    async fn upload(State(stub): State<Shared>, headers: HeaderMap, body: Bytes) -> Response {
        let mut stub = stub.lock().unwrap();
        if let Some(refusal) = record(&mut stub, "POST", "/api/v1/refmods", &headers, body.len(), true) {
            return refusal;
        }
        let boundary = headers.get("content-type").and_then(|v| v.to_str().ok()).and_then(|v| v.split("boundary=").nth(1)).unwrap_or("").to_string();
        let text = body.to_vec();
        let marker = format!("--{boundary}");
        let mut parts: Vec<(String, Vec<u8>)> = Vec::new();
        let mut rest = &text[..];
        let find = |hay: &[u8], needle: &[u8]| hay.windows(needle.len()).position(|w| w == needle);
        while let Some(start) = find(rest, marker.as_bytes()) {
            rest = &rest[start + marker.len()..];
            let Some(head_end) = find(rest, b"\r\n\r\n") else { break };
            let head = String::from_utf8_lossy(&rest[..head_end]).to_string();
            let body_start = head_end + 4;
            let Some(end) = find(&rest[body_start..], format!("\r\n{marker}").as_bytes()) else { break };
            let name = head.split("name=\"").nth(1).and_then(|s| s.split('"').next()).unwrap_or("").to_string();
            parts.push((name, rest[body_start..body_start + end].to_vec()));
        }
        stub.uploads.push(parts);
        let status = if stub.upload_status.is_empty() { "approved".to_string() } else { stub.upload_status.clone() };
        (StatusCode::CREATED, axum::Json(json!({"ok": true, "duplicate": false, "replaced": false, "status": status, "refmod": {"id": "d".repeat(24), "mine": true}}))).into_response()
    }
    async fn remove(State(stub): State<Shared>, headers: HeaderMap, UrlPath(id): UrlPath<String>) -> Response {
        let mut stub = stub.lock().unwrap();
        if let Some(refusal) = record(&mut stub, "DELETE", &format!("/api/v1/refmods/{id}"), &headers, 0, true) {
            return refusal;
        }
        if id.starts_with('f') {
            return failure(403, "Only the person who shared this RefMod can delete it.", "");
        }
        axum::Json(json!({"ok": true, "id": id})).into_response()
    }
    async fn report(State(stub): State<Shared>, headers: HeaderMap, UrlPath(id): UrlPath<String>, body: Bytes) -> Response {
        let mut stub = stub.lock().unwrap();
        if let Some(refusal) = record(&mut stub, "POST", &format!("/api/v1/refmods/{id}/report"), &headers, body.len(), true) {
            return refusal;
        }
        stub.reports.push(serde_json::from_slice(&body).unwrap_or(Value::Null));
        axum::Json(json!({"ok": true})).into_response()
    }
    let app = axum::Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/auth/start", post(start))
        .route("/api/v1/auth/poll", post(poll))
        .route("/api/v1/auth/signout", post(signout))
        .route("/api/v1/refmods", get(list).post(upload))
        .route("/api/v1/refmods/{id}", get(show))
        .route("/api/v1/refmods/{id}", delete(remove))
        .route("/api/v1/refmods/{id}/file", get(file))
        .route("/api/v1/refmods/{id}/preview", get(preview))
        .route("/api/v1/refmods/{id}/report", post(report))
        .with_state(stub);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

/// A ComfyUI that only tells where its folders are.
async fn fake_comfy(refmods: &Path) -> String {
    std::fs::create_dir_all(refmods).unwrap();
    let folders = json!({"refmods": [refmods.display().to_string()]});
    let app = axum::Router::new().route("/internal/folder_paths", get(move || async move { axum::Json(folders) }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

struct Studio {
    base: String,
    http: reqwest::Client,
    refmods: std::path::PathBuf,
    config_dir: std::path::PathBuf,
    stub: Shared,
    server: String,
    _dir: tempfile::TempDir,
}

async fn start_with(signed_in: bool) -> Studio {
    // An address set in this computer's environment must not change what the tests expect.
    std::env::remove_var("H3_REFMOD_SERVER");
    let dir = tempfile::tempdir().unwrap();
    let refmods = dir.path().join("models").join("refmods");
    let comfy = fake_comfy(&refmods).await;
    let stub: Shared = Arc::new(Mutex::new(Stub::default()));
    let server = stub_server(stub.clone()).await;
    let cfg = AppConfig { comfy_url: comfy, project_root: dir.path().join("projects"), ..AppConfig::default() };
    let state = h3_api::build_state(cfg, &dir.path().join("jobs.db")).unwrap();
    // build_state keeps the config next to the jobs file in tests, so the settings file lands in `dir`.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, h3_api::router(state)).await.unwrap() });
    let studio = Studio { base: format!("http://{addr}"), http: reqwest::Client::new(), refmods, config_dir: dir.path().to_path_buf(), stub, server: server.clone(), _dir: dir };
    let saved = studio.post("/vrgdg/refmod/server/settings", json!({"url": server})).await;
    assert_eq!(saved.0, 200, "{:?}", saved.1);
    if signed_in {
        studio.sign_in().await;
    }
    studio.stub.lock().unwrap().seen.clear();
    studio
}

/// Signed in, the way the Viewer does it.
async fn start() -> Studio {
    start_with(true).await
}

async fn start_signed_out() -> Studio {
    start_with(false).await
}

impl Studio {
    async fn get(&self, path: &str) -> (u16, Value) {
        let response = self.http.get(format!("{}{path}", self.base)).send().await.unwrap();
        (response.status().as_u16(), response.json().await.unwrap_or(Value::Null))
    }

    async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        let response = self.http.post(format!("{}{path}", self.base)).json(&body).send().await.unwrap();
        (response.status().as_u16(), response.json().await.unwrap_or(Value::Null))
    }

    async fn sign_in(&self) {
        let (status, started) = self.post("/vrgdg/refmod/server/signin", json!({"provider": "github"})).await;
        assert_eq!(status, 200, "{started}");
        let (status, done) = self.post("/vrgdg/refmod/server/signin/poll", json!({})).await;
        assert_eq!((status, done["status"].as_str()), (200, Some("signed_in")), "{done}");
    }

    fn share(&self, bytes: &[u8], kind: &str, stem: &str, description: &str) -> String {
        let id = id_of(bytes);
        let meta = json!({"id": id, "type": kind, "stem": stem, "name": format!("{kind}/{stem}"), "has_preview": true, "uploader": "Robin", "description": description, "mine": false});
        self.stub.lock().unwrap().files.push((id.clone(), meta, bytes.to_vec()));
        id
    }

    fn saved(&self, name: &str) -> std::path::PathBuf {
        self.refmods.join(format!("{name}.safetensors"))
    }

    fn settings_file(&self) -> Value {
        serde_json::from_str(&std::fs::read_to_string(self.config_dir.join("refmod_server.json")).unwrap()).unwrap()
    }

    fn requests(&self) -> usize {
        self.stub.lock().unwrap().seen.len()
    }
}

#[tokio::test]
async fn settings_round_trip_and_the_token_is_never_shown() {
    let studio = start_signed_out().await;
    let (_, settings) = studio.get("/vrgdg/refmod/server/settings").await;
    assert_eq!(settings["url"], studio.server.as_str());
    assert_eq!(settings["default_url"], "https://aidigitalcreations.com");
    assert_eq!((settings["signed_in"].as_bool(), settings["account"].as_str()), (Some(false), Some("")));
    assert!(settings.get("uploader").is_none());

    let (status, body) = studio.post("/vrgdg/refmod/server/settings", json!({"url": "ftp://x.com"})).await;
    assert_eq!(status, 400, "{body}");
    let (_, body) = studio.post("/vrgdg/refmod/server/settings", json!({})).await;
    assert_eq!(body["url"], studio.server.as_str(), "an unchanged address stays");
    let (_, body) = studio.post("/vrgdg/refmod/server/settings", json!({"url": ""})).await;
    assert_eq!(body["url"], "https://aidigitalcreations.com", "an empty address means the default");
}

#[tokio::test]
async fn signing_in_saves_a_token_that_is_never_shown() {
    let studio = start_signed_out().await;
    let (status, options) = studio.get("/vrgdg/refmod/server/signin").await;
    assert_eq!((status, options["providers"].clone()), (200, json!(["github", "discord"])), "{options}");
    assert_eq!(studio.stub.lock().unwrap().seen.last().unwrap().2, None, "the ways to sign in are asked without a token");

    studio.stub.lock().unwrap().waiting_polls = 1;
    let (status, started) = studio.post("/vrgdg/refmod/server/signin", json!({"provider": "github"})).await;
    assert_eq!(status, 200, "{started}");
    assert_eq!(started["url"], "https://github.com/login/oauth/authorize?state=s");
    assert!(!started.to_string().contains("poll-secret"), "the secret that collects the token stays on this computer: {started}");

    let (_, waiting) = studio.post("/vrgdg/refmod/server/signin/poll", json!({})).await;
    assert_eq!(waiting["status"], "waiting");
    assert_eq!(studio.get("/vrgdg/refmod/server/settings").await.1["signed_in"], false);
    let (status, done) = studio.post("/vrgdg/refmod/server/signin/poll", json!({})).await;
    assert_eq!((status, done["status"].as_str(), done["signed_in"].as_bool(), done["account"].as_str()), (200, Some("signed_in"), Some(true), Some("alice-gh")), "{done}");
    assert!(!done.to_string().contains(TOKEN), "{done}");
    let (_, settings) = studio.get("/vrgdg/refmod/server/settings").await;
    assert_eq!((settings["signed_in"].as_bool(), settings["account"].as_str()), (Some(true), Some("alice-gh")));
    assert!(!settings.to_string().contains(TOKEN));
    let saved = studio.settings_file();
    assert_eq!((saved["token"].as_str(), saved["token_url"].as_str()), (Some(TOKEN), Some(studio.server.as_str())), "the token is kept in the settings file");

    let (status, _) = studio.post("/vrgdg/refmod/server/signin/poll", json!({})).await;
    assert_eq!(status, 400, "finished: nothing is pending any more");
}

#[tokio::test]
async fn polling_before_starting_or_after_a_failure_is_refused() {
    let studio = start_signed_out().await;
    let before = studio.requests();
    assert_eq!(studio.post("/vrgdg/refmod/server/signin/poll", json!({})).await.0, 400);
    assert_eq!(studio.requests(), before, "nothing was sent");

    studio.post("/vrgdg/refmod/server/signin", json!({"provider": "github"})).await;
    studio.stub.lock().unwrap().poll_failure = Some((401, "signin_failed"));
    let (status, body) = studio.post("/vrgdg/refmod/server/signin/poll", json!({})).await;
    assert_eq!((status, body["code"].as_str()), (401, Some("signin_failed")), "{body}");
    assert!(body["error"].as_str().unwrap().contains("banned"), "{body}");
    assert_eq!(studio.post("/vrgdg/refmod/server/signin/poll", json!({})).await.0, 400, "a failed sign-in ends");
    assert_eq!(studio.get("/vrgdg/refmod/server/settings").await.1["signed_in"], false);

    // A provider the server does not offer is the server's refusal.
    let (status, body) = studio.post("/vrgdg/refmod/server/signin", json!({"provider": "myspace"})).await;
    assert_eq!(status, 400, "{body}");
}

#[tokio::test]
async fn changing_the_address_midway_cancels_the_sign_in() {
    let studio = start_signed_out().await;
    studio.post("/vrgdg/refmod/server/signin", json!({"provider": "github"})).await;
    studio.post("/vrgdg/refmod/server/settings", json!({"url": "http://127.0.0.1:9"})).await;
    let before = studio.requests();
    let (status, body) = studio.post("/vrgdg/refmod/server/signin/poll", json!({})).await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(studio.requests(), before, "the secret is not sent to a different server");
}

#[tokio::test]
async fn a_token_is_only_sent_to_the_server_that_gave_it() {
    let studio = start().await;
    assert_eq!(studio.get("/vrgdg/refmod/server/settings").await.1["signed_in"], true);
    studio.post("/vrgdg/refmod/server/settings", json!({"url": "http://127.0.0.1:9"})).await;
    let (_, other) = studio.get("/vrgdg/refmod/server/settings").await;
    assert_eq!((other["signed_in"].as_bool(), other["account"].as_str()), (Some(false), Some("")));
    studio.post("/vrgdg/refmod/server/settings", json!({"url": studio.server})).await;
    assert_eq!(studio.get("/vrgdg/refmod/server/settings").await.1["signed_in"], false, "going back needs a new sign-in");
    assert_eq!(studio.settings_file()["token"], "");
    let before = studio.requests();
    let (status, body) = studio.get("/vrgdg/refmod/server/library").await;
    assert_eq!((status, body["code"].as_str()), (401, Some("signin_required")), "{body}");
    assert_eq!(studio.requests(), before, "nothing is sent without a sign-in");
}

#[tokio::test]
async fn signing_out_ends_the_token_on_the_server_and_here() {
    let studio = start().await;
    let (status, body) = studio.post("/vrgdg/refmod/server/signout", json!({})).await;
    assert_eq!((status, body["signed_in"].as_bool()), (200, Some(false)), "{body}");
    {
        let stub = studio.stub.lock().unwrap();
        let (method, path, auth, _) = stub.seen.last().unwrap();
        assert_eq!((method.as_str(), path.as_str(), auth.as_deref()), ("POST", "/api/v1/auth/signout", Some(format!("Bearer {TOKEN}").as_str())));
    }
    assert_eq!(studio.settings_file()["token"], "");
}

#[tokio::test]
async fn a_token_the_server_no_longer_knows_is_forgotten_but_a_ban_is_not_a_lost_token() {
    let studio = start().await;
    studio.stub.lock().unwrap().forced_failure = Some((403, "banned"));
    let (status, body) = studio.get("/vrgdg/refmod/server/library").await;
    assert_eq!((status, body["code"].as_str()), (403, Some("banned")), "{body}");
    assert_eq!(studio.get("/vrgdg/refmod/server/settings").await.1["signed_in"], true);

    studio.stub.lock().unwrap().forced_failure = Some((401, "signin_required"));
    let (status, body) = studio.get("/vrgdg/refmod/server/library").await;
    assert_eq!((status, body["code"].as_str()), (401, Some("signin_required")), "{body}");
    assert_eq!(studio.get("/vrgdg/refmod/server/settings").await.1["signed_in"], false);
}

#[tokio::test]
async fn the_library_comes_from_the_server_and_carries_the_token() {
    let studio = start().await;
    studio.share(&refmod_bytes("video", "identity", 1), "identity", "darrel", "A tall man.");
    let (status, body) = studio.get("/vrgdg/refmod/server/library").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!((body["url"].as_str(), body["account"].as_str()), (Some(studio.server.as_str()), Some("alice-gh")));
    assert_eq!(body["refmods"][0]["stem"], "darrel");
    let stub = studio.stub.lock().unwrap();
    assert_eq!(stub.seen.last().unwrap().2.as_deref(), Some(format!("Bearer {TOKEN}").as_str()));
}

#[tokio::test]
async fn the_library_needs_a_sign_in_and_says_so_without_asking_the_server() {
    let studio = start_signed_out().await;
    let (status, body) = studio.get("/vrgdg/refmod/server/library").await;
    assert_eq!((status, body["code"].as_str()), (401, Some("signin_required")), "{body}");
    assert_eq!(studio.requests(), 0);
    let response = studio.http.get(format!("{}/vrgdg/refmod/server/preview?id={}", studio.base, "a".repeat(24))).send().await.unwrap();
    assert_eq!(response.status().as_u16(), 401);
    for (path, body) in [("/vrgdg/refmod/server/upload", json!({"name": "identity/x"})), ("/vrgdg/refmod/server/report", json!({"id": "a".repeat(24)})), ("/vrgdg/refmod/server/delete", json!({"id": "a".repeat(24)}))] {
        let status = studio.post(path, body).await.0;
        assert!(status == 400 || status == 401, "{path}: {status}");
    }
    assert_eq!(studio.requests(), 0);
}

#[tokio::test]
async fn a_server_that_is_not_running_says_where_it_looked() {
    let studio = start().await;
    // A token for the dead address, as if signed in there.
    let mut settings = studio.settings_file();
    settings["url"] = json!("http://127.0.0.1:9");
    settings["token_url"] = json!("http://127.0.0.1:9");
    std::fs::write(studio.config_dir.join("refmod_server.json"), settings.to_string()).unwrap();
    let (status, body) = studio.get("/vrgdg/refmod/server/library").await;
    assert_eq!(status, 502);
    assert!(body["error"].as_str().unwrap().contains("http://127.0.0.1:9"), "{body}");
}

#[tokio::test]
async fn a_shared_refmod_is_added_with_its_preview_and_never_overwrites() {
    let studio = start().await;
    let first = refmod_bytes("video", "identity", 1);
    let id = studio.share(&first, "identity", "darrel", "A tall man.");
    let (status, body) = studio.post("/vrgdg/refmod/server/download", json!({"id": id})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!((body["name"].as_str(), body["already"].as_bool(), body["renamed"].as_bool()), (Some("identity/darrel"), Some(false), Some(false)));
    assert_eq!(std::fs::read(studio.saved("identity/darrel")).unwrap(), first);
    assert_eq!(std::fs::read(studio.refmods.join("identity/darrel.preview.png")).unwrap(), PNG);
    assert!(studio.stub.lock().unwrap().seen.iter().all(|(_, _, auth, _)| auth.as_deref() == Some(format!("Bearer {TOKEN}").as_str())));

    // The same file again is already there.
    let (_, again) = studio.post("/vrgdg/refmod/server/download", json!({"id": id})).await;
    assert_eq!((again["name"].as_str(), again["already"].as_bool()), (Some("identity/darrel"), Some(true)));

    // Another file with the same name is saved beside it, and yours is not touched.
    let other = refmod_bytes("video", "identity", 2);
    let other_id = studio.share(&other, "identity", "darrel", "Another.");
    let (_, body) = studio.post("/vrgdg/refmod/server/download", json!({"id": other_id})).await;
    assert_eq!((body["name"].as_str(), body["renamed"].as_bool()), (Some("identity/darrel (2)"), Some(true)));
    assert_eq!(std::fs::read(studio.saved("identity/darrel")).unwrap(), first);
    assert_eq!(std::fs::read(studio.saved("identity/darrel (2)")).unwrap(), other);
}

#[tokio::test]
async fn names_from_the_server_cannot_leave_the_library() {
    let studio = start().await;
    let id = studio.share(&refmod_bytes("image", "identity", 1), "../../outside", "..\\..\\evil/../name", "");
    let (status, body) = studio.post("/vrgdg/refmod/server/download", json!({"id": id})).await;
    assert_eq!(status, 200, "{body}");
    let saved = studio.saved(body["name"].as_str().unwrap());
    assert!(saved.is_file(), "{body}");
    assert_eq!(saved.parent().unwrap().parent().unwrap(), studio.refmods.as_path());
    let outside: Vec<_> = std::fs::read_dir(studio.refmods.parent().unwrap().parent().unwrap()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert!(!outside.iter().any(|name| name.contains("evil") || name.contains("outside")), "{outside:?}");
}

#[tokio::test]
async fn a_damaged_or_wrong_file_is_refused_and_leaves_nothing_behind() {
    let studio = start().await;
    let good = refmod_bytes("image", "identity", 1);
    let id = studio.share(&good, "identity", "a", "");
    // The server returns other bytes than the id promises.
    studio.stub.lock().unwrap().files[0].2 = refmod_bytes("image", "identity", 9);
    let (status, body) = studio.post("/vrgdg/refmod/server/download", json!({"id": id})).await;
    assert_eq!(status, 502, "{body}");
    assert!(body["error"].as_str().unwrap().contains("damaged"), "{body}");

    // Bytes that match their id but are not a RefMod.
    let junk = b"this is not a safetensors file at all".to_vec();
    let junk_id = studio.share(&junk, "identity", "junk", "");
    let (status, _) = studio.post("/vrgdg/refmod/server/download", json!({"id": junk_id})).await;
    assert_eq!(status, 502);
    let files: Vec<_> = walk(&studio.refmods);
    assert!(files.is_empty(), "{files:?}");

    let (status, _) = studio.post("/vrgdg/refmod/server/download", json!({"id": "../x"})).await;
    assert_eq!(status, 400);
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(walk(&path));
        } else {
            found.push(path);
        }
    }
    found
}

#[tokio::test]
async fn a_missing_preview_does_not_stop_the_download() {
    let studio = start().await;
    let id = studio.share(&refmod_bytes("image", "background", 1), "background", "street", "");
    studio.stub.lock().unwrap().preview_status = 404;
    let (status, body) = studio.post("/vrgdg/refmod/server/download", json!({"id": id})).await;
    assert_eq!(status, 200, "{body}");
    assert!(studio.saved("background/street").is_file());
    assert!(!studio.refmods.join("background/street.preview.png").exists());
}

#[tokio::test]
async fn a_refmod_is_sent_with_its_preview_and_name_and_your_token() {
    let studio = start().await;
    let file = refmod_bytes("video", "identity", 1);
    std::fs::create_dir_all(studio.refmods.join("identity")).unwrap();
    std::fs::write(studio.saved("identity/darrel"), &file).unwrap();
    std::fs::write(studio.refmods.join("identity/darrel.preview.png"), PNG).unwrap();
    studio.stub.lock().unwrap().upload_status = "pending".into();
    let (status, body) = studio.post("/vrgdg/refmod/server/upload", json!({"name": "identity/darrel"})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["refmod"]["id"], "d".repeat(24));
    assert_eq!(body["status"], "pending");
    let stub = studio.stub.lock().unwrap();
    let parts = stub.uploads.last().unwrap();
    let part = |name: &str| parts.iter().find(|(n, _)| n == name).map(|(_, bytes)| bytes.clone());
    assert_eq!(part("file").unwrap(), file);
    assert_eq!(part("preview").unwrap(), PNG);
    assert_eq!(part("name").unwrap(), b"identity/darrel");
    assert!(part("uploader").is_none(), "the server names the uploader from the account");
    assert_eq!(stub.seen.last().unwrap().2.as_deref(), Some(format!("Bearer {TOKEN}").as_str()));
}

#[tokio::test]
async fn a_refmod_that_is_not_in_the_library_is_not_sent() {
    let studio = start().await;
    for name in ["identity/missing", "", "../secret"] {
        let (status, _) = studio.post("/vrgdg/refmod/server/upload", json!({"name": name})).await;
        assert_eq!(status, 400, "{name}");
    }
    assert!(studio.stub.lock().unwrap().uploads.is_empty());
}

#[tokio::test]
async fn removing_an_upload_sends_the_token_and_passes_a_refusal_on() {
    let studio = start().await;
    let (status, body) = studio.post("/vrgdg/refmod/server/delete", json!({"id": "c".repeat(24)})).await;
    assert_eq!(status, 200, "{body}");
    {
        let stub = studio.stub.lock().unwrap();
        let (method, path, auth, _) = stub.seen.last().unwrap();
        assert_eq!((method.as_str(), path.as_str()), ("DELETE", format!("/api/v1/refmods/{}", "c".repeat(24)).as_str()));
        assert_eq!(auth.as_deref(), Some(format!("Bearer {TOKEN}").as_str()));
    }
    let (status, body) = studio.post("/vrgdg/refmod/server/delete", json!({"id": "f".repeat(24)})).await;
    assert_eq!(status, 403);
    assert_eq!(body["error"], "Only the person who shared this RefMod can delete it.");
}

#[tokio::test]
async fn a_report_sends_a_short_clean_reason() {
    let studio = start().await;
    let long = format!("  not   a\nreal  person {}", "x".repeat(400));
    let (status, body) = studio.post("/vrgdg/refmod/server/report", json!({"id": "d".repeat(24), "reason": long})).await;
    assert_eq!(status, 200, "{body}");
    let reports = studio.stub.lock().unwrap().reports.clone();
    let reason = reports[0]["reason"].as_str().unwrap();
    assert!(reason.starts_with("not a real person x") && reason.chars().count() == 200, "{reason}");
    assert_eq!(studio.stub.lock().unwrap().seen.last().unwrap().2.as_deref(), Some(format!("Bearer {TOKEN}").as_str()));
    let (status, _) = studio.post("/vrgdg/refmod/server/report", json!({"id": "../x", "reason": "no"})).await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn previews_must_be_pngs_and_ids_must_be_ids() {
    let studio = start().await;
    let response = studio.http.get(format!("{}/vrgdg/refmod/server/preview?id={}", studio.base, "a".repeat(24))).send().await.unwrap();
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(response.headers()["content-type"], "image/png");
    assert_eq!(response.bytes().await.unwrap().as_ref(), PNG);
    let response = studio.http.get(format!("{}/vrgdg/refmod/server/preview?id=..%2F..%2Fx", studio.base)).send().await.unwrap();
    assert_eq!(response.status().as_u16(), 400);
}
