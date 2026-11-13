//! The Models dialog's Dmad Models tab and the two MOSS-Image LoRA buttons: allowlisted LoRA downloads into ComfyUI's
//! `models/loras` folder. Port of `core/model_downloads.py` and its three routes in `core/system_routes.py`; the routes and
//! replies are the same, so the tab and the button work the same in the pack and here.
//!
//! The browser names a model by id. It never sends a URL or a path, so it cannot make this fetch anything else or write
//! anywhere else. A download goes to `<name>.part` first, is checked, and only then gets its real name, so ComfyUI never
//! lists a half-written file and a failed download leaves nothing behind. A file that already exists is never replaced.
//! The folder comes from ComfyUI (`/internal/folder_paths`), so the file lands where ComfyUI reads its LoRAs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use axum::body::Bytes;
use axum::extract::State;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use futures_util::StreamExt;
use h3_comfy::ComfyClient;
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;

use super::{body_object, into_response, Fail, Out};
use crate::app::AppState;

const MAX_SAFETENSORS_HEADER_BYTES: u64 = 100 * 1024 * 1024;

/// One downloadable model. Keep the urls and file names in step with `DOWNLOADABLE_MODELS` in `core/model_downloads.py`
/// and `DMAD_MODEL_FILES` / `MOSS_IMAGE_MODEL_DOWNLOADS` in `web/music_video_builder/models.mjs`. `filename` is the name the
/// file is saved as, which need not be the name in the url.
struct Model {
    id: &'static str,
    label: &'static str,
    group: &'static str,
    folder: &'static str,
    filename: &'static str,
    url: &'static str,
    page: &'static str,
    note: &'static str,
}

const MODELS: [Model; 4] = [
    Model {
        id: "dmad_lora",
        label: "DMAD 4-step LoRA",
        group: "dmad",
        folder: "loras",
        filename: "dmad_minimax_h3_4step_full_critic_comfyui.safetensors",
        url: "https://huggingface.co/ZhengmingYu/DMAD/resolve/main/minimax_h3/dmad_minimax_h3_4step_full_critic_comfyui.safetensors",
        page: "https://huggingface.co/ZhengmingYu/DMAD/tree/main/minimax_h3",
        note: "Pass 1 LoRA. Use the ComfyUI file (name ends in _comfyui). The Diffusers file does not load in ComfyUI.",
    },
    Model {
        id: "dmad_refiner",
        label: "Refiner LoRA",
        group: "dmad",
        folder: "loras",
        filename: "minimax_h3_open_regenerate_2k_refiner_rank_16_bf16.safetensors",
        url: "https://huggingface.co/Kijai/MiniMax-H3-experimental/resolve/main/loras/minimax_h3_open_regenerate_2k_refiner_rank_16_bf16.safetensors",
        page: "https://huggingface.co/Kijai/MiniMax-H3-experimental/blob/main/loras/minimax_h3_open_regenerate_2k_refiner_rank_16_bf16.safetensors",
        note: "Pass 2 LoRA. Restores detail in 2 to 3 steps.",
    },
    // Hugging Face calls this file lora.safetensors. It is saved under the name the MOSS-Image settings look for.
    Model {
        id: "moss_turbo_lora",
        label: "MOSS-Image 8-step Turbo LoRA",
        group: "moss",
        folder: "loras",
        filename: "moss_image_sft_8_steps_dmd_lora.safetensors",
        url: "https://huggingface.co/lodestones/MOSS-Image-SFT-8-steps-dmd/resolve/main/lora.safetensors",
        page: "https://huggingface.co/lodestones/MOSS-Image-SFT-8-steps-dmd/tree/main",
        note: "Samples MOSS-Image in 8 steps. Saved as moss_image_sft_8_steps_dmd_lora.safetensors.",
    },
    // Hugging Face calls this file adapter_model.safetensors. It is saved under the name the MOSS-Image settings look for.
    Model {
        id: "moss_edit_lora",
        label: "MOSS-Image Edit LoRA",
        group: "moss",
        folder: "loras",
        filename: "moss_image_edit_lora.safetensors",
        url: "https://huggingface.co/chinoll/Moss-Image-Edit/resolve/main/adapter_model.safetensors",
        page: "https://huggingface.co/chinoll/Moss-Image-Edit/tree/main",
        note: "Follows a written instruction with one to four reference images. Saved as moss_image_edit_lora.safetensors. Needs a ComfyUI newer than v0.42.1.",
    },
];

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
}

fn find_model(id: &str) -> Option<&'static Model> {
    MODELS.iter().find(|model| model.id == id)
}

/// ComfyUI's folders for one kind of model (`loras`), in its order. The first one is where a download goes.
async fn folder_dirs(state: &AppState, folder: &str) -> Vec<PathBuf> {
    let Some(paths) = ComfyClient::new(state.cfg.comfy_base()).folder_paths().await else { return Vec::new() };
    paths
        .get(folder)
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(Value::as_str).filter(|p| !p.is_empty()).map(PathBuf::from).collect())
        .unwrap_or_default()
}

/// Where this file already is in any of the folders (subfolders included), else `None`.
fn installed_path(dirs: &[PathBuf], filename: &str) -> Option<PathBuf> {
    fn search(dir: &Path, filename: &str) -> Option<PathBuf> {
        let mut folders = Vec::new();
        for entry in std::fs::read_dir(dir).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                folders.push(path);
            } else if path.file_name().is_some_and(|name| name == filename) {
                return Some(path);
            }
        }
        folders.iter().find_map(|folder| search(folder, filename))
    }
    dirs.iter().find_map(|dir| search(dir, filename))
}

fn job_view(job: Option<&Job>) -> Value {
    match job {
        Some(job) => json!({"state": job.state, "downloaded": job.downloaded, "total": job.total, "error": job.error}),
        None => json!({"state": "idle", "downloaded": 0, "total": 0, "error": ""}),
    }
}

async fn model_status(state: &AppState) -> Vec<Value> {
    let dirs = folder_dirs(state, "loras").await;
    MODELS
        .iter()
        .map(|model| {
            let path = installed_path(&dirs, model.filename);
            let size = path.as_ref().and_then(|p| std::fs::metadata(p).ok()).map_or(0, |m| m.len());
            let job = JOBS.lock().ok().and_then(|jobs| jobs.get(model.id).cloned());
            json!({
                "id": model.id,
                "label": model.label,
                "group": model.group,
                "folder": model.folder,
                "filename": model.filename,
                "url": model.url,
                "page": model.page,
                "note": model.note,
                "installed": path.is_some(),
                "path": path.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
                "size": size,
                "destination": dirs.first().map(|p| p.display().to_string()).unwrap_or_default(),
                "job": job_view(job.as_ref()),
            })
        })
        .collect()
}

/// `GET /vrgdg/video_builder/models/status`.
async fn status(State(state): State<Arc<AppState>>) -> Response {
    let models = model_status(&state).await;
    into_response(Ok(Out::Fields(json!({"models": models}))))
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
        let unknown: Vec<&String> = ids.iter().filter(|id| find_model(id).is_none()).collect();
        if !unknown.is_empty() {
            let names: Vec<&str> = unknown.iter().map(|id| id.as_str()).collect();
            return Err(Fail::from(format!("Unknown model: {}.", names.join(", "))));
        }
        let dirs = folder_dirs(&state, "loras").await;
        let mut started: Vec<String> = Vec::new();
        for id in &ids {
            let Some(model) = find_model(id) else { continue };
            if started.contains(id) || installed_path(&dirs, model.filename).is_some() {
                continue;
            }
            let Ok(mut jobs) = JOBS.lock() else { continue };
            if jobs.get(id.as_str()).is_some_and(|job| job.state == "downloading") {
                continue;
            }
            jobs.insert(id.clone(), Job { state: "downloading", downloaded: 0, total: 0, error: String::new(), cancel: Arc::new(AtomicBool::new(false)) });
            started.push(id.clone());
        }
        if !started.is_empty() {
            let Some(destination) = dirs.first().cloned() else {
                for id in &started {
                    set_job(id, |job| {
                        job.state = "error";
                        job.error = "ComfyUI has no 'loras' model folder. Is ComfyUI running?".to_string();
                    });
                }
                return Ok(json!({"started": started, "models": model_status(&state).await}));
            };
            let queue = started.clone();
            tokio::spawn(async move {
                for id in queue {
                    run_download(&id, &destination).await;
                }
            });
        }
        Ok(json!({"started": started, "models": model_status(&state).await}))
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

async fn run_download(id: &str, destination: &Path) {
    let Some(model) = find_model(id) else { return };
    let cancel = JOBS.lock().ok().and_then(|jobs| jobs.get(id).map(|job| job.cancel.clone()));
    let Some(cancel) = cancel else { return };
    if cancel.load(Ordering::Relaxed) {
        set_job(id, |job| job.state = "cancelled");
        return;
    }
    match download_to(id, model.url, destination, model.filename, &cancel).await {
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

/// Download `url` into `dir/filename`. An empty error means it was cancelled. Reports progress through the job for `id`.
async fn download_to(id: &str, url: &str, dir: &Path, filename: &str, cancel: &AtomicBool) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    let target = dir.join(filename);
    let part = dir.join(format!("{filename}.part"));
    let result = fetch_to_part(id, url, &part, cancel).await;
    let finished = match result {
        Ok(()) => {
            if !looks_like_safetensors(&part) {
                Err("The downloaded file is not a valid .safetensors file. Try again, or download it from the model page.".to_string())
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

async fn fetch_to_part(id: &str, url: &str, part: &Path, cancel: &AtomicBool) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .user_agent("comfyui-vrgamedevgirl-model-installer")
        .connect_timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;
    let response = client.get(url).send().await.map_err(|e| format!("Could not reach the download server: {e}"))?;
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

    fn start(id: &str) -> Arc<AtomicBool> {
        let cancel = Arc::new(AtomicBool::new(false));
        JOBS.lock().unwrap().insert(
            id.to_string(),
            Job { state: "downloading", downloaded: 0, total: 0, error: String::new(), cancel: cancel.clone() },
        );
        cancel
    }

    #[test]
    fn the_allowlist_holds_the_dmad_files_and_the_two_moss_loras_with_hugging_face_urls() {
        let ids: Vec<&str> = MODELS.iter().map(|m| m.id).collect();
        assert_eq!(ids, ["dmad_lora", "dmad_refiner", "moss_turbo_lora", "moss_edit_lora"]);
        for model in &MODELS {
            assert!(model.url.starts_with("https://huggingface.co/") && model.url.contains("/resolve/"), "{}", model.id);
            // Only the two MOSS-Image LoRAs are saved under a name other than the one in their url.
            assert_eq!(model.url.ends_with(model.filename), !matches!(model.id, "moss_turbo_lora" | "moss_edit_lora"), "{}", model.id);
            assert!(model.filename.ends_with(".safetensors"));
        }
        assert!(find_model("../../evil").is_none());
    }

    #[test]
    fn the_moss_turbo_lora_is_saved_as_the_name_the_settings_look_for() {
        let model = find_model("moss_turbo_lora").unwrap();
        assert_eq!(model.filename, "moss_image_sft_8_steps_dmd_lora.safetensors");
        assert!(model.url.ends_with("/lora.safetensors"));
        assert_eq!((model.group, model.folder), ("moss", "loras"));
    }

    #[tokio::test]
    async fn the_turbo_lora_download_is_saved_under_its_own_name_not_lora_safetensors() {
        let dir = tempfile::tempdir().unwrap();
        let data = safetensors_bytes(2048);
        let url = serve("200 OK", data.clone(), None, 0).await.replace("file.safetensors", "lora.safetensors");
        let model = find_model("moss_turbo_lora").unwrap();
        let cancel = start("test_moss_turbo_name");
        download_to("test_moss_turbo_name", &url, dir.path(), model.filename, &cancel).await.unwrap();
        assert_eq!(std::fs::read(dir.path().join("moss_image_sft_8_steps_dmd_lora.safetensors")).unwrap(), data);
        assert!(!dir.path().join("lora.safetensors").exists());
    }

    #[test]
    fn the_moss_edit_lora_is_saved_as_the_name_the_settings_look_for() {
        let model = find_model("moss_edit_lora").unwrap();
        assert_eq!(model.filename, "moss_image_edit_lora.safetensors");
        assert!(model.url.ends_with("/adapter_model.safetensors"));
        assert_eq!((model.group, model.folder), ("moss", "loras"));
    }

    #[tokio::test]
    async fn the_edit_lora_download_is_saved_under_its_own_name_not_the_one_in_the_url() {
        let dir = tempfile::tempdir().unwrap();
        let data = safetensors_bytes(2048);
        // The server's file is called adapter_model.safetensors, as on Hugging Face.
        let url = serve("200 OK", data.clone(), None, 0).await.replace("file.safetensors", "adapter_model.safetensors");
        let model = find_model("moss_edit_lora").unwrap();
        let cancel = start("test_moss_name");
        download_to("test_moss_name", &url, dir.path(), model.filename, &cancel).await.unwrap();
        assert_eq!(std::fs::read(dir.path().join("moss_image_edit_lora.safetensors")).unwrap(), data);
        assert!(!dir.path().join("adapter_model.safetensors").exists());
    }

    #[test]
    fn installed_files_are_found_in_subfolders() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("minimax")).unwrap();
        std::fs::write(dir.path().join("minimax").join("a.safetensors"), b"x").unwrap();
        let dirs = vec![dir.path().to_path_buf()];
        assert!(installed_path(&dirs, "a.safetensors").is_some());
        assert!(installed_path(&dirs, "b.safetensors").is_none());
        assert!(installed_path(&[], "a.safetensors").is_none());
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
        download_to("test_good", &url, dir.path(), "x.safetensors", &cancel).await.unwrap();
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
        let error = download_to("test_html", &url, dir.path(), "x.safetensors", &cancel).await.unwrap_err();
        assert!(error.contains("not a valid .safetensors"), "{error}");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn a_server_error_is_reported_with_its_status() {
        let dir = tempfile::tempdir().unwrap();
        let url = serve("503 Service Unavailable", b"busy".to_vec(), None, 0).await;
        let cancel = start("test_503");
        let error = download_to("test_503", &url, dir.path(), "x.safetensors", &cancel).await.unwrap_err();
        assert_eq!(error, "The download server answered 503 Service Unavailable.");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn a_short_download_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let data = safetensors_bytes(1000);
        let url = serve("200 OK", data.clone(), Some(data.len() + 500), 0).await;
        let cancel = start("test_short");
        let error = download_to("test_short", &url, dir.path(), "x.safetensors", &cancel).await.unwrap_err();
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
        let error = download_to("test_cancel", &url, dir.path(), "x.safetensors", &cancel).await.unwrap_err();
        assert_eq!(error, "", "an empty error means cancelled");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn an_existing_file_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("x.safetensors"), b"mine").unwrap();
        let url = serve("200 OK", safetensors_bytes(64), None, 0).await;
        let cancel = start("test_keep");
        download_to("test_keep", &url, dir.path(), "x.safetensors", &cancel).await.unwrap();
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
