//! RefMods Studio routes of the Builder: the saved RefMods, picking and describing pictures, and creating a RefMod. Ports of the routes in
//! `minimax/refmod_library.py`, `minimax/refmod_picker.py` and `minimax/refmod_studio.py`.
//!
//! Creating a RefMod queues ComfyUI-MiniMaxH3Mod's *Create H3 RefMod* node with the prepared pictures. The node writes the file into the RefMod
//! folder of ComfyUI. This app writes the preview next to it.

use std::collections::HashMap;
use std::path::{Path as FsPath, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{Multipart, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post as axum_post};
use axum::Router;
use h3_builder::prompt_writers::RunnerLlm;
use h3_builder::refmod_studio::{self as studio, Request};
use h3_comfy::queue::extract_texts;
use h3_comfy::ComfyClient;
use h3_core::pyval::{text_or_empty, Obj};
use h3_llm::runner::runner_kind;
use h3_pipeline::context::refmod_dirs_of;
use h3_pipeline::refmod_library::{find_refmod, list_refmods, PREVIEW_SUFFIX};
use serde_json::{json, Value};

use super::llm::base_config;
use super::{body_object, failure, into_response, Fail, Out};
use crate::app::AppState;

const EXTRACT_NODE: &str = "MiniMaxH3RefModExtract";
const VIDEO_VAE: &str = "minimax_h3_video_vae_fp16.safetensors";
/// Only one RefMod is made at a time, like the pack's lock.
static CREATING: AtomicBool = AtomicBool::new(false);

struct Creating;

impl Creating {
    fn start() -> Option<Self> {
        CREATING.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).ok().map(|_| Self)
    }
}

impl Drop for Creating {
    fn drop(&mut self) {
        CREATING.store(false, Ordering::SeqCst);
    }
}

fn bad(message: impl Into<String>) -> Fail {
    Fail::new(StatusCode::BAD_REQUEST, message)
}

fn upload_folder() -> PathBuf {
    std::env::temp_dir().join("vrgdg_refmod_studio")
}

fn nanos() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
}

async fn library(State(state): State<Arc<AppState>>, Query(query): Query<HashMap<String, String>>) -> Response {
    let dirs = refmod_dirs_of(&state.cfg).await;
    let folder = query.get("folder").cloned().unwrap_or_default();
    let entries = tokio::task::spawn_blocking(move || list_refmods(&dirs, &folder)).await.unwrap_or_default();
    into_response(Ok(Out::Fields(json!({"refmods": entries.iter().map(|e| e.public()).collect::<Vec<_>>()}))))
}

async fn delete(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let payload = match body_object(&body) {
        Ok(payload) => payload,
        Err(fail) => return into_response(Err(fail)),
    };
    let dirs = refmod_dirs_of(&state.cfg).await;
    let name = text_or_empty(payload.get("name"));
    let result = tokio::task::spawn_blocking(move || studio::delete_refmod(&dirs, &name)).await.unwrap_or_else(|e| Err(e.to_string()));
    into_response(result.map(Out::Fields).map_err(bad))
}

async fn preview(State(state): State<Arc<AppState>>, Query(query): Query<HashMap<String, String>>) -> Response {
    let dirs = refmod_dirs_of(&state.cfg).await;
    let name = query.get("name").cloned().unwrap_or_default();
    let path = studio::preview_path(&dirs, &name).filter(|p| p.is_file());
    let Some(path) = path else { return failure(StatusCode::NOT_FOUND, "No preview for this RefMod.") };
    match tokio::fs::read(&path).await {
        Ok(bytes) => Response::builder().header(header::CONTENT_TYPE, "image/png").header(header::CACHE_CONTROL, "no-cache").body(Body::from(bytes)).unwrap_or_else(|e| failure(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
        Err(_) => failure(StatusCode::NOT_FOUND, "No preview for this RefMod."),
    }
}

async fn pick_images() -> Response {
    let result = tokio::task::spawn_blocking(studio::pick_image_files).await.unwrap_or_else(|e| Err(e.to_string()));
    into_response(result.map(|paths| Out::Fields(json!({"paths": paths}))).map_err(bad))
}

async fn describe_prompts() -> Response {
    let prompts: Obj = studio::describe_instructions().into_iter().map(|(key, text)| (key.to_string(), Value::String(text))).collect();
    into_response(Ok(Out::Fields(json!({"prompts": prompts}))))
}

async fn upload_image(mut multipart: Multipart) -> Response {
    let result: Result<String, String> = async {
        let field = multipart.next_field().await.map_err(|e| e.to_string())?;
        let Some(field) = field.filter(|f| f.name() == Some("image")) else { return Err("Send the file in a multipart field named 'image'.".to_string()) };
        let filename = field.file_name().unwrap_or("image").to_string();
        let data = field.bytes().await.map_err(|e| e.to_string())?;
        if data.len() > studio::MAX_UPLOAD_BYTES {
            return Err("Image is larger than 64 MB.".into());
        }
        let unique = format!("{:08x}", nanos() as u32);
        let path = tokio::task::spawn_blocking(move || studio::save_upload(&upload_folder(), &filename, &data, &unique)).await.map_err(|e| e.to_string())??;
        Ok(path.display().to_string())
    }
    .await;
    into_response(result.map(|path| Out::Fields(json!({"path": path}))).map_err(bad))
}

async fn describe(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let body = match body_object(&body) {
        Ok(body) => body,
        Err(fail) => return into_response(Err(fail)),
    };
    let result: Result<String, String> = async {
        let paths: Vec<String> = match body.get("paths") {
            Some(Value::Array(items)) => items.iter().map(|p| text_or_empty(Some(p))).collect(),
            _ => Vec::new(),
        };
        if paths.is_empty() {
            return Err("Add at least one image first.".into());
        }
        let images = tokio::task::spawn_blocking(move || studio::load_pictures(&paths)).await.map_err(|e| e.to_string())??;
        let base = base_config(&state);
        // A local runner is not used for pictures here: LM Studio answers with the model that is already loaded.
        let mut payload: Obj = body.get("llm").and_then(Value::as_object).cloned().unwrap_or_default();
        if !matches!(runner_kind(&payload), "lm_studio" | "llm_api" | "own_server") {
            payload = json!({"text_runner": "lm_studio", "lmstudio_base_url": base.lm_studio_base_url}).as_object().cloned().unwrap_or_default();
        }
        let llm = RunnerLlm { payload: &payload, base: &base };
        let concept = body.get("concept_type").filter(|v| h3_core::pyval::truthy(v)).map(|v| text_or_empty(Some(v))).unwrap_or_else(|| "identity".into());
        studio::describe_images(&llm, &images, &concept, "", &text_or_empty(body.get("name_hint")), 6, 768).await
    }
    .await;
    into_response(result.map(|description| Out::Fields(json!({"description": description}))).map_err(bad))
}

/// Why creating a RefMod failed: the file is already there, or something else.
pub(crate) enum CreateError {
    Exists { path: String, message: String },
    Failed(String),
}

impl From<String> for CreateError {
    fn from(message: String) -> Self {
        Self::Failed(message)
    }
}

fn create_response(result: Result<Value, CreateError>) -> Response {
    match result {
        Ok(value) => into_response(Ok(Out::Fields(value))),
        Err(CreateError::Exists { path, message }) => (StatusCode::CONFLICT, axum::Json(json!({"ok": false, "exists": true, "path": path, "error": message}))).into_response(),
        Err(CreateError::Failed(message)) => {
            eprintln!("[VRGDG-RS RefMod] Create failed: {message}");
            failure(StatusCode::BAD_REQUEST, message)
        }
    }
}

/// The graph that makes the RefMod: the video VAE, one Load Image node for each prepared picture and the Create H3 RefMod node.
fn create_graph(request: &Request, image_names: &[String]) -> Value {
    let mut graph = Obj::new();
    graph.insert("1".into(), json!({"class_type": "VAELoader", "inputs": {"vae_name": VIDEO_VAE}}));
    let mut inputs = studio::fixed_settings();
    inputs.insert("name".into(), json!(request.name));
    inputs.insert("mode".into(), json!(request.mode));
    inputs.insert("concept_type".into(), json!(studio::node_concept_type(&request.concept_type)));
    inputs.insert("identity".into(), json!(request.steps));
    inputs.insert("description".into(), json!(request.description));
    inputs.insert("save".into(), json!(true));
    inputs.insert("subfolder".into(), json!(request.concept_type));
    inputs.insert("vae".into(), json!(["1", 0]));
    for (index, name) in image_names.iter().enumerate() {
        let node = format!("{}", 10 + index);
        graph.insert(node.clone(), json!({"class_type": "LoadImage", "inputs": {"image": name}}));
        inputs.insert(format!("refs_image.ref_image_{index}"), json!([node, 0]));
    }
    graph.insert("3".into(), json!({"class_type": EXTRACT_NODE, "inputs": Value::Object(inputs)}));
    Value::Object(graph)
}

/// `create_refmod`: make the RefMod file and its preview.
pub(crate) async fn create_refmod(state: &AppState, payload: Obj) -> Result<Value, CreateError> {
    let request = studio::validate_request(&payload)?;
    let client = ComfyClient::new(state.cfg.comfy_base());
    let node = client.object_info_for(EXTRACT_NODE).await.map_err(|e| e.to_string())?;
    if node.get(EXTRACT_NODE).is_none() {
        return Err("ComfyUI-MiniMaxH3Mod is not installed or did not load. Install it in custom_nodes and restart ComfyUI.".to_string().into());
    }
    let dirs = refmod_dirs_of(&state.cfg).await;
    let Some(root) = dirs.first() else { return Err("ComfyUI did not say where it keeps RefMods. Is ComfyUI running?".to_string().into()) };
    let target = root.join(&request.concept_type).join(format!("{}.safetensors", request.name));
    if target.is_file() && !request.overwrite {
        return Err(CreateError::Exists { path: target.display().to_string(), message: format!("A RefMod already exists at {}.", target.display()) });
    }
    let Some(_guard) = Creating::start() else { return Err("A RefMod is already being created. Wait for it to finish.".to_string().into()) };
    eprintln!("[VRGDG-RS RefMod] Creating {} RefMod '{}' from {} image(s).", request.concept_type, request.name, request.paths.len());
    let (paths, crops, quality) = (request.paths.clone(), request.crops.clone(), request.quality.clone());
    let (canvases, (width, height)) = tokio::task::spawn_blocking(move || studio::prepare_images(&paths, &crops, &quality)).await.map_err(|e| e.to_string())??;
    let tokens = studio::tokens_for_canvas(canvases.len() as i64, i64::from(width), i64::from(height));
    if request.mode == "encode" && tokens > studio::MAX_TOKENS {
        return Err(format!(
            "These images would use {} tokens ({} x {width}x{height}), over the {} limit. Choose a lower quality, trim closer to the subject, or remove images.",
            thousands(tokens),
            canvases.len(),
            thousands(studio::MAX_TOKENS)
        )
        .into());
    }
    if canvases.len() > 16 {
        return Err("ComfyUI's Create H3 RefMod node takes up to 16 images at a time.".to_string().into());
    }
    eprintln!("[VRGDG-RS RefMod] Canvas {width}x{height}, {} frame(s), {} tokens.", canvases.len(), thousands(tokens));
    let vae = client.object_info_for("VAELoader").await.map_err(|e| e.to_string())?;
    let vae_options = vae.pointer("/VAELoader/input/required/vae_name/0").and_then(Value::as_array).cloned().unwrap_or_default();
    if !vae_options.iter().any(|v| v.as_str() == Some(VIDEO_VAE)) {
        return Err(format!("The MiniMax H3 video VAE '{VIDEO_VAE}' was not found in models/vae.").into());
    }
    let stamp = nanos();
    let mut names = Vec::new();
    for (index, canvas) in canvases.iter().enumerate() {
        let png = studio::png_bytes(canvas)?;
        names.push(client.upload_image(&format!("vrgdg_refmod_{stamp}_{index}.png"), png, true).await.map_err(|e| e.to_string())?);
    }
    let queued = client.queue_prompt(&create_graph(&request, &names)).await.map_err(|e| e.to_string())?;
    let cancel = Arc::new(AtomicBool::new(false));
    let history = client.wait_for_prompt(&queued.prompt_id, Duration::from_secs(3600), Duration::from_secs(1), &cancel, |_| {}).await.map_err(|e| e.to_string())?;
    let details: Value = extract_texts(&history, &queued.prompt_id).first().and_then(|t| serde_json::from_str(t).ok()).unwrap_or(Value::Null);
    let saved: PathBuf = details.get("saved_paths").and_then(Value::as_array).and_then(|a| a.first()).and_then(Value::as_str).map(PathBuf::from).unwrap_or(target);
    // The first picture as it was encoded is the preview the pickers show.
    if let Some(first) = canvases.first() {
        let preview = FsPath::new(&saved).with_extension("").with_extension(&PREVIEW_SUFFIX[1..]);
        if let Err(error) = studio::save_preview(first, &preview) {
            eprintln!("[VRGDG-RS RefMod] Could not save a preview: {error}");
        }
    }
    eprintln!("[VRGDG-RS RefMod] Saved {}", saved.display());
    let entry = find_refmod(&dirs, &format!("{}/{}", request.concept_type, request.name));
    Ok(json!({
        "path": saved.display().to_string(),
        "folder": request.concept_type,
        "name": request.name,
        "kind": details.get("kind").cloned().or_else(|| entry.as_ref().map(|e| json!(e.kind))).unwrap_or(Value::Null),
        "latent_frames": details.get("latent_frames").cloned().or_else(|| entry.as_ref().map(|e| json!(e.frames))).unwrap_or(Value::Null),
        "tokens": details.get("tokens").cloned().or_else(|| entry.as_ref().map(|e| json!(e.tokens))).unwrap_or(Value::Null),
        "canvas": [width, height],
        "quality": request.quality,
        "size": std::fs::metadata(&saved).map(|m| m.len()).unwrap_or(0),
    }))
}

/// `12345` as `12,345`.
fn thousands(n: i64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

async fn create(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let payload = match body_object(&body) {
        Ok(payload) => payload,
        Err(fail) => return into_response(Err(fail)),
    };
    create_response(create_refmod(&state, payload).await)
}

/// A picture sent as base64 data, written to the Studio's temp folder.
fn save_data_url(data: &str, name: &str) -> Result<String, String> {
    use base64::Engine;
    let payload = match data.strip_prefix("data:").and_then(|d| d.split_once(',')) {
        Some((_, rest)) => rest,
        None => data,
    };
    let raw = base64::engine::general_purpose::STANDARD.decode(payload.trim()).map_err(|e| format!("Invalid base64-encoded string: {e}"))?;
    if raw.len() > studio::MAX_UPLOAD_BYTES {
        return Err("Image is larger than 64 MB.".into());
    }
    let extension = FsPath::new(name).extension().map(|e| format!(".{}", e.to_string_lossy().to_lowercase())).filter(|e| studio::IMAGE_EXTENSIONS.contains(&e.as_str())).unwrap_or_else(|| ".png".into());
    let unique = format!("{:08x}", nanos() as u32);
    studio::save_upload(&upload_folder(), &format!("reference{extension}"), &raw, &unique).map(|p| p.display().to_string())
}

async fn from_image(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let payload = match body_object(&body) {
        Ok(payload) => payload,
        Err(fail) => return into_response(Err(fail)),
    };
    let result: Result<Value, CreateError> = async {
        let source = payload.clone();
        let paths = tokio::task::spawn_blocking(move || studio::collect_image_paths(&source, &save_data_url)).await.map_err(|e| e.to_string())??;
        let mut next = payload;
        next.insert("paths".into(), json!(paths));
        create_refmod(&state, next).await
    }
    .await;
    create_response(result)
}

pub fn routes(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route("/vrgdg/refmod/library", get(library))
        .route("/vrgdg/refmod/delete", axum_post(delete))
        .route("/vrgdg/refmod/preview", get(preview))
        .route("/vrgdg/refmod/pick_images", axum_post(pick_images))
        .route("/vrgdg/refmod/describe_prompts", get(describe_prompts))
        .route("/vrgdg/refmod/upload_image", axum_post(upload_image))
        .route("/vrgdg/refmod/describe", axum_post(describe))
        .route("/vrgdg/refmod/from_image", axum_post(from_image))
        .route("/vrgdg/refmod/create", axum_post(create))
}
