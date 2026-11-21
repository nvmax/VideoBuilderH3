//! The Models dialog's installer: allowlisted model downloads into the folders ComfyUI reads them from (`diffusion_models`, `text_encoders`,
//! `vae`, `loras`, `latent_upscale_models` and `models/LLM`). It serves the Dmad Models tab and the MiniMax H3, Image Models and LLM Models
//! tabs. Port of `core/model_downloads.py` and its three routes in `core/system_routes.py`; the routes and replies are the same, so the tabs
//! work the same in the pack and here.
//!
//! What can be installed is `data/model_catalog.json`, a copy of the pack's `core/model_catalog.json` (a test compares them when the pack is
//! next to the app). The browser names a model by id. It never sends a URL or a path, so it cannot make this fetch anything else or write
//! anywhere else. A download goes to `<name>.part` first, is checked, and only then gets its real name, so ComfyUI never lists a half-written
//! file and a failed download leaves nothing behind. A file that already exists is never replaced. The folders come from ComfyUI
//! (`/internal/folder_paths`), so a file lands where ComfyUI reads it.
//!
//! A Hugging Face token saved in App Settings or in the token box of the Models dialog (`h3_project::config::load_hf_token`, one file for both) goes
//! with every download from huggingface.co and with no other address. It is what lets the installer fetch the files Hugging Face keeps behind a
//! login. It is never logged and no reply carries it. The box saves and tests it through the two `hf_token` routes below, which are the pack's
//! (`core/hf_token.py` and its routes in `core/system_routes.py`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use futures_util::StreamExt;
use h3_comfy::ComfyClient;
use h3_project::config::{load_hf_token, save_hf_token};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;

use super::{body_object, into_response, Fail, Out};
use crate::app::AppState;
use crate::setup_routes::{checked_hf_token, hf_whoami, HF_BASE};

const MAX_SAFETENSORS_HEADER_BYTES: u64 = 100 * 1024 * 1024;
/// GGUF files start with these four bytes.
const GGUF_MAGIC: &[u8; 4] = b"GGUF";

/// One downloadable model. `folder` is a ComfyUI model folder name (`loras`, `diffusion_models`, `LLM`). `filename` is the name the file is saved
/// as, which need not be the name in the url. `category` (`minimax`, `image`, `llm`, `dmad`) says which tab of the Models dialog shows it,
/// `groups` which cards, `default` whether it is part of the recommended set, `format` how it is checked (`safetensors` or `gguf`) and `gated`
/// that Hugging Face wants a login for it, so it cannot be installed here.
#[derive(Debug, Deserialize)]
struct Model {
    id: String,
    label: String,
    category: String,
    groups: Vec<String>,
    /// The older name of the category, kept for the DMAD (`dmad`) and MOSS-Image (`moss`) LoRAs.
    #[serde(default)]
    group: Option<String>,
    folder: String,
    filename: String,
    url: String,
    page: String,
    note: String,
    format: String,
    default: bool,
    #[serde(default)]
    gated: bool,
    download_size: u64,
}

impl Model {
    fn group(&self) -> &str {
        self.group.as_deref().unwrap_or(&self.category)
    }
}

static MODELS: LazyLock<Vec<Model>> = LazyLock::new(|| serde_json::from_str(include_str!("../../data/model_catalog.json")).expect("data/model_catalog.json is the model catalog"));

#[derive(Clone)]
struct Job {
    state: &'static str,
    downloaded: u64,
    total: u64,
    error: String,
    cancel: Arc<AtomicBool>,
}

/// id -> how its download is going. In memory only, like the pack's.
static JOBS: LazyLock<Mutex<HashMap<String, Job>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn routes(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route("/vrgdg/video_builder/models/status", get(status))
        .route("/vrgdg/video_builder/models/install", post(install))
        .route("/vrgdg/video_builder/models/cancel", post(cancel))
        .route("/vrgdg/video_builder/models/hf_token", post(hf_token_save))
        .route("/vrgdg/video_builder/models/hf_token/test", post(hf_token_test))
}

fn find_model(id: &str) -> Option<&'static Model> {
    MODELS.iter().find(|model| model.id == id)
}

/// The ids that cannot be started: one that is not in the catalog, or (without a saved Hugging Face token) one Hugging Face keeps behind a login.
fn check_ids(ids: &[String], has_token: bool) -> Result<(), String> {
    let unknown: Vec<&str> = ids.iter().filter(|id| find_model(id).is_none()).map(String::as_str).collect();
    if !unknown.is_empty() {
        return Err(format!("Unknown model: {}.", unknown.join(", ")));
    }
    let gated: Vec<&str> = ids.iter().filter_map(|id| find_model(id)).filter(|model| model.gated).map(|model| model.label.as_str()).collect();
    if !has_token && !gated.is_empty() {
        return Err(format!("{}: {NEEDS_TOKEN}", gated.join(", ")));
    }
    Ok(())
}

/// What a gated file says while no Hugging Face token is saved.
const NEEDS_TOKEN: &str = "Hugging Face asks for a login before it gives this file out. Paste your Hugging Face token in the box at the top of this tab (or save it in Menu > App Settings), accept the model's licence on its page, then try again.";

/// The token for a download from `url`: only an address on huggingface.co gets it, and only when one is saved.
fn token_for<'a>(url: &str, token: &'a str) -> Option<&'a str> {
    (!token.is_empty() && url.starts_with("https://huggingface.co/")).then_some(token)
}

/// The saved Hugging Face token, or an empty string.
async fn saved_token(state: &AppState) -> String {
    let config_path = state.config_path.clone();
    tokio::task::spawn_blocking(move || load_hf_token(&config_path)).await.unwrap_or_default()
}

/// ComfyUI's folders for one kind of model (`loras`), in its order, from what `/internal/folder_paths` answered. The first one is where a download
/// goes. ComfyUI has no `LLM` folder of its own: the local LLM nodes read `models/LLM`, which is next to the other model folders.
fn dirs_of(paths: Option<&Value>, folder: &str) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = paths
        .and_then(|paths| paths.get(folder))
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(Value::as_str).filter(|p| !p.is_empty()).map(PathBuf::from).collect())
        .unwrap_or_default();
    if dirs.is_empty() && folder == "LLM" {
        if let Some(models) = h3_pipeline::context::models_dir(paths) {
            dirs.push(models.join("LLM"));
        }
    }
    dirs
}

/// File name -> path of every file in `dirs` (subfolders included). The first folder wins.
fn index_of(dirs: &[PathBuf]) -> HashMap<String, PathBuf> {
    fn walk(dir: &Path, index: &mut HashMap<String, PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, index);
            } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                index.entry(name.to_string()).or_insert_with(|| path.clone());
            }
        }
    }
    let mut index = HashMap::new();
    for dir in dirs {
        walk(dir, &mut index);
    }
    index
}

/// Where the files of the catalog are: for every model folder, ComfyUI's directories and the files in them, read once.
struct Folders {
    dirs: HashMap<String, Vec<PathBuf>>,
    index: HashMap<String, HashMap<String, PathBuf>>,
}

impl Folders {
    fn scan(paths: Option<&Value>) -> Self {
        let mut folders = Folders { dirs: HashMap::new(), index: HashMap::new() };
        for model in MODELS.iter() {
            if !folders.dirs.contains_key(&model.folder) {
                let dirs = dirs_of(paths, &model.folder);
                folders.index.insert(model.folder.clone(), index_of(&dirs));
                folders.dirs.insert(model.folder.clone(), dirs);
            }
        }
        folders
    }

    /// Where this file already is, else `None`.
    fn path_of(&self, model: &Model) -> Option<&PathBuf> {
        self.index.get(&model.folder).and_then(|index| index.get(&model.filename))
    }

    /// The folder a model of this kind is installed into: ComfyUI's first one, skipping its legacy name (`models/unet` for `diffusion_models`,
    /// `models/clip` for `text_encoders`, which ComfyUI lists first) when there is another.
    fn destination(&self, folder: &str) -> Option<&PathBuf> {
        let dirs = self.dirs.get(folder)?;
        let legacy = match folder {
            "diffusion_models" => "unet",
            "text_encoders" => "clip",
            _ => "",
        };
        dirs.iter().find(|dir| !dir.file_name().is_some_and(|name| name.eq_ignore_ascii_case(legacy))).or_else(|| dirs.first())
    }
}

/// ComfyUI's folders and what is in them. The folders are read on a blocking thread: a LoRA folder can hold thousands of files.
async fn scan_folders(state: &AppState) -> Folders {
    let paths = ComfyClient::new(state.cfg.comfy_base()).folder_paths().await;
    tokio::task::spawn_blocking(move || Folders::scan(paths.as_ref())).await.unwrap_or_else(|_| Folders { dirs: HashMap::new(), index: HashMap::new() })
}

fn job_view(job: Option<&Job>) -> Value {
    match job {
        Some(job) => json!({"state": job.state, "downloaded": job.downloaded, "total": job.total, "error": job.error}),
        None => json!({"state": "idle", "downloaded": 0, "total": 0, "error": ""}),
    }
}

/// Every model of the catalog with whether it is installed and how its download is going (`model_status` of the pack). `has_token` is whether a
/// Hugging Face token is saved: a gated file can be installed only then (`token_saved`), and says what is missing until it is.
fn status_rows(folders: &Folders, has_token: bool) -> Vec<Value> {
    MODELS
        .iter()
        .map(|model| {
            let path = folders.path_of(model);
            let size = path.and_then(|p| std::fs::metadata(p).ok()).map_or(0, |m| m.len());
            let job = JOBS.lock().ok().and_then(|jobs| jobs.get(&model.id).cloned());
            json!({
                "id": model.id,
                "label": model.label,
                "group": model.group(),
                "category": model.category,
                "groups": model.groups,
                "default": model.default,
                "gated": model.gated,
                "token_saved": has_token,
                "gated_message": if model.gated && !has_token { json!(NEEDS_TOKEN) } else { Value::Null },
                "format": model.format,
                "download_size": model.download_size,
                "folder": model.folder,
                "filename": model.filename,
                "url": model.url,
                "page": model.page,
                "note": model.note,
                "installed": path.is_some(),
                "path": path.map(|p| p.display().to_string()).unwrap_or_default(),
                "size": size,
                "destination": folders.destination(&model.folder).map(|p| p.display().to_string()).unwrap_or_default(),
                "job": job_view(job.as_ref()),
            })
        })
        .collect()
}

/// `GET /vrgdg/video_builder/models/status`.
async fn status(State(state): State<Arc<AppState>>) -> Response {
    let folders = scan_folders(&state).await;
    let has_token = !saved_token(&state).await.is_empty();
    into_response(Ok(Out::Fields(json!({"models": status_rows(&folders, has_token), "hf_token_saved": has_token}))))
}

fn ids_of(body: &Bytes) -> Result<Vec<String>, Fail> {
    let object = body_object(body)?;
    match object.get("ids") {
        None => Ok(Vec::new()),
        Some(Value::Array(items)) => Ok(items.iter().map(|item| item.as_str().map(str::to_string).unwrap_or_else(|| item.to_string())).collect()),
        Some(_) => Err(Fail::from("ids must be a list.".to_string())),
    }
}

/// `POST /vrgdg/video_builder/models/install` `{ids}`: start downloading the named models one after another.
async fn install(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let result: Result<Value, Fail> = async {
        let ids = ids_of(&body)?;
        let token = saved_token(&state).await;
        let has_token = !token.is_empty();
        check_ids(&ids, has_token).map_err(Fail::from)?;
        let folders = scan_folders(&state).await;
        let mut started: Vec<String> = Vec::new();
        for id in &ids {
            let Some(model) = find_model(id) else { continue };
            if started.contains(id) || folders.path_of(model).is_some() {
                continue;
            }
            let Ok(mut jobs) = JOBS.lock() else { continue };
            if jobs.get(id.as_str()).is_some_and(|job| job.state == "downloading") {
                continue;
            }
            jobs.insert(id.clone(), Job { state: "downloading", downloaded: 0, total: 0, error: String::new(), cancel: Arc::new(AtomicBool::new(false)) });
            started.push(id.clone());
        }
        // Each file goes to the first folder ComfyUI has for its kind. One that has none (ComfyUI is not running) fails at once.
        let mut queue: Vec<(String, PathBuf)> = Vec::new();
        for id in &started {
            let Some(model) = find_model(id) else { continue };
            match folders.destination(&model.folder) {
                Some(dir) => queue.push((id.clone(), dir.clone())),
                None => set_job(id, |job| {
                    job.state = "error";
                    job.error = format!("ComfyUI has no '{}' model folder. Is ComfyUI running?", model.folder);
                }),
            }
        }
        if !queue.is_empty() {
            tokio::spawn(async move {
                for (id, destination) in queue {
                    run_download(&id, &destination, &token).await;
                }
            });
            eprintln!("[VRGDG-RS Models] Started {} download(s){}.", started.len(), if has_token { " with a Hugging Face token" } else { "" });
        }
        Ok(json!({"started": started, "models": status_rows(&folders, has_token), "hf_token_saved": has_token}))
    }
    .await;
    into_response(result.map(Out::Fields))
}

/// `POST /vrgdg/video_builder/models/hf_token` `{api_key}`: save the Hugging Face token the installer downloads with, or clear it with an empty string.
/// It is the token of App Settings. No reply carries it.
async fn hf_token_save(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let result: Result<Value, Fail> = async {
        let object = body_object(&body)?;
        let Some(raw) = object.get("api_key").and_then(Value::as_str) else {
            return Err(Fail::from("api_key is required; use an empty string to clear it."));
        };
        let token = checked_hf_token(raw).map_err(Fail::from)?;
        let configured = !token.is_empty();
        let config_path = state.config_path.clone();
        tokio::task::spawn_blocking(move || save_hf_token(&config_path, &token))
            .await
            .map_err(|e| Fail::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .map_err(|e| Fail::new(StatusCode::INTERNAL_SERVER_ERROR, format!("The token could not be saved: {e}")))?;
        Ok(json!({"configured": configured}))
    }
    .await;
    into_response(result.map(Out::Fields))
}

/// `POST /vrgdg/video_builder/models/hf_token/test` `{api_key}`: ask Hugging Face whose token this is: the one in `api_key` when it is sent, else the
/// saved one.
async fn hf_token_test(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let result: Result<Value, Fail> = async {
        let object = body_object(&body)?;
        let sent = checked_hf_token(object.get("api_key").and_then(Value::as_str).unwrap_or_default()).map_err(Fail::from)?;
        let token = if sent.is_empty() { saved_token(&state).await } else { sent };
        if token.is_empty() {
            return Err(Fail::from("No Hugging Face token is saved. Paste one and try again."));
        }
        let user = hf_whoami(HF_BASE, &token).await.map_err(Fail::from)?;
        Ok(json!({"connected": true, "user": user}))
    }
    .await;
    into_response(result.map(Out::Fields))
}

/// `POST /vrgdg/video_builder/models/cancel` `{ids}`: stop running downloads.
async fn cancel(body: Bytes) -> Response {
    let result: Result<Value, Fail> = async {
        let ids = ids_of(&body)?;
        let mut cancelled: Vec<String> = Vec::new();
        if let Ok(jobs) = JOBS.lock() {
            for id in &ids {
                if let Some(job) = jobs.get(id.as_str()).filter(|job| job.state == "downloading") {
                    job.cancel.store(true, Ordering::Relaxed);
                    cancelled.push(id.clone());
                }
            }
        }
        Ok(json!({"cancelled": cancelled}))
    }
    .await;
    into_response(result.map(Out::Fields))
}

fn set_job(id: &str, change: impl FnOnce(&mut Job)) {
    if let Ok(mut jobs) = JOBS.lock() {
        if let Some(job) = jobs.get_mut(id) {
            change(job);
        }
    }
}

/// True when the file starts with a safetensors header (rejects an HTML error page saved as a model).
fn looks_like_safetensors(path: &Path) -> bool {
    use std::io::Read;
    let Ok(mut file) = std::fs::File::open(path) else { return false };
    let Ok(size) = file.metadata().map(|m| m.len()) else { return false };
    let mut length = [0u8; 8];
    if file.read_exact(&mut length).is_err() {
        return false;
    }
    let header_bytes = u64::from_le_bytes(length);
    if header_bytes == 0 || header_bytes > MAX_SAFETENSORS_HEADER_BYTES || 8 + header_bytes > size {
        return false;
    }
    let mut header = vec![0u8; header_bytes as usize];
    if file.read_exact(&mut header).is_err() {
        return false;
    }
    matches!(serde_json::from_slice::<Value>(&header), Ok(Value::Object(_)))
}

/// True when the file starts with the GGUF magic (rejects an HTML error page saved as a model).
fn looks_like_gguf(path: &Path) -> bool {
    use std::io::Read;
    let mut magic = [0u8; 4];
    std::fs::File::open(path).and_then(|mut file| file.read_exact(&mut magic)).is_ok() && &magic == GGUF_MAGIC
}

async fn run_download(id: &str, destination: &Path, token: &str) {
    let Some(model) = find_model(id) else { return };
    let cancel = JOBS.lock().ok().and_then(|jobs| jobs.get(id).map(|job| job.cancel.clone()));
    let Some(cancel) = cancel else { return };
    if cancel.load(Ordering::Relaxed) {
        set_job(id, |job| job.state = "cancelled");
        return;
    }
    let source = Source { url: &model.url, format: &model.format, token: token_for(&model.url, token), gated: model.gated };
    match download_to(id, destination, &model.filename, &source, &cancel).await {
        Ok(()) => {
            eprintln!("[VRGDG-RS Models] Installed {} into {}", model.filename, destination.display());
            set_job(id, |job| job.state = "done");
        }
        Err(message) if message.is_empty() => set_job(id, |job| {
            job.state = "cancelled";
            job.error.clear();
        }),
        Err(message) => set_job(id, |job| {
            job.state = "error";
            job.error = message;
        }),
    }
}

/// What to fetch and how: the address, the kind of file it must be (`safetensors` or `gguf`), the Hugging Face token to send with it (already limited to
/// huggingface.co, see [`token_for`]) and whether Hugging Face keeps the file behind a login.
struct Source<'a> {
    url: &'a str,
    format: &'a str,
    token: Option<&'a str>,
    gated: bool,
}

/// Download `source` into `dir/filename` and check it is the kind of file it should be. An empty error means it was cancelled. Reports progress through
/// the job for `id`.
async fn download_to(id: &str, dir: &Path, filename: &str, source: &Source<'_>, cancel: &AtomicBool) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    let target = dir.join(filename);
    let part = dir.join(format!("{filename}.part"));
    let result = fetch_to_part(id, source, &part, cancel).await;
    let finished = match result {
        Ok(()) => {
            let format = source.format;
            let valid = if format == "gguf" { looks_like_gguf(&part) } else { looks_like_safetensors(&part) };
            if !valid {
                Err(format!("The downloaded file is not a valid .{format} file. Try again, or download it from the model page."))
            } else if target.exists() {
                // Someone put the file there while this ran. Keep theirs.
                Ok(())
            } else {
                std::fs::rename(&part, &target).map_err(|e| format!("Could not save {}: {e}", target.display()))
            }
        }
        Err(message) => Err(message),
    };
    if part.exists() {
        let _ = std::fs::remove_file(&part);
    }
    finished
}

/// Hugging Face turned the token down for a gated file.
const TOKEN_REFUSED: &str = "Hugging Face refused your token for this file. Accept the model's licence on its page with the same account, and check that the token is allowed to read gated repositories.";

async fn fetch_to_part(id: &str, source: &Source<'_>, part: &Path, cancel: &AtomicBool) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .user_agent("comfyui-vrgamedevgirl-model-installer")
        .connect_timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;
    // The token goes in an Authorization header. A redirect to another host (Hugging Face's download servers) does not carry it along.
    let send = |with_token: bool| {
        let mut request = client.get(source.url);
        if let (true, Some(token)) = (with_token, source.token) {
            request = request.bearer_auth(token);
        }
        request.send()
    };
    let mut response = send(true).await.map_err(|e| format!("Could not reach the download server: {e}"))?;
    if source.token.is_some() {
        let code = response.status().as_u16();
        if source.gated && matches!(code, 401 | 403) {
            return Err(TOKEN_REFUSED.to_string());
        }
        if code == 401 {
            // A token Hugging Face does not accept fails public files too. Try again without it.
            response = send(false).await.map_err(|e| format!("Could not reach the download server: {e}"))?;
        }
    }
    if !response.status().is_success() {
        let status = response.status();
        return Err(format!("The download server answered {} {}.", status.as_u16(), status.canonical_reason().unwrap_or("")));
    }
    let total = response.content_length().unwrap_or(0);
    set_job(id, |job| job.total = total);
    let mut file = tokio::fs::File::create(part).await.map_err(|e| format!("Could not write {}: {e}", part.display()))?;
    let mut stream = response.bytes_stream();
    let mut downloaded: u64 = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("The download stopped: {e}"))?;
        file.write_all(&chunk).await.map_err(|e| format!("Could not write {}: {e}", part.display()))?;
        downloaded += chunk.len() as u64;
        set_job(id, |job| job.downloaded = downloaded);
        if cancel.load(Ordering::Relaxed) {
            return Err(String::new());
        }
    }
    file.flush().await.map_err(|e| e.to_string())?;
    drop(file);
    if total != 0 && downloaded != total {
        return Err(format!("The download stopped early ({downloaded} of {total} bytes). Try again."));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn safetensors_bytes(extra: usize) -> Vec<u8> {
        let header = br#"{"w":{"dtype":"F32","shape":[1],"data_offsets":[0,4]}}"#;
        let mut out = (header.len() as u64).to_le_bytes().to_vec();
        out.extend_from_slice(header);
        out.extend(std::iter::repeat(0u8).take(extra));
        out
    }

    /// A one-shot HTTP server on localhost that answers any request with `status` and `body`, optionally slowly.
    async fn serve(status: &'static str, body: Vec<u8>, claim_length: Option<usize>, delay_ms: u64) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut buffer = [0u8; 2048];
                let _ = socket.read(&mut buffer).await;
                let length = claim_length.unwrap_or(body.len());
                let head = format!("HTTP/1.1 {status}\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n");
                let _ = socket.write_all(head.as_bytes()).await;
                for piece in body.chunks(64 * 1024) {
                    if delay_ms > 0 {
                        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                    }
                    if socket.write_all(piece).await.is_err() {
                        return;
                    }
                }
                let _ = socket.shutdown().await;
            }
        });
        format!("http://{address}/file.safetensors")
    }

    /// A localhost server that answers each request with the next of `replies` (status line and body) and keeps the text of every request.
    async fn serve_replies(replies: Vec<(&'static str, Vec<u8>)>) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            for (status, body) in replies {
                let Ok((mut socket, _)) = listener.accept().await else { return };
                let mut buffer = vec![0u8; 4096];
                let read = socket.read(&mut buffer).await.unwrap_or(0);
                log.lock().unwrap().push(String::from_utf8_lossy(&buffer[..read]).to_lowercase());
                let head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = socket.write_all(head.as_bytes()).await;
                let _ = socket.write_all(&body).await;
                let _ = socket.shutdown().await;
            }
        });
        (format!("http://{address}/file.safetensors"), seen)
    }

    fn start(id: &str) -> Arc<AtomicBool> {
        let cancel = Arc::new(AtomicBool::new(false));
        JOBS.lock().unwrap().insert(
            id.to_string(),
            Job { state: "downloading", downloaded: 0, total: 0, error: String::new(), cancel: cancel.clone() },
        );
        cancel
    }

    #[test]
    fn the_catalog_is_sound() {
        let mut seen = std::collections::HashSet::new();
        for model in MODELS.iter() {
            let id = &model.id;
            assert!(seen.insert(id.clone()), "{id} is listed twice");
            assert!(["loras", "diffusion_models", "text_encoders", "vae", "latent_upscale_models", "LLM"].contains(&model.folder.as_str()), "{id}");
            assert!(["minimax", "image", "llm", "dmad"].contains(&model.category.as_str()), "{id}");
            assert!(!model.groups.is_empty() && !model.label.is_empty() && model.page.starts_with("https://huggingface.co/"), "{id}");
            assert!(model.url.starts_with("https://huggingface.co/") && model.url.contains("/resolve/"), "{id}");
            assert!(["safetensors", "gguf"].contains(&model.format.as_str()), "{id}");
            assert!(model.filename.ends_with(&format!(".{}", model.format)), "{id}");
            assert_eq!(model.folder == "LLM", model.format == "gguf", "{id}: GGUF files go to models/LLM and only they do");
            assert!(model.download_size > 1_000_000, "{id}");
            assert_eq!(Path::new(&model.filename).file_name().and_then(|n| n.to_str()), Some(model.filename.as_str()), "{id}: a file name, never a path");
            // Two entries saved as the same file would make one count as the other.
            assert_eq!(MODELS.iter().filter(|other| other.folder == model.folder && other.filename == model.filename).count(), 1, "{id}");
        }
        assert!(find_model("../../evil").is_none());
    }

    #[test]
    fn the_four_lora_ids_the_older_tabs_use_are_still_there_with_their_groups() {
        let legacy: Vec<(&str, &str)> = ["dmad_lora", "dmad_refiner", "moss_turbo_lora", "moss_edit_lora"].iter().map(|id| (*id, find_model(id).unwrap().group())).collect();
        assert_eq!(legacy, [("dmad_lora", "dmad"), ("dmad_refiner", "dmad"), ("moss_turbo_lora", "moss"), ("moss_edit_lora", "moss")]);
        assert_eq!(find_model("qwen_mmproj").unwrap().group(), "llm", "a newer file has its category as its group");
    }

    #[test]
    fn the_catalog_is_the_one_in_the_pack_when_the_pack_is_next_to_the_app() {
        let pack = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../core/model_catalog.json");
        let Ok(text) = std::fs::read_to_string(&pack) else {
            eprintln!("The pack is not next to the app, so its catalog is not compared.");
            return;
        };
        let theirs: Value = serde_json::from_str(&text).unwrap();
        let ours: Value = serde_json::from_str(include_str!("../../data/model_catalog.json")).unwrap();
        assert_eq!(ours, theirs, "copy core/model_catalog.json over crates/h3-api/data/model_catalog.json");
    }

    #[test]
    fn what_the_builder_selects_by_default_is_in_the_recommended_set() {
        let defaults: Value = serde_json::from_str(include_str!("../../../h3-core/data/h3_settings_defaults.json")).unwrap();
        let recommended: Vec<&str> = MODELS.iter().filter(|m| m.default && m.category == "minimax").map(|m| m.filename.as_str()).collect();
        for key in ["diffusion_model_name", "clip_name", "video_vae_name", "audio_vae_name", "two_pass_lora_name", "three_pass_lightx_lora_name", "two_pass_latent_upscaler_name"] {
            let name = defaults[key].as_str().unwrap_or_else(|| panic!("no {key} in the settings"));
            assert!(recommended.contains(&name), "{key}: {name}");
        }
    }

    #[test]
    fn a_gated_or_unknown_id_cannot_be_started() {
        assert!(check_ids(&["minimax_h3_audio_vae".into(), "qwen_mmproj".into()], false).is_ok());
        assert_eq!(check_ids(&["../../evil".into(), "dmad_lora".into()], true).unwrap_err(), "Unknown model: ../../evil.");
        // A gated file needs the saved Hugging Face token, and the message says where to put it.
        let gated = check_ids(&["flux_klein_9b_diffusion".into()], false).unwrap_err();
        assert!(gated.contains("login") && gated.contains("9B diffusion model") && gated.contains("App Settings"), "{gated}");
        assert!(check_ids(&["flux_klein_9b_diffusion".into()], true).is_ok());
    }

    #[test]
    fn the_token_goes_to_huggingface_co_and_nowhere_else() {
        assert_eq!(token_for("https://huggingface.co/black-forest-labs/x/resolve/main/y.safetensors", "hf_abc"), Some("hf_abc"));
        assert_eq!(token_for("https://huggingface.co/a/b/resolve/main/c", ""), None, "no token saved");
        assert_eq!(token_for("https://example.com/huggingface.co/x", "hf_abc"), None);
        assert_eq!(token_for("https://huggingface.co.evil.test/x", "hf_abc"), None);
        assert_eq!(token_for("http://huggingface.co/x", "hf_abc"), None, "never over plain http");
        // Every address in the catalog is on huggingface.co, so a saved token goes with all of them.
        assert!(MODELS.iter().all(|model| token_for(&model.url, "hf_abc").is_some()));
    }

    #[test]
    fn the_status_says_whether_a_gated_file_can_be_installed() {
        let without = status_rows(&Folders::scan(None), false);
        let with = status_rows(&Folders::scan(None), true);
        let row = |rows: &[Value]| rows.iter().find(|r| r["id"] == "flux_klein_9b_diffusion").unwrap().clone();
        let (blocked, ready) = (row(&without), row(&with));
        assert_eq!((blocked["gated"].clone(), blocked["token_saved"].clone()), (json!(true), json!(false)));
        assert!(blocked["gated_message"].as_str().unwrap().contains("App Settings"));
        assert_eq!((ready["gated"].clone(), ready["token_saved"].clone(), ready["gated_message"].clone()), (json!(true), json!(true), Value::Null));
        // A file that is not gated has no message either way, and the status never carries the token itself.
        assert!(without.iter().filter(|r| r["gated"] == false).all(|r| r["gated_message"].is_null()));
        assert!(!serde_json::to_string(&with).unwrap().contains("hf_"));
    }

    #[test]
    fn a_model_goes_to_diffusion_models_not_the_legacy_unet_folder_comfyui_lists_first() {
        let models = PathBuf::from("C:/ComfyUI/models");
        let paths = json!({
            "diffusion_models": [models.join("unet"), models.join("diffusion_models"), "C:/ComfyUI/output/diffusion_models"],
            "text_encoders": [models.join("clip"), models.join("text_encoders")],
            "vae": [models.join("vae")],
        });
        let folders = Folders::scan(Some(&paths));
        assert_eq!(folders.destination("diffusion_models"), Some(&models.join("diffusion_models")));
        assert_eq!(folders.destination("text_encoders"), Some(&models.join("text_encoders")));
        assert_eq!(folders.destination("vae"), Some(&models.join("vae")), "a folder with no legacy name is used as it is");
        // The legacy folder is still used when it is the only one.
        let only = Folders::scan(Some(&json!({"diffusion_models": [models.join("unet")]})));
        assert_eq!(only.destination("diffusion_models"), Some(&models.join("unet")));
        assert_eq!(only.destination("loras"), None);
    }

    #[test]
    fn the_llm_folder_is_models_llm_when_comfyui_has_no_llm_folder() {
        let reported = json!({"loras": ["C:/ComfyUI/models/loras"], "vae": ["C:/ComfyUI/models/vae"]});
        assert_eq!(dirs_of(Some(&reported), "LLM"), vec![PathBuf::from("C:/ComfyUI/models/LLM")]);
        // A folder ComfyUI reports is used as it is, and only the LLM folder has a fallback.
        let own = json!({"LLM": ["D:/llms"], "loras": ["C:/ComfyUI/models/loras"]});
        assert_eq!(dirs_of(Some(&own), "LLM"), vec![PathBuf::from("D:/llms")]);
        assert!(dirs_of(Some(&reported), "diffusion_models").is_empty());
        assert!(dirs_of(None, "LLM").is_empty(), "ComfyUI was not reachable");
    }

    #[test]
    fn the_status_says_where_every_file_is_and_goes() {
        let dir = tempfile::tempdir().unwrap();
        let models = dir.path().join("models");
        let (vae, llm, loras) = (models.join("vae"), models.join("LLM"), models.join("loras"));
        std::fs::create_dir_all(vae.join("sub")).unwrap();
        std::fs::create_dir_all(&loras).unwrap();
        // One file is already there, in a subfolder. The LLM folder does not exist yet.
        std::fs::write(vae.join("sub").join("minimax_h3_audio_vae_fp32.safetensors"), b"x").unwrap();
        let paths = json!({"vae": [vae.display().to_string()], "loras": [loras.display().to_string()]});
        let rows = status_rows(&Folders::scan(Some(&paths)), false);
        assert_eq!(rows.len(), MODELS.len());
        let row = |id: &str| rows.iter().find(|r| r["id"] == id).unwrap().clone();
        let audio = row("minimax_h3_audio_vae");
        assert_eq!((audio["installed"].clone(), audio["size"].clone(), audio["category"].clone()), (json!(true), json!(1), json!("minimax")));
        assert!(audio["path"].as_str().unwrap().ends_with("minimax_h3_audio_vae_fp32.safetensors"));
        let video = row("minimax_h3_video_vae");
        assert_eq!((video["installed"].clone(), video["destination"].clone()), (json!(false), json!(vae.display().to_string())));
        let qwen = row("qwen_mmproj");
        assert_eq!((qwen["destination"].clone(), qwen["format"].clone(), qwen["default"].clone(), qwen["groups"].clone()), (json!(llm.display().to_string()), json!("gguf"), json!(true), json!(["Qwen"])));
        // A folder ComfyUI did not report has no destination, so the install fails at once instead of guessing.
        assert_eq!(row("minimax_h3_ref2va_pruned_int8_convrot")["destination"], "");
        assert_eq!(row("flux_klein_9b_diffusion")["gated"], true);
        assert_eq!(row("dmad_lora")["group"], "dmad");
    }

    #[test]
    fn a_gguf_download_is_checked_for_the_gguf_header() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("good.gguf");
        let bad = dir.path().join("bad.gguf");
        std::fs::write(&good, b"GGUF\x03\0\0\0 and the rest").unwrap();
        std::fs::write(&bad, b"<html>login required</html>").unwrap();
        assert!(looks_like_gguf(&good));
        assert!(!looks_like_gguf(&bad));
        assert!(!looks_like_gguf(&dir.path().join("missing.gguf")));
    }

    #[tokio::test]
    async fn a_gguf_file_is_saved_in_its_folder_under_its_own_name_and_a_safetensors_page_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let mut gguf = b"GGUF\x03\0\0\0".to_vec();
        gguf.extend(std::iter::repeat(0u8).take(4096));
        let url = serve("200 OK", gguf.clone(), None, 0).await.replace("file.safetensors", "mmproj-BF16.gguf");
        let model = find_model("qwen_mmproj").unwrap();
        let cancel = start("test_gguf_name");
        download_to("test_gguf_name", dir.path(), &model.filename, &Source { url: &url, format: &model.format, token: None, gated: false }, &cancel).await.unwrap();
        assert_eq!(std::fs::read(dir.path().join("qwen-mmproj-BF16.gguf")).unwrap(), gguf);
        assert!(!dir.path().join("mmproj-BF16.gguf").exists(), "Gemma's projector has the other name");
        // A safetensors file where a GGUF was expected.
        let wrong = serve("200 OK", safetensors_bytes(256), None, 0).await;
        let other = tempfile::tempdir().unwrap();
        let cancel = start("test_gguf_wrong");
        let error = download_to("test_gguf_wrong", other.path(), "x.gguf", &Source { url: &wrong, format: "gguf", token: None, gated: false }, &cancel).await.unwrap_err();
        assert!(error.contains("not a valid .gguf"), "{error}");
        assert_eq!(std::fs::read_dir(other.path()).unwrap().count(), 0);
    }

    #[test]
    fn the_moss_turbo_lora_is_saved_as_the_name_the_settings_look_for() {
        let model = find_model("moss_turbo_lora").unwrap();
        assert_eq!(model.filename, "moss_image_sft_8_steps_dmd_lora.safetensors");
        assert!(model.url.ends_with("/lora.safetensors"));
        assert_eq!((model.group(), model.folder.as_str()), ("moss", "loras"));
    }

    #[tokio::test]
    async fn the_turbo_lora_download_is_saved_under_its_own_name_not_lora_safetensors() {
        let dir = tempfile::tempdir().unwrap();
        let data = safetensors_bytes(2048);
        let url = serve("200 OK", data.clone(), None, 0).await.replace("file.safetensors", "lora.safetensors");
        let model = find_model("moss_turbo_lora").unwrap();
        let cancel = start("test_moss_turbo_name");
        download_to("test_moss_turbo_name", dir.path(), &model.filename, &Source { url: &url, format: &model.format, token: None, gated: false }, &cancel).await.unwrap();
        assert_eq!(std::fs::read(dir.path().join("moss_image_sft_8_steps_dmd_lora.safetensors")).unwrap(), data);
        assert!(!dir.path().join("lora.safetensors").exists());
    }

    #[test]
    fn the_moss_edit_lora_is_saved_as_the_name_the_settings_look_for() {
        let model = find_model("moss_edit_lora").unwrap();
        assert_eq!(model.filename, "moss_image_edit_lora.safetensors");
        assert!(model.url.ends_with("/adapter_model.safetensors"));
        assert_eq!((model.group(), model.folder.as_str()), ("moss", "loras"));
    }

    #[tokio::test]
    async fn the_edit_lora_download_is_saved_under_its_own_name_not_the_one_in_the_url() {
        let dir = tempfile::tempdir().unwrap();
        let data = safetensors_bytes(2048);
        // The server's file is called adapter_model.safetensors, as on Hugging Face.
        let url = serve("200 OK", data.clone(), None, 0).await.replace("file.safetensors", "adapter_model.safetensors");
        let model = find_model("moss_edit_lora").unwrap();
        let cancel = start("test_moss_name");
        download_to("test_moss_name", dir.path(), &model.filename, &Source { url: &url, format: &model.format, token: None, gated: false }, &cancel).await.unwrap();
        assert_eq!(std::fs::read(dir.path().join("moss_image_edit_lora.safetensors")).unwrap(), data);
        assert!(!dir.path().join("adapter_model.safetensors").exists());
    }

    #[test]
    fn installed_files_are_found_in_subfolders_and_the_first_folder_wins() {
        let dir = tempfile::tempdir().unwrap();
        let (first, second) = (dir.path().join("one"), dir.path().join("two"));
        std::fs::create_dir_all(first.join("minimax")).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        std::fs::write(first.join("minimax").join("a.safetensors"), b"x").unwrap();
        std::fs::write(second.join("a.safetensors"), b"y").unwrap();
        let index = index_of(&[first.clone(), second.clone(), dir.path().join("missing")]);
        assert_eq!(index.get("a.safetensors"), Some(&first.join("minimax").join("a.safetensors")));
        assert!(!index.contains_key("b.safetensors"));
        assert!(index_of(&[]).is_empty());
    }

    #[tokio::test]
    async fn a_download_sends_the_token_as_a_bearer_header_and_without_one_sends_none() {
        let dir = tempfile::tempdir().unwrap();
        let data = safetensors_bytes(2048);
        let (url, seen) = serve_replies(vec![("200 OK", data.clone())]).await;
        let cancel = start("test_token_sent");
        download_to("test_token_sent", dir.path(), "a.safetensors", &Source { url: &url, format: "safetensors", token: Some("hf_secret"), gated: true }, &cancel).await.unwrap();
        assert_eq!(std::fs::read(dir.path().join("a.safetensors")).unwrap(), data);
        assert!(seen.lock().unwrap()[0].contains("authorization: bearer hf_secret"), "{:?}", seen.lock().unwrap());

        let (url, seen) = serve_replies(vec![("200 OK", data)]).await;
        let cancel = start("test_token_absent");
        download_to("test_token_absent", dir.path(), "b.safetensors", &Source { url: &url, format: "safetensors", token: None, gated: false }, &cancel).await.unwrap();
        assert!(!seen.lock().unwrap()[0].contains("authorization"), "{:?}", seen.lock().unwrap());
    }

    #[tokio::test]
    async fn a_gated_file_whose_token_is_refused_says_to_accept_the_licence() {
        let dir = tempfile::tempdir().unwrap();
        for status in ["401 Unauthorized", "403 Forbidden"] {
            let (url, seen) = serve_replies(vec![(status, b"gated".to_vec())]).await;
            let cancel = start("test_gated_refused");
            let error = download_to("test_gated_refused", dir.path(), "g.safetensors", &Source { url: &url, format: "safetensors", token: Some("hf_secret"), gated: true }, &cancel).await.unwrap_err();
            assert!(error.contains("refused your token") && error.contains("licence"), "{error}");
            assert!(!error.contains("hf_secret"), "the token is never in a message");
            assert_eq!(seen.lock().unwrap().len(), 1, "no second try for a gated file");
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        }
    }

    #[tokio::test]
    async fn a_token_hugging_face_does_not_accept_does_not_stop_a_public_file() {
        let dir = tempfile::tempdir().unwrap();
        let data = safetensors_bytes(1024);
        let (url, seen) = serve_replies(vec![("401 Unauthorized", b"bad token".to_vec()), ("200 OK", data.clone())]).await;
        let cancel = start("test_bad_token_public");
        download_to("test_bad_token_public", dir.path(), "p.safetensors", &Source { url: &url, format: "safetensors", token: Some("hf_revoked"), gated: false }, &cancel).await.unwrap();
        assert_eq!(std::fs::read(dir.path().join("p.safetensors")).unwrap(), data);
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert!(seen[0].contains("authorization: bearer hf_revoked"));
        assert!(!seen[1].contains("authorization"), "the second try is anonymous");
    }

    #[test]
    fn the_header_check_accepts_a_model_and_refuses_a_web_page() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("good.safetensors");
        let bad = dir.path().join("bad.safetensors");
        std::fs::write(&good, safetensors_bytes(32)).unwrap();
        std::fs::write(&bad, b"<html>login required</html>").unwrap();
        assert!(looks_like_safetensors(&good));
        assert!(!looks_like_safetensors(&bad));
        assert!(!looks_like_safetensors(&dir.path().join("missing.safetensors")));
    }

    #[tokio::test]
    async fn a_download_is_checked_and_named() {
        let dir = tempfile::tempdir().unwrap();
        let data = safetensors_bytes(300_000);
        let url = serve("200 OK", data.clone(), None, 0).await;
        let cancel = start("test_good");
        download_to("test_good", dir.path(), "x.safetensors", &Source { url: &url, format: "safetensors", token: None, gated: false }, &cancel).await.unwrap();
        assert_eq!(std::fs::read(dir.path().join("x.safetensors")).unwrap(), data);
        let jobs = JOBS.lock().unwrap();
        assert_eq!(jobs["test_good"].downloaded, data.len() as u64);
        assert_eq!(jobs["test_good"].total, data.len() as u64);
        assert!(!dir.path().join("x.safetensors.part").exists());
    }

    #[tokio::test]
    async fn a_web_page_is_refused_and_leaves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let url = serve("200 OK", b"<html>sign in</html>".to_vec(), None, 0).await;
        let cancel = start("test_html");
        let error = download_to("test_html", dir.path(), "x.safetensors", &Source { url: &url, format: "safetensors", token: None, gated: false }, &cancel).await.unwrap_err();
        assert!(error.contains("not a valid .safetensors"), "{error}");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn a_server_error_is_reported_with_its_status() {
        let dir = tempfile::tempdir().unwrap();
        let url = serve("503 Service Unavailable", b"busy".to_vec(), None, 0).await;
        let cancel = start("test_503");
        let error = download_to("test_503", dir.path(), "x.safetensors", &Source { url: &url, format: "safetensors", token: None, gated: false }, &cancel).await.unwrap_err();
        assert_eq!(error, "The download server answered 503 Service Unavailable.");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn a_short_download_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let data = safetensors_bytes(1000);
        let url = serve("200 OK", data.clone(), Some(data.len() + 500), 0).await;
        let cancel = start("test_short");
        let error = download_to("test_short", dir.path(), "x.safetensors", &Source { url: &url, format: "safetensors", token: None, gated: false }, &cancel).await.unwrap_err();
        assert!(error.contains("stopped"), "{error}");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn cancel_stops_the_download_and_removes_the_partial_file() {
        let dir = tempfile::tempdir().unwrap();
        let url = serve("200 OK", safetensors_bytes(4 * 1024 * 1024), None, 40).await;
        let cancel = start("test_cancel");
        let flag = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            flag.store(true, Ordering::Relaxed);
        });
        let error = download_to("test_cancel", dir.path(), "x.safetensors", &Source { url: &url, format: "safetensors", token: None, gated: false }, &cancel).await.unwrap_err();
        assert_eq!(error, "", "an empty error means cancelled");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn an_existing_file_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("x.safetensors"), b"mine").unwrap();
        let url = serve("200 OK", safetensors_bytes(64), None, 0).await;
        let cancel = start("test_keep");
        download_to("test_keep", dir.path(), "x.safetensors", &Source { url: &url, format: "safetensors", token: None, gated: false }, &cancel).await.unwrap();
        assert_eq!(std::fs::read(dir.path().join("x.safetensors")).unwrap(), b"mine");
        assert!(!dir.path().join("x.safetensors.part").exists());
    }

    #[test]
    fn ids_must_be_a_list() {
        assert_eq!(ids_of(&Bytes::from_static(br#"{"ids":["dmad_lora"]}"#)).ok().unwrap(), vec!["dmad_lora".to_string()]);
        assert!(ids_of(&Bytes::from_static(b"{}")).ok().unwrap().is_empty());
        assert!(ids_of(&Bytes::from_static(br#"{"ids":"dmad_lora"}"#)).is_err());
    }
}
