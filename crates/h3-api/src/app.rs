//! The router and its handlers.

use std::collections::HashMap;
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use h3_comfy::models::model_choices;
use h3_comfy::ComfyClient;
use h3_core::pyval::Obj;
use h3_jobs::JobManager;
use h3_media::Tools;
use h3_pipeline::{connect, prepare_scene_render, RenderParams};
use h3_project::{latents, load_session, persist_session, AppConfig, LoadedSession, Stamp};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use tower_http::cors::{AllowOrigin, Any, CorsLayer};

use crate::error::{respond, success, ApiError, ApiResult};
use crate::job_handlers::{self, VIDEOS_RENDER_BATCH, VIDEO_RENDER, VIDEO_STITCH};
use crate::edit_routes as er;
use crate::image_routes as ir;
use crate::instruction_routes as ins;
use crate::llm_routes as lr;
use crate::session_routes as sr;
use crate::speaking_routes as sp;
use crate::views::{self, SceneFilter};

pub struct AppState {
    pub cfg: AppConfig,
    /// The LLM settings in use. They change at run time and are saved to `config_path`.
    pub llm: std::sync::RwLock<h3_llm::LlmConfig>,
    pub config_path: PathBuf,
    pub roots: Vec<PathBuf>,
    /// ComfyUI's output folder, when known. The Builder keeps its profiles and defaults there, like the pack.
    pub comfy_output: Option<PathBuf>,
    pub jobs: JobManager,
    pub presets: h3_image::PresetStore,
    pub presets_dir: PathBuf,
    /// Edits of a project run one at a time, so two requests cannot overwrite each other's changes.
    pub edit_lock: tokio::sync::Mutex<()>,
    /// Messages for the Builder's event stream (`/vrgdg/events`), as JSON text.
    pub builder_events: tokio::sync::broadcast::Sender<String>,
    /// ComfyUI's node list (`/object_info`, several MB), kept for a short while because the Builder asks for it often.
    pub object_info_cache: tokio::sync::Mutex<Option<(std::time::Instant, Arc<Value>)>>,
    /// The app's screens, served beside the API when set (`h3 serve --web`). The desktop app embeds them instead.
    pub web_dir: Option<PathBuf>,
    /// What an agent is doing, for the open Builder (see `activity.rs`).
    pub activity: crate::activity::Feed,
}

impl AppState {
    /// ComfyUI's node list, at most 60 seconds old. `fresh` asks again.
    pub async fn object_info(&self, fresh: bool) -> Result<Arc<Value>, String> {
        let mut cache = self.object_info_cache.lock().await;
        if let Some((at, info)) = cache.as_ref() {
            if !fresh && at.elapsed() < std::time::Duration::from_secs(60) {
                return Ok(Arc::clone(info));
            }
        }
        let info = Arc::new(h3_comfy::ComfyClient::new(self.cfg.comfy_base()).object_info().await.map_err(|e| e.to_string())?);
        *cache = Some((std::time::Instant::now(), Arc::clone(&info)));
        Ok(info)
    }

    /// The folders and tools a Builder call works with.
    pub fn builder_env(&self) -> h3_builder::Env {
        h3_builder::Env {
            output: self.comfy_output.clone().unwrap_or_else(|| self.cfg.project_root.clone()),
            roots: self.roots.clone(),
            ffmpeg: self.cfg.ffmpeg_path.clone(),
            ffprobe: self.cfg.ffprobe_path.clone(),
            luts: self.luts_dir(),
        }
    }

    /// Where `.cube` LUT files are kept: a `luts` folder next to the config file.
    pub fn luts_dir(&self) -> PathBuf {
        self.config_path.parent().map(std::path::Path::to_path_buf).unwrap_or_default().join("luts")
    }
}

pub type Shared = State<Arc<AppState>>;

/// Build the `/api/v1` router and register the job handlers.
pub fn router(state: Arc<AppState>) -> Router {
    job_handlers::register(&state);
    crate::image_jobs::register(&state);
    crate::llm_jobs::register(&state);
    crate::story_jobs::register(&state);
    crate::render_files::register(&state);
    crate::lyric_jobs::register(&state);
    crate::video_routes::register(&state);
    crate::post_routes::register(&state);
    crate::pipeline_jobs::register(&state);
    crate::speaking_routes::register(&state);
    crate::activity::follow_jobs(&state);
    let v1 = Router::new()
        .route("/health", get(health))
        .route("/meta", get(meta))
        .route("/models", get(models))
        .route("/modes", get(modes))
        .route("/setup/status", get(crate::setup_routes::status))
        .route("/setup/check", post(crate::setup_routes::check_settings))
        .route("/setup/config", get(crate::setup_routes::config).put(crate::setup_routes::save))
        .route("/setup/mcp", get(crate::mcp::info))
        .route("/agent/activity", get(crate::activity::recent))
        .route("/openapi.json", get(crate::mcp::openapi))
        .route("/endpoints.md", get(crate::mcp::endpoints))
        .route("/queue", get(queue))
        .route("/llm/settings", get(lr::get_settings).put(lr::put_settings))
        .route("/llm/active", get(lr::active))
        .route("/llm/models", get(lr::models).post(lr::models_for))
        .route("/llm/test", post(lr::test))
        .route("/settings/minimax-h3/schema", get(settings_schema))
        .route("/events", get(events))
        .route("/jobs", get(list_jobs))
        .route("/jobs/{id}", get(get_job))
        .route("/jobs/{id}/log", get(job_log))
        .route("/jobs/{id}/cancel", post(cancel_job))
        .route("/jobs/{id}/retry", post(retry_job))
        .route("/projects", get(list_projects).post(sr::create_project))
        .route("/files", get(sr::serve_file))
        .route("/projects/{pid}", get(get_project).delete(sr::delete_project))
        .route("/projects/{pid}/finals", get(sr::list_finals))
        .route("/projects/{pid}/lyrics", get(sr::get_lyrics).put(sr::put_lyrics))
        .route("/projects/{pid}/audio", axum::routing::put(sr::put_audio))
        .route("/projects/{pid}/audio/waveform", get(sr::get_waveform))
        .route("/projects/{pid}/audio/beats", get(sr::get_beats).put(sr::put_beats))
        .route("/projects/{pid}/timeline/calibrate", post(sr::calibrate))
        .route("/projects/{pid}/references/scene-mapping", get(sr::get_scene_mapping).put(sr::put_scene_mapping))
        .route("/projects/{pid}/references/assign-scenes", post(sr::assign_scenes))
        .route("/projects/{pid}/story/settings", axum::routing::put(sr::story_settings))
        .route("/projects/{pid}/story", get(sr::get_story).put(sr::put_story))
        .route("/projects/{pid}/audio/silent", post(sr::silent_audio))
        .route("/projects/{pid}/duplicate", post(sr::duplicate_project))
        .route("/projects/{pid}/settings/preflight", post(preflight_settings))
        .route("/projects/{pid}/story/{step}", post(crate::story_jobs::story_step))
        .route("/projects/{pid}/references/{kind}/{rid}", axum::routing::put(sr::put_reference).delete(sr::delete_reference))
        .route("/projects/{pid}/scenes/{sid}/prompts/{field}", axum::routing::put(sr::put_prompt))
        .route("/projects/{pid}/scenes/{sid}/prompts/minimax/validate", post(sr::validate_prompt))
        .route("/projects/{pid}/scenes/{sid}/prompts/minimax/assemble", post(sr::assemble_prompt))
        .route("/projects/{pid}/scenes/{sid}/prompts/context", get(sr::prompt_context))
        .route("/projects/{pid}/summary", get(project_summary))
        .route("/projects/{pid}/assets", get(sr::project_assets))
        .route("/projects/{pid}/export", post(sr::export_project))
        .route("/projects/{pid}/settings", get(get_settings).patch(patch_settings))
        .route("/projects/{pid}/scenes", get(list_scenes).post(er::create_scene))
        .route("/projects/{pid}/scenes/bulk", post(sr::bulk_scenes))
        .route("/projects/{pid}/scenes/{sid}", get(get_scene).delete(er::delete_scene).patch(sr::patch_scene))
        .route("/projects/{pid}/scenes/{sid}/split", post(er::split_scene))
        .route("/projects/{pid}/scenes/{sid}/merge", post(er::merge_scene))
        .route("/projects/{pid}/scenes/{sid}/move", post(er::move_scene))
        .route("/projects/{pid}/scenes/{sid}/resize", post(er::resize_scene))
        .route("/projects/{pid}/timeline/bulk", post(er::timeline_bulk))
        .route("/projects/{pid}/timeline/close-gaps", post(er::close_gaps))
        .route("/projects/{pid}/timeline/enforce-length", post(er::enforce_length))
        .route("/projects/{pid}/scenes/{sid}/video/takes", get(crate::video_routes::takes_route))
        .route("/projects/{pid}/scenes/{sid}/video/trim", post(crate::video_routes::trim_route))
        .route("/projects/{pid}/scenes/{sid}/video/match-start-color", post(crate::video_routes::match_color_route))
        .route("/projects/{pid}/scenes/{sid}/video/select", post(crate::video_routes::select_route))
        .route("/projects/{pid}/scenes/{sid}/video/recover", post(crate::video_routes::recover_route))
        .route("/projects/{pid}/scenes/{sid}/video", axum::routing::delete(crate::video_routes::delete_route))
        .route("/projects/{pid}/scenes/{sid}/video/minimax-stage-recover", post(crate::video_routes::stage_recover_route))
        .route("/projects/{pid}/minimax/cleanup", post(crate::video_routes::cleanup_route))
        .route("/projects/{pid}/video/scan", post(crate::video_routes::scan_route))
        .route("/projects/{pid}/pipelines/plan", get(crate::pipeline_jobs::plan_route))
        .route("/projects/{pid}/latents/rebuild", post(crate::pipeline_jobs::rebuild_latents_route))
        .route("/projects/{pid}/pipelines/build-full-video", post(crate::pipeline_jobs::full_video_route))
        .route("/pipelines/from-song", post(crate::pipeline_jobs::from_song_route))
        .route("/projects/{pid}/slideshow", post(crate::video_routes::slideshow_route))
        .route("/projects/{pid}/scenes/{sid}/image/from-video-frame", post(crate::video_routes::from_video_frame_route))
        .route("/projects/{pid}/minimax/index", get(crate::video_routes::index_route))
        .route("/projects/{pid}/timeline/notes", get(crate::notes_routes::list_notes).post(crate::notes_routes::create_note))
        .route("/projects/{pid}/timeline/notes/{nid}", axum::routing::patch(crate::notes_routes::update_note).delete(crate::notes_routes::delete_note))
        .route("/refmods", get(crate::refmod_routes::list).post(crate::render_files::create))
        .route("/projects/{pid}/audio/stems", post(crate::render_files::stems))
        .route("/projects/{pid}/scenes/{sid}/minimax-references", get(crate::reference_routes::get).put(crate::reference_routes::put))
        .route("/post/luts", get(crate::post_routes::list_luts))
        .route("/post/luts/upload", post(crate::post_routes::upload_lut))
        .route("/projects/{pid}/scenes/{sid}/post/lut", post(crate::post_routes::lut_route))
        .route("/projects/{pid}/post/apply-all", post(crate::post_routes::apply_all_route))
        .route("/projects/{pid}/lyrics/align", post(crate::lyric_jobs::align_route))
        .route("/projects/{pid}/timeline/from-lines", post(crate::lyric_jobs::from_lines_route))
        .route("/projects/{pid}/timeline/snap", post(er::snap))
        .route("/projects/{pid}/validate", post(er::validate))
        .route("/projects/{pid}/scenes/{sid}/video/render", post(render_scene))
        .route("/projects/{pid}/minimax-prompts", post(lr::write_prompts))
        .route("/projects/{pid}/scenes/{sid}/prompts/image", post(lr::scene_image_prompt))
        .route("/projects/{pid}/scenes/{sid}/prompts/video", post(lr::scene_video_prompt))
        .route("/projects/{pid}/prompts/batch", post(lr::batch_prompts))
        .route("/instructions", get(ins::list))
        .route("/instructions/{key}", get(ins::get_one).put(ins::save_one))
        .route("/instructions/{key}/override", axum::routing::delete(ins::reset_one))
        .route("/instructions/presets/{name}", axum::routing::put(ins::save_preset))
        .route("/instructions/presets/{name}/load", post(ins::load_preset))
        .route("/projects/{pid}/prompts/concepts", post(lr::concept_prompts))
        .route("/projects/{pid}/speaking-audio/defaults", get(sp::get_defaults).patch(sp::patch_defaults))
        .route("/projects/{pid}/scenes/{sid}/audio-settings", get(sp::get_scene_settings).patch(sp::patch_scene_settings))
        .route("/projects/{pid}/scenes/{sid}/dialogue/craft", post(sp::craft))
        .route("/projects/{pid}/scenes/{sid}/dialogue/generate", post(sp::generate))
        .route("/projects/{pid}/elevenlabs", get(sp::status).put(sp::save_key))
        .route("/projects/{pid}/elevenlabs/voices", get(sp::voices))
        .route("/projects/{pid}/elevenlabs/test", post(sp::test))
        .route("/projects/{pid}/references/subjects/{rid}/voice-design/description", post(sp::voice_description))
        .route("/projects/{pid}/elevenlabs/voice-design/previews", post(sp::voice_previews))
        .route("/projects/{pid}/elevenlabs/voice-design/create", post(sp::voice_create))
        .route("/setup/elevenlabs", get(sp::app_key_status).put(sp::app_key_save))
        .route("/setup/elevenlabs/test", post(sp::app_key_test))
        .route("/projects/{pid}/prompts/motion-notes", post(lr::motion_notes))
        .route("/projects/{pid}/scenes/{sid}/prompts/enhance", post(lr::enhance_scene_prompt))
        .route("/projects/{pid}/scenes/{sid}/prompts/edit", post(lr::edit_scene_prompt))
        .route("/projects/{pid}/references/{kind}/{rid}/prompt", post(lr::reference_image_prompt))
        .route("/projects/{pid}/references/locations/extract", post(lr::extract_locations))
        .route("/projects/{pid}/references/{kind}/{rid}/describe", post(lr::describe_reference))
        .route("/projects/{pid}/video/render", post(render_batch))
        .route("/projects/{pid}/video/graph", post(video_graph))
        .route("/projects/{pid}/stitch", post(stitch))
        .route("/image-presets", get(ir::list_presets))
        .route("/image-presets/import", post(ir::import_preset))
        .route("/image-presets/{id}", get(ir::get_one_preset).put(ir::update_preset).delete(ir::delete_preset))
        .route("/image-presets/{id}/choices", get(ir::preset_choices))
        .route("/image-presets/{id}/graph", post(ir::preset_graph))
        .route("/projects/{pid}/images/generate", post(ir::generate_batch_images))
        .route("/projects/{pid}/scenes/{sid}/image", axum::routing::put(ir::put_scene_image).delete(ir::delete_scene_image))
        .route("/projects/{pid}/scenes/{sid}/image/generate", post(ir::generate_scene_image))
        .route("/projects/{pid}/scenes/{sid}/image/approve", post(ir::approve_scene_image))
        .route("/projects/{pid}/scenes/{sid}/image/revert", post(ir::revert_scene_image))
        .route("/projects/{pid}/scenes/{sid}/image/history", get(ir::scene_image_history))
        .route("/projects/{pid}/references", get(ir::get_references))
        .route("/projects/{pid}/references/{kind}/{rid}/image", axum::routing::put(ir::put_reference_image))
        .route("/projects/{pid}/references/{kind}/{rid}/image/generate", post(ir::generate_reference_image))
        .route("/projects/{pid}/latents", get(latent_list))
        .route("/projects/{pid}/latents/dirty", get(latent_dirty))
        .route("/projects/{pid}/scenes/{sid}/latent", get(latent_one).delete(latent_delete));
    // CORS answers only pages of this computer. `guard` already refused every other origin, so a web page cannot use the API (see `guard.rs`).
    let cors = CorsLayer::new().allow_origin(AllowOrigin::predicate(|origin, _| origin.to_str().is_ok_and(crate::guard::origin_allowed))).allow_methods(Any).allow_headers(Any);
    // The built-in MCP server calls the API above inside this process. It is added last and has no CORS layer: a web page may not use it.
    let mcp = crate::mcp::router(v1.clone().with_state(Arc::clone(&state)), &state);
    Router::new()
        .nest("/api/v1", v1.clone())
        // The pack's front end reaches the agent API under /vrgdg/api/v1.
        .nest("/vrgdg/api/v1", v1)
        .merge(crate::vrgdg::router())
        .fallback(crate::vrgdg::fallback)
        .layer(cors)
        .layer(axum::middleware::from_fn_with_state(Arc::clone(&state), crate::activity::rest_activity))
        .with_state(state)
        .merge(mcp)
        // Outermost, so it covers every route, the ComfyUI pass-through and /mcp.
        .layer(axum::middleware::from_fn(crate::guard::only_this_computer))
}

// -- helpers ----------------------------------------------------------------------------------------------------------------

pub(crate) async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(work).await.map_err(|e| ApiError::internal(e.to_string()))
}

pub(crate) async fn load(state: &AppState, pid: &str) -> Result<LoadedSession, ApiError> {
    let folder = views::resolve_project(&state.roots, pid)?;
    blocking(move || load_session(&folder)).await?.map_err(|e| ApiError::internal(e.to_string()))
}

pub(crate) fn if_match(headers: &HeaderMap) -> Result<Option<i64>, ApiError> {
    match headers.get("if-match").and_then(|v| v.to_str().ok()) {
        None => Ok(None),
        Some(text) => text.trim().trim_matches('"').parse().map(Some).map_err(|_| ApiError::validation("If-Match must be the project revision number.")),
    }
}

pub(crate) fn check_revision(headers: &HeaderMap, current: i64) -> Result<(), ApiError> {
    match if_match(headers)? {
        Some(incoming) if incoming != current => Err(ApiError::revision_conflict(current, incoming)),
        _ => Ok(()),
    }
}

pub(crate) fn body_object(body: Option<Json<Value>>) -> Result<Obj, ApiError> {
    match body.map(|b| b.0) {
        None | Some(Value::Null) => Ok(Obj::new()),
        Some(Value::Object(map)) => Ok(map),
        Some(_) => Err(ApiError::validation("The request body must be a JSON object.")),
    }
}

pub(crate) fn job_response(job: &h3_jobs::Job) -> Response {
    respond(StatusCode::ACCEPTED, json!({"job_id": job.id, "status": job.status, "job": job.to_api()}), None)
}

// -- service ----------------------------------------------------------------------------------------------------------------

async fn health(State(state): Shared) -> ApiResult {
    let client = ComfyClient::new(state.cfg.comfy_base());
    let stats = client.system_stats().await.ok();
    let queue_depth = match &stats {
        Some(_) => {
            let q = client.queue_state().await;
            ["queue_running", "queue_pending"].iter().map(|k| q.get(*k).and_then(Value::as_array).map_or(0, Vec::len)).sum::<usize>()
        }
        None => 0,
    };
    let ffmpeg = Tools::find(&state.cfg.ffmpeg_path, &state.cfg.ffprobe_path).is_ok();
    Ok(success(
        json!({
            "status": if stats.is_some() { "healthy" } else { "degraded" },
            "comfy_up": stats.is_some(),
            "queue_depth": queue_depth,
            "ffmpeg_available": ffmpeg,
            "gpu_available": stats.as_ref().is_some_and(|s| !s.devices.is_empty()),
            "features": features(),
        }),
        None,
    ))
}

/// What this build can and cannot do, so an agent plans with it. The pack's own list has no such thing because the pack has all of it.
fn features() -> Value {
    json!({
        "video_engine": "minimax_h3",
        "story_steps": h3_builder::story::agent::STEPS,
        "scene_numbers": "A scene is named by its id or by its number on the timeline (scene_id \"11\" is Scene 11).",
        "refmod_cards": "A character or location card renders from a saved RefMod with source \"refmod\" and refmod {name, strength} (reference_upsert). A RefMod render sends RefMods and audio only, never scene images.",
        "refmod_create": "POST /refmods makes a RefMod from pictures (tool refmod_create).",
        "audio_stems": "POST /projects/{pid}/audio/stems splits the song and writes a vocal-free track (tool audio_stems_split). Render a scene against it with audio_path in the render params.",
        "not_in_this_app": ["LTX video engine", "face fix", "film grain and color adjust on scenes", "LUT previews", "the Storyboard Builder's own copy of the project (storyboard.json)"],
    })
}

async fn meta() -> ApiResult {
    Ok(success(
        json!({
            "api_version": "v1",
            "app": "vrgdg-h3",
            "app_version": env!("CARGO_PKG_VERSION"),
            "capabilities": [
                "projects", "scenes", "assets", "modes", "models", "settings", "summary", "mutations", "references", "lyrics", "audio", "prompts", "images", "videos", "latents", "post",
                "pipelines", "story", "refmods", "jobs", "events", "queue",
            ],
            "features": features(),
        }),
        None,
    ))
}

async fn models(State(state): Shared) -> ApiResult {
    let client = ComfyClient::new(state.cfg.comfy_base());
    let info = client.object_info().await?;
    let mut groups = serde_json::Map::new();
    for category in ["diffusion_models", "text_encoders", "vae", "loras", "latent_upscale_models", "upscale_models"] {
        groups.insert(category.to_string(), json!(model_choices(&info, category)));
    }
    Ok(success(Value::Object(groups), None))
}

async fn settings_schema() -> ApiResult {
    Ok(success(json!({"settings": h3_core::settings::schema()}), None))
}

/// `GET /modes`: what this app can render. Port of `get_modes_catalog`, with only MiniMax H3 and the image presets.
async fn modes() -> ApiResult {
    let mut keys: Vec<String> = h3_core::settings::defaults().keys().cloned().collect();
    keys.sort();
    Ok(success(
        json!({
            "video_engines": {
                "minimax_h3": {
                    "name": "MiniMax Hailuo H3",
                    "modes": {
                        "image_to_video": {
                            "name": "Image to Video",
                            "scene_inputs": ["approved_image", "audio"],
                            "prompt_fields": ["i2v_prompt"],
                            "settings_group": "minimax_h3",
                            "settings_keys": keys,
                            "supports": {"scene_override": true, "post_process": true, "latent_continuation": true, "audio_drive": true, "continuity": ["off", "i2v_chain"]},
                        },
                        "refmod_pipeline": {
                            "name": "RefMod pipeline",
                            "description": "Set `pipeline` to `refmod` in the MiniMax H3 settings. The whole project then renders from saved RefMods (GET /refmods) picked on Reference Builder cards (`source: refmod`, `refmod: {name, strength}`): one mode (reference_to_video), single or 2 pass, no scene images.",
                            "scene_inputs": ["refmods", "audio"],
                            "render_pass": ["single", "two_pass"],
                            "limits": {"refmods_per_scene": 24},
                            "supports": {"audio_drive": true, "latent_continuation": true},
                        },
                        "reference_to_video": {
                            "name": "Reference to Video",
                            "scene_inputs": ["reference_images", "audio"],
                            "limits": {"reference_images": 9, "video_references": 0},
                            "render_pass": ["single", "two_pass", "three_pass"],
                            "audio_mode": ["input_audio", "built_in_audio"],
                            "supports": {"reference_images": true, "audio_drive": true},
                        },
                    },
                },
            },
            "image_modes": {
                "image_preset": {
                    "name": "ComfyUI image preset",
                    "description": "Scene and reference images are made by an API workflow preset (GET /image-presets) for any image model.",
                    "prompt_fields": ["t2i_prompt"],
                },
            },
            "enums": {
                "continuity_mode": ["off", "i2v_chain", "img2img"],
                "video_engines": ["minimax_h3"],
                "render_passes": ["single", "two_pass", "three_pass"],
                "audio_modes": ["input_audio", "built_in_audio"],
                "seed_modes": ["randomize", "fixed"],
            },
        }),
        None,
    ))
}

async fn queue(State(state): Shared) -> ApiResult {
    let mut summary = state.jobs.queue_summary();
    let q = ComfyClient::new(state.cfg.comfy_base()).queue_state().await;
    summary["comfy_queue_depth"] = json!(["queue_running", "queue_pending"].iter().map(|k| q.get(*k).and_then(Value::as_array).map_or(0, Vec::len)).sum::<usize>());
    Ok(success(summary, None))
}

#[derive(Deserialize)]
struct EventsQuery {
    project_id: Option<String>,
}

async fn events(State(state): Shared, Query(query): Query<EventsQuery>) -> Sse<impl tokio_stream::Stream<Item = Result<SseEvent, Infallible>>> {
    let wanted = query.project_id;
    let stream = BroadcastStream::new(state.jobs.subscribe()).filter_map(move |item| match item {
        Ok(event) if wanted.is_none() || event.project_id.is_none() || event.project_id == wanted => {
            Some(Ok(SseEvent::default().event(event.name).data(event.data.to_string())))
        }
        _ => None,
    });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)).text("ping"))
}

// -- jobs -------------------------------------------------------------------------------------------------------------------

async fn list_jobs(State(state): Shared, Query(q): Query<HashMap<String, String>>) -> ApiResult {
    let jobs = state.jobs.list(q.get("project_id").map(String::as_str), q.get("status").map(String::as_str), q.get("type").map(String::as_str))?;
    Ok(success(Value::Array(jobs.iter().map(h3_jobs::Job::to_api).collect()), None))
}

async fn get_job(State(state): Shared, Path(id): Path<String>) -> ApiResult {
    Ok(success(state.jobs.get(&id)?.to_api(), None))
}

async fn job_log(State(state): Shared, Path(id): Path<String>, Query(q): Query<HashMap<String, String>>) -> ApiResult {
    let since = q.get("since").and_then(|s| s.parse().ok()).unwrap_or(0);
    let logs = state.jobs.logs(&id, since)?;
    Ok(success(json!({"job_id": id, "logs": logs}), None))
}

async fn cancel_job(State(state): Shared, Path(id): Path<String>) -> ApiResult {
    Ok(success(state.jobs.cancel(&id)?.to_api(), None))
}

async fn retry_job(State(state): Shared, Path(id): Path<String>, body: Option<Json<Value>>) -> ApiResult {
    let resume = body_object(body)?.get("resume").and_then(Value::as_bool).unwrap_or(false);
    let job = state.jobs.retry(&id, resume)?;
    Ok(respond(StatusCode::ACCEPTED, job.to_api(), None))
}

// -- projects ---------------------------------------------------------------------------------------------------------------

async fn list_projects(State(state): Shared) -> ApiResult {
    let roots = state.roots.clone();
    Ok(success(Value::Array(blocking(move || views::list_projects(&roots)).await?), None))
}

async fn get_project(State(state): Shared, Path(pid): Path<String>) -> Result<Response, ApiError> {
    let loaded = load(&state, &pid).await?;
    let mut response = success(views::project_detail(&loaded), Some(loaded.revision));
    if let Ok(tag) = HeaderValue::from_str(&format!("\"{}\"", loaded.revision)) {
        response.headers_mut().insert(header::ETAG, tag);
    }
    Ok(response)
}

async fn project_summary(State(state): Shared, Path(pid): Path<String>) -> ApiResult {
    let loaded = load(&state, &pid).await?;
    Ok(success(views::project_summary(&loaded), Some(loaded.revision)))
}

async fn get_settings(State(state): Shared, Path(pid): Path<String>) -> ApiResult {
    let loaded = load(&state, &pid).await?;
    Ok(success(views::settings_view(&loaded.session), Some(loaded.revision)))
}

/// Check the saved settings against the engine the project uses.
async fn preflight_settings(State(state): Shared, Path(pid): Path<String>) -> ApiResult {
    let loaded = load(&state, &pid).await?;
    let effective = views::settings_view(&loaded.session);
    let mut errors: Vec<&str> = Vec::new();
    let engine = effective["project"]["video_engine"].as_str().unwrap_or("minimax_h3").to_string();
    if engine == "minimax_h3" {
        let mm = &effective["minimax_h3"];
        if mm["aspect_ratio"].as_str().is_none_or(|a| a.is_empty()) {
            errors.push("MiniMax aspect_ratio is required.");
        }
        if mm["steps"].as_f64().unwrap_or(0.0) <= 0.0 {
            errors.push("MiniMax steps must be greater than 0.");
        }
    } else if engine == "ltx" && effective["ltx_video"]["fps"].as_f64().unwrap_or(0.0) <= 0.0 {
        errors.push("LTX fps must be greater than 0.");
    }
    Ok(success(json!({"valid": errors.is_empty(), "errors": errors, "warnings": [], "effective_settings": effective}), None))
}

async fn patch_settings(State(state): Shared, Path(pid): Path<String>, headers: HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    let patch = body_object(body)?;
    if patch.is_empty() {
        return Err(ApiError::validation("Send the settings to change, for example {\"minimax_h3\": {\"render_pass\": \"two_pass\"}}."));
    }
    let loaded = load(&state, &pid).await?;
    check_revision(&headers, loaded.revision)?;
    let mut session = loaded.session.clone();
    views::apply_settings_patch(&mut session, &patch)?;
    let folder = loaded.project_folder.clone();
    let saved = blocking(move || persist_session(&folder, session, Stamp::now())).await?.map_err(|e| ApiError::new(StatusCode::CONFLICT, "REVISION_CONFLICT", e.to_string()).retryable())?;
    Ok(success(json!({"settings": views::settings_view(&saved.session), "revision": saved.revision}), Some(saved.revision)))
}

// -- scenes -----------------------------------------------------------------------------------------------------------------

fn flag(q: &HashMap<String, String>, key: &str) -> Option<bool> {
    q.get(key).map(|v| matches!(v.trim().to_lowercase().as_str(), "1" | "true" | "yes"))
}

async fn list_scenes(State(state): Shared, Path(pid): Path<String>, Query(q): Query<HashMap<String, String>>) -> ApiResult {
    let loaded = load(&state, &pid).await?;
    let filter = SceneFilter { has_image: flag(&q, "has_image"), has_video: flag(&q, "has_video"), has_prompt: flag(&q, "has_prompt"), status: q.get("status").cloned() };
    Ok(success(Value::Array(views::scenes_view(&loaded, &filter, false)), Some(loaded.revision)))
}

async fn get_scene(State(state): Shared, Path((pid, sid)): Path<(String, String)>) -> ApiResult {
    let loaded = load(&state, &pid).await?;
    let index = views::find_scene(&loaded, &sid)?;
    let segments = views::segments(&loaded.session);
    Ok(success(views::scene_view(&loaded.project_folder, &segments[index], index, true), Some(loaded.revision)))
}

// -- video ------------------------------------------------------------------------------------------------------------------

async fn render_scene(State(state): Shared, Path((pid, sid)): Path<(String, String)>, body: Option<Json<Value>>) -> ApiResult {
    let mut params = body_object(body)?;
    let loaded = load(&state, &pid).await?;
    views::find_scene(&loaded, &sid)?;
    params.insert("scene_id".into(), json!(sid));
    let job = state.jobs.submit(VIDEO_RENDER, Some(&pid), Value::Object(params), true)?;
    Ok(job_response(&job))
}

async fn render_batch(State(state): Shared, Path(pid): Path<String>, body: Option<Json<Value>>) -> ApiResult {
    let params = body_object(body)?;
    load(&state, &pid).await?;
    let job = state.jobs.submit(VIDEOS_RENDER_BATCH, Some(&pid), Value::Object(params), true)?;
    Ok(job_response(&job))
}

async fn stitch(State(state): Shared, Path(pid): Path<String>, body: Option<Json<Value>>) -> ApiResult {
    const SUPPORTED: [&str; 5] = ["scene_ids", "output_prefix", "audio", "audio_path", "overlays"];
    let payload = body_object(body)?;
    let unknown: Vec<&String> = payload.keys().filter(|k| !SUPPORTED.contains(&k.as_str())).collect();
    if !unknown.is_empty() {
        return Err(ApiError::validation(format!(
            "Unknown stitch field(s): {}. Supported: {}.",
            unknown.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "),
            SUPPORTED.join(", ")
        ))
        .details(json!({"unknown": unknown, "supported": SUPPORTED})));
    }
    let params: Obj = payload.into_iter().filter(|(_, v)| !v.is_null()).collect();
    // Check the scenes, audio and files now so a bad request is a 400 instead of a failed job.
    let loaded = load(&state, &pid).await?;
    let check = params.clone();
    blocking(move || h3_pipeline::stitch_plan::build_stitch_request(&loaded.project_folder, &loaded.session, &check, true).map(|_| ()))
        .await?
        .map_err(ApiError::validation)?;
    let job = state.jobs.submit(VIDEO_STITCH, Some(&pid), Value::Object(params), false)?;
    Ok(job_response(&job))
}

async fn video_graph(State(state): Shared, Path(pid): Path<String>, body: Option<Json<Value>>) -> ApiResult {
    let mut params = body_object(body)?;
    let loaded = load(&state, &pid).await?;
    let scene = match params.remove("scene_id") {
        Some(Value::String(s)) => s,
        Some(Value::Number(n)) => n.to_string(),
        _ => return Err(ApiError::validation("scene_id is required.")),
    };
    views::find_scene(&loaded, &scene)?;
    let run = connect(&state.cfg).await.map_err(|e| ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "COMFY_UNAVAILABLE", e).retryable())?;
    let env = run.env;
    let render = RenderParams { overrides: params.into_iter().filter(|(k, _)| k != "mode").collect(), timeout_hours: None };
    let result = blocking(move || -> Result<Value, h3_pipeline::RenderError> {
        let prepared = prepare_scene_render(&*env, &loaded, &scene, &render)?;
        let built = h3_graph::build(&*env, prepared.workflow_key, &prepared.payload)?;
        Ok(json!({"scene_id": prepared.scene_id, "scene_number": prepared.scene_number, "workflow": prepared.workflow_key, "result": built}))
    })
    .await??;
    Ok(success(result, None))
}

// -- latents ----------------------------------------------------------------------------------------------------------------

async fn latent_list(State(state): Shared, Path(pid): Path<String>) -> ApiResult {
    let loaded = load(&state, &pid).await?;
    let folder = loaded.project_folder.clone();
    let count = views::segments(&loaded.session).len() as u32;
    let items = blocking(move || (1..=count).filter_map(|n| latents::latent_info(&folder, n).ok()).collect::<Vec<_>>()).await?;
    Ok(success(json!({"scenes": items}), Some(loaded.revision)))
}

async fn latent_dirty(State(state): Shared, Path(pid): Path<String>) -> ApiResult {
    let loaded = load(&state, &pid).await?;
    let folder = loaded.project_folder.clone();
    let dirty = blocking(move || latents::list_dirty(&folder)).await?;
    Ok(success(json!({"dirty_scenes": dirty}), Some(loaded.revision)))
}

fn scene_number(loaded: &LoadedSession, sid: &str) -> Result<u32, ApiError> {
    Ok(views::find_scene(loaded, sid)? as u32 + 1)
}

async fn latent_one(State(state): Shared, Path((pid, sid)): Path<(String, String)>) -> ApiResult {
    let loaded = load(&state, &pid).await?;
    let number = scene_number(&loaded, &sid)?;
    let folder = loaded.project_folder.clone();
    let info = blocking(move || latents::latent_info(&folder, number)).await?.map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(success(serde_json::to_value(info).unwrap_or(Value::Null), Some(loaded.revision)))
}

async fn latent_delete(State(state): Shared, Path((pid, sid)): Path<(String, String)>, Query(q): Query<HashMap<String, String>>) -> ApiResult {
    let loaded = load(&state, &pid).await?;
    let number = scene_number(&loaded, &sid)?;
    let folder = loaded.project_folder.clone();
    let all = flag(&q, "all").unwrap_or(false);
    let reindex = flag(&q, "reindex").unwrap_or(false);
    let deleted = blocking(move || if all { latents::delete_all_latents(&folder) } else { usize::from(latents::delete_latent(&folder, number, reindex)) }).await?;
    Ok(success(json!({"deleted": deleted}), Some(loaded.revision)))
}
