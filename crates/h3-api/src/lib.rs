//! Local `/api/v1` server for agents and the MCP server. Same paths, envelopes and error codes as the pack's agent API,
//! so `mcp_server` works against it with `VRGDG_API_URL=http://127.0.0.1:8190/api/v1`.

pub mod app;
pub mod edit_routes;
pub mod embedded;
pub mod error;
pub mod image_jobs;
pub mod image_routes;
pub mod instruction_routes;
pub mod job_handlers;
pub mod continued_prompt;
pub mod llm_jobs;
pub mod lyric_jobs;
pub mod notes_routes;
pub mod pipeline_jobs;
pub mod post_routes;
pub mod refmod_routes;
pub mod reference_routes;
pub mod video_routes;
pub mod llm_routes;
pub mod mcp;
pub mod session_routes;
pub mod speaking_routes;
pub mod setup_routes;
pub mod views;
pub mod vrgdg;

use std::path::Path;
use std::sync::Arc;

use h3_jobs::{JobManager, JobStore};
use h3_project::AppConfig;

pub use app::{router, AppState};

/// Where the job database lives, next to the config file.
pub fn jobs_db_path(config_path: &Path) -> std::path::PathBuf {
    config_path.parent().unwrap_or_else(|| Path::new(".")).join("jobs.db")
}

/// ComfyUI's output folder: the configured one, else the one the running ComfyUI reports. Asking happens on its own
/// thread so this works inside and outside an async runtime. No answer within a few seconds means it is not known yet.
fn detect_output_dir(cfg: &AppConfig) -> Option<std::path::PathBuf> {
    let configured = cfg.comfy_output_dir.trim();
    if !configured.is_empty() {
        return Some(std::path::PathBuf::from(configured));
    }
    let cfg = cfg.clone();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().ok()?;
        runtime.block_on(async { tokio::time::timeout(std::time::Duration::from_secs(4), h3_pipeline::context::comfy_dirs(&cfg)).await.ok()?.ok() }).map(|dirs| std::path::PathBuf::from(dirs.output))
    })
    .join()
    .ok()
    .flatten()
}

/// Build the shared state for the config file the program was started with (`H3_CONFIG` can name a file that is not `config.json`).
/// The job database and the saved settings sit next to it.
pub fn build_state_for(cfg: AppConfig, config_path: &Path) -> Result<Arc<AppState>, String> {
    build_state_at(cfg, &jobs_db_path(config_path), config_path)
}

/// Like [`build_state_for`] for a folder that holds `jobs.db` and `config.json`, which is how the tests lay theirs out.
pub fn build_state(cfg: AppConfig, db_path: &Path) -> Result<Arc<AppState>, String> {
    let config_path = db_path.parent().unwrap_or_else(|| Path::new(".")).join("config.json");
    build_state_at(cfg, db_path, &config_path)
}

/// Open the job database, mark jobs a stopped app left unfinished as interrupted and build the shared state.
fn build_state_at(mut cfg: AppConfig, db_path: &Path, config_path: &Path) -> Result<Arc<AppState>, String> {
    cfg.llm.comfy_url = cfg.comfy_url.clone();
    let store = JobStore::open(db_path).map_err(|e| e.to_string())?;
    let jobs = JobManager::new(store);
    match jobs.recover_on_startup() {
        Ok(0) => {}
        Ok(n) => eprintln!("[VRGDG-RS API] {n} unfinished job(s) from the last run are marked interrupted."),
        Err(e) => eprintln!("[VRGDG-RS API] Could not check for interrupted jobs: {e}"),
    }
    let output = detect_output_dir(&cfg);
    let roots = views::project_roots(&cfg.project_root, output.as_deref());
    let presets_dir = db_path.parent().unwrap_or_else(|| Path::new(".")).join("image_presets");
    let llm = std::sync::RwLock::new(cfg.llm.clone());
    Ok(Arc::new(AppState { llm, config_path: config_path.to_path_buf(), cfg, roots, comfy_output: output, jobs, presets: h3_image::PresetStore::new(&presets_dir), presets_dir, edit_lock: tokio::sync::Mutex::new(()), builder_events: tokio::sync::broadcast::channel(64).0, object_info_cache: tokio::sync::Mutex::new(None), web_dir: std::env::var_os("H3_WEB").filter(|p| !p.is_empty()).map(std::path::PathBuf::from).filter(|p| p.is_dir()) }))
}

/// Take the API's address. The desktop app does this before it opens its window, so the window finds the server there.
pub async fn bind(state: &AppState) -> std::io::Result<tokio::net::TcpListener> {
    tokio::net::TcpListener::bind(&state.cfg.api_bind).await
}

/// Serve the API on an address taken with [`bind`] until the process stops.
pub async fn serve_on(listener: tokio::net::TcpListener, state: Arc<AppState>) -> std::io::Result<()> {
    if let Ok(address) = listener.local_addr() {
        eprintln!("[VRGDG-RS API] Listening on http://{address}/api/v1");
    }
    axum::serve(listener, router(state)).await
}

/// Serve the API until the process stops.
pub async fn serve(state: Arc<AppState>) -> std::io::Result<()> {
    let listener = bind(&state).await?;
    serve_on(listener, state).await
}
