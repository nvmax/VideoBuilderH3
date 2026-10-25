//! The Speaking (short film) routes of the agent API: scene audio settings, dialogue and ElevenLabs voices. Port of `agent_api/elevenlabs.py`,
//! `scene_audio.py` and `scene_dialogue.py`. The ElevenLabs key is the project's own, else the one saved for every project in App Settings.
//! No key is ever returned or written to a log.

#![allow(clippy::result_large_err)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use h3_builder::elevenlabs;
use h3_builder::prompt_writers::speech::{craft_dialogue, generate_voice_description};
use h3_builder::prompt_writers::RunnerLlm;
use h3_core::pyval::{text_or_empty, truthy, Obj};
use h3_core::speaking::{audio_settings_view, dialogue_speaker, project_audio_defaults, require_speaking, required_text, update_audio_settings, validate_api_key, validate_dialogue, validate_settings};
use h3_jobs::{Handler, JobCtx, JobError, ProgressUpdate};
use h3_project::builder_store::{load_global_llm_keys, save_global_llm_keys};
use h3_project::{load_session, persist_session, LoadedSession, Stamp};
use serde_json::{json, Value};

use crate::app::{blocking, body_object, check_revision, if_match, job_response, load, AppState, Shared};
use crate::error::{respond, success, ApiError, ApiResult};
use crate::views;
use crate::vrgdg::speaking::Network;

pub const CRAFT_DIALOGUE: &str = "elevenlabs.craft_dialogue";
pub const VOICE_DESCRIPTION: &str = "elevenlabs.voice_description";

type Run = fn(Arc<AppState>, JobCtx) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, JobError>> + Send>>;

pub fn register(state: &Arc<AppState>) {
    let table: [(&str, Run); 2] = [(CRAFT_DIALOGUE, |s, c| Box::pin(craft_dialogue_job(s, c))), (VOICE_DESCRIPTION, |s, c| Box::pin(voice_description_job(s, c)))];
    for (kind, run) in table {
        let owned = Arc::clone(state);
        let handler: Handler = Arc::new(move |ctx| run(Arc::clone(&owned), ctx));
        state.jobs.register(kind, handler);
    }
}

fn fail(error: impl Into<ApiError>) -> JobError {
    error.into().into_job_error()
}

fn internal(e: impl std::fmt::Display) -> JobError {
    fail(ApiError::internal(e.to_string()))
}

// -- what the routes share ----------------------------------------------------------------------------------------------------

/// Speaking routes work only in the Speaking video mode.
fn speaking(session: &Obj) -> Result<(), ApiError> {
    require_speaking(&json!({"video_type": session.get("video_type").cloned().unwrap_or(Value::Null)})).map_err(ApiError::validation)
}

/// The key the project saved for itself, else the one saved for every project.
fn elevenlabs_key(state: &AppState, session: &Obj) -> String {
    let own = text_or_empty(session.get("elevenlabs_api_key_project")).trim().to_string();
    if !own.is_empty() {
        return own;
    }
    text_or_empty(load_global_llm_keys(&state.builder_env().output).get("elevenlabs_api_key")).trim().to_string()
}

fn key_source(state: &AppState, session: &Obj) -> &'static str {
    if !text_or_empty(session.get("elevenlabs_api_key_project")).trim().is_empty() {
        "project"
    } else if !text_or_empty(load_global_llm_keys(&state.builder_env().output).get("elevenlabs_api_key")).trim().is_empty() {
        "app"
    } else {
        "none"
    }
}

/// Save the project after an edit. A project that changed meanwhile is a revision conflict.
async fn save(folder: PathBuf, session: Obj) -> Result<i64, ApiError> {
    let saved = blocking(move || persist_session(&folder, session, Stamp::now())).await?.map_err(|e| ApiError::new(StatusCode::CONFLICT, "REVISION_CONFLICT", e.to_string()).retryable())?;
    Ok(saved.revision)
}

fn scene_of<'a>(session: &'a Obj, scene_id: &str) -> Option<&'a Value> {
    session.get("segments").and_then(Value::as_array).and_then(|all| all.iter().find(|s| s.get("id").and_then(Value::as_str) == Some(scene_id)))
}

/// The id of the scene a route was given: an id, or the scene's 1-based number on the timeline like the other scene routes take. Anything else comes back as it was.
fn scene_key(session: &Obj, scene: &str) -> String {
    let wanted = scene.trim();
    if scene_of(session, wanted).is_some() {
        return wanted.to_string();
    }
    let segments = session.get("segments").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    wanted.parse::<usize>().ok().and_then(|n| n.checked_sub(1)).and_then(|i| segments.get(i)).and_then(|s| s.get("id").and_then(Value::as_str)).map_or_else(|| wanted.to_string(), str::to_string)
}

/// The scene's dialogue draft (or the one sent), checked, with the voice of its speaker. Nothing is changed.
fn dialogue_request(session: &Obj, project: &str, scene_id: &str, dialogue: Option<&Value>) -> Result<(Value, Value), ApiError> {
    let scene = scene_of(session, scene_id).ok_or_else(|| ApiError::scene_not_found(scene_id, project))?;
    speaking(session)?;
    let given = dialogue.cloned().unwrap_or_else(|| scene.get("scene_dialogue").cloned().unwrap_or_else(|| json!({})));
    let draft = validate_dialogue(&given, true).map_err(ApiError::validation)?;
    let speaker = dialogue_speaker(session.get("flux_reference_builder"), &text_or_empty(draft.get("speaker_id"))).map_err(ApiError::validation)?;
    Ok((draft, speaker))
}

/// The primary character a voice description is written for.
fn voice_subject<'a>(session: &'a Obj, subject_id: &str) -> Result<&'a Value, ApiError> {
    session
        .get("flux_reference_builder")
        .and_then(|r| r.get("subjects"))
        .and_then(Value::as_array)
        .and_then(|all| all.iter().find(|s| s.get("id").and_then(Value::as_str) == Some(subject_id)))
        .filter(|s| s.get("reference_type").map_or("character".to_string(), |v| text_or_empty(Some(v))) == "character" && !s.get("extra_reference_for").is_some_and(truthy))
        .ok_or_else(|| ApiError::validation("Voice Design requires an existing primary character reference."))
}

fn voice_brief(body: &Obj) -> Result<String, ApiError> {
    required_text(body.get("user_input").unwrap_or(&json!("")), "Voice idea", 1, 4000).map_err(ApiError::validation)
}

async fn read_session(folder: PathBuf) -> Result<LoadedSession, JobError> {
    tokio::task::spawn_blocking(move || load_session(&folder)).await.map_err(internal)?.map_err(internal)
}

// -- ElevenLabs account and voices ---------------------------------------------------------------------------------------------

/// `GET /projects/{pid}/elevenlabs`: whether a key is saved. The key is never returned.
pub async fn status(State(state): Shared, Path(pid): Path<String>) -> ApiResult {
    let loaded = load(&state, &pid).await?;
    speaking(&loaded.session)?;
    let configured = !elevenlabs_key(&state, &loaded.session).is_empty();
    Ok(success(json!({"configured": configured, "source": key_source(&state, &loaded.session), "revision": loaded.revision}), Some(loaded.revision)))
}

/// `PUT /projects/{pid}/elevenlabs`: save (or, with an empty string, clear) the project's own key.
pub async fn save_key(State(state): Shared, Path(pid): Path<String>, headers: HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    let body = body_object(body)?;
    let Some(key) = body.get("api_key").filter(|v| !v.is_null()) else {
        return Err(ApiError::validation("api_key is required; use an empty string to clear it."));
    };
    let key = validate_api_key(key, true).map_err(ApiError::validation)?;
    let folder = views::resolve_project(&state.roots, &pid)?;
    let _guard = state.edit_lock.lock().await;
    let loaded = load(&state, &pid).await?;
    speaking(&loaded.session)?;
    check_revision(&headers, loaded.revision)?;
    let mut session = loaded.session;
    session.insert("elevenlabs_api_key_project".into(), json!(key));
    let revision = save(folder, session.clone()).await?;
    let configured = !elevenlabs_key(&state, &session).is_empty();
    Ok(success(json!({"configured": configured, "source": key_source(&state, &session), "revision": revision}), Some(revision)))
}

/// `GET /projects/{pid}/elevenlabs/voices`: one page of the account's voices.
pub async fn voices(State(state): Shared, Path(pid): Path<String>, Query(query): Query<HashMap<String, String>>) -> ApiResult {
    let loaded = load(&state, &pid).await?;
    speaking(&loaded.session)?;
    let key = elevenlabs_key(&state, &loaded.session);
    let token = query.get("next_page_token").cloned().unwrap_or_default();
    let page = elevenlabs::list_voices(&Network, &json!(key), &json!(token)).await.map_err(ApiError::validation)?;
    Ok(success(page, None))
}

/// `POST /projects/{pid}/elevenlabs/test`: read the voices once to check the key. No speech is made.
pub async fn test(State(state): Shared, Path(pid): Path<String>) -> ApiResult {
    let loaded = load(&state, &pid).await?;
    speaking(&loaded.session)?;
    let key = elevenlabs_key(&state, &loaded.session);
    elevenlabs::list_voices(&Network, &json!(key), &json!("")).await.map_err(ApiError::validation)?;
    Ok(success(json!({"connected": true}), None))
}

/// `POST /projects/{pid}/elevenlabs/voice-design/previews`: voice previews from a reviewed description.
pub async fn voice_previews(State(state): Shared, Path(pid): Path<String>, body: Option<Json<Value>>) -> ApiResult {
    let body = body_object(body)?;
    let loaded = load(&state, &pid).await?;
    speaking(&loaded.session)?;
    let key = elevenlabs_key(&state, &loaded.session);
    let payload = json!({
        "voice_description": body.get("voice_description").cloned().unwrap_or(Value::Null),
        "text": body.get("text").cloned().unwrap_or_else(|| json!("")),
        "model_id": body.get("model_id").cloned().unwrap_or_else(|| json!("eleven_ttv_v3")),
    });
    let previews = elevenlabs::design_voice(&Network, &json!(key), &payload).await.map_err(ApiError::validation)?;
    Ok(success(previews, None))
}

/// `POST /projects/{pid}/elevenlabs/voice-design/create`: keep a chosen preview as a voice of the account.
pub async fn voice_create(State(state): Shared, Path(pid): Path<String>, body: Option<Json<Value>>) -> ApiResult {
    let body = body_object(body)?;
    let loaded = load(&state, &pid).await?;
    speaking(&loaded.session)?;
    let key = elevenlabs_key(&state, &loaded.session);
    let payload = json!({
        "voice_name": body.get("voice_name").cloned().unwrap_or(Value::Null),
        "voice_description": body.get("voice_description").cloned().unwrap_or(Value::Null),
        "generated_voice_id": body.get("generated_voice_id").cloned().unwrap_or(Value::Null),
    });
    let voice = elevenlabs::create_designed_voice(&Network, &json!(key), &payload).await.map_err(ApiError::validation)?;
    Ok(respond(StatusCode::CREATED, json!({"voice": voice}), None))
}

/// `POST /projects/{pid}/references/subjects/{rid}/voice-design/description`: write a voice description for a character, as a job.
pub async fn voice_description(State(state): Shared, Path((pid, rid)): Path<(String, String)>, body: Option<Json<Value>>) -> ApiResult {
    let body = body_object(body)?;
    let loaded = load(&state, &pid).await?;
    speaking(&loaded.session)?;
    let brief = voice_brief(&body)?;
    voice_subject(&loaded.session, &rid)?;
    let job = state.jobs.submit(VOICE_DESCRIPTION, Some(&pid), json!({"subject_id": rid, "user_input": brief}), false)?;
    Ok(job_response(&job))
}

async fn voice_description_job(state: Arc<AppState>, ctx: JobCtx) -> Result<Value, JobError> {
    let pid = ctx.project_id.clone().ok_or_else(|| fail(ApiError::validation("project_id is required.")))?;
    let folder = views::resolve_project(&state.roots, &pid).map_err(fail)?;
    let session = read_session(folder.clone()).await?.session;
    speaking(&session).map_err(fail)?;
    let brief = voice_brief(ctx.params.as_object().unwrap_or(&Obj::new())).map_err(fail)?;
    let subject = voice_subject(&session, &text_or_empty(ctx.params.get("subject_id"))).map_err(fail)?;
    let mut payload = Obj::new();
    payload.insert("user_input".into(), json!(brief));
    payload.insert("character_name".into(), subject.get("name").cloned().unwrap_or_else(|| json!("")));
    payload.insert("character_description".into(), subject.get("description").cloned().unwrap_or_else(|| json!("")));
    ctx.progress(ProgressUpdate { percent: 10.0, stage: "voice_description".into(), message: "Writing a voice description with the project LLM...".into(), stage_count: 1, ..Default::default() });
    let base = crate::vrgdg::llm::base_config(&state);
    let llm = RunnerLlm { payload: &payload, base: &base };
    generate_voice_description(&llm, &payload).await.map_err(|m| fail(ApiError::validation(m)))
}

// -- scene dialogue ----------------------------------------------------------------------------------------------------------

/// `POST /projects/{pid}/scenes/{sid}/dialogue/craft`: tagged dialogue written by the LLM, as a job. Nothing is saved.
pub async fn craft(State(state): Shared, Path((pid, sid)): Path<(String, String)>, body: Option<Json<Value>>) -> ApiResult {
    let body = body_object(body)?;
    let loaded = load(&state, &pid).await?;
    let sid = scene_key(&loaded.session, &sid);
    let (draft, _) = dialogue_request(&loaded.session, &pid, &sid, body.get("dialogue").filter(|v| !v.is_null()))?;
    let job = state.jobs.submit(CRAFT_DIALOGUE, Some(&pid), json!({"scene_id": sid, "dialogue": draft}), false)?;
    Ok(job_response(&job))
}

async fn craft_dialogue_job(state: Arc<AppState>, ctx: JobCtx) -> Result<Value, JobError> {
    let pid = ctx.project_id.clone().ok_or_else(|| fail(ApiError::validation("project_id is required.")))?;
    let folder = views::resolve_project(&state.roots, &pid).map_err(fail)?;
    let session = read_session(folder.clone()).await?.session;
    let scene = text_or_empty(ctx.params.get("scene_id"));
    let (draft, speaker) = dialogue_request(&session, &pid, &scene, ctx.params.get("dialogue").filter(|v| !v.is_null())).map_err(fail)?;
    let mut payload = speaker.as_object().cloned().unwrap_or_default();
    payload.insert("dialogue".into(), draft);
    payload.insert("project_folder".into(), json!(folder.display().to_string()));
    payload.insert("scene_id".into(), json!(scene));
    ctx.progress(ProgressUpdate { percent: 10.0, stage: "dialogue".into(), message: "Preparing dialogue with the project LLM...".into(), scene_id: Some(scene), stage_count: 1, ..Default::default() });
    let base = crate::vrgdg::llm::base_config(&state);
    let llm = RunnerLlm { payload: &payload, base: &base };
    craft_dialogue(&llm, &payload).await.map_err(|m| fail(ApiError::validation(m)))
}

/// `POST /projects/{pid}/scenes/{sid}/dialogue/generate`: one MP3 take with the character's voice. Nothing is imported.
pub async fn generate(State(state): Shared, Path((pid, sid)): Path<(String, String)>, body: Option<Json<Value>>) -> ApiResult {
    let body = body_object(body)?;
    let loaded = load(&state, &pid).await?;
    let sid = scene_key(&loaded.session, &sid);
    let (draft, speaker) = dialogue_request(&loaded.session, &pid, &sid, body.get("dialogue").filter(|v| !v.is_null()))?;
    let key = elevenlabs_key(&state, &loaded.session);
    let take = elevenlabs::generate_speech(&Network, &json!(key), &text_or_empty(speaker.get("voice_id")), &draft).await.map_err(ApiError::validation)?;
    Ok(success(take, None))
}

// -- scene audio settings ---------------------------------------------------------------------------------------------------

/// `GET /projects/{pid}/speaking-audio/defaults`
pub async fn get_defaults(State(state): Shared, Path(pid): Path<String>) -> ApiResult {
    let loaded = load(&state, &pid).await?;
    let view = project_audio_defaults(&Value::Object(loaded.session)).map_err(ApiError::validation)?;
    Ok(success(view, Some(loaded.revision)))
}

/// `GET /projects/{pid}/scenes/{sid}/audio-settings`
pub async fn get_scene_settings(State(state): Shared, Path((pid, sid)): Path<(String, String)>) -> ApiResult {
    let loaded = load(&state, &pid).await?;
    let sid = scene_key(&loaded.session, &sid);
    let scene = scene_of(&loaded.session, &sid).cloned().ok_or_else(|| ApiError::scene_not_found(&sid, &pid))?;
    let view = audio_settings_view(&Value::Object(loaded.session), &scene).map_err(ApiError::validation)?;
    Ok(success(view, Some(loaded.revision)))
}

/// `PATCH /projects/{pid}/speaking-audio/defaults`
pub async fn patch_defaults(State(state): Shared, Path(pid): Path<String>, headers: HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    patch_audio(&state, pid, None, &headers, body_object(body)?).await
}

/// `PATCH /projects/{pid}/scenes/{sid}/audio-settings`
pub async fn patch_scene_settings(State(state): Shared, Path((pid, sid)): Path<(String, String)>, headers: HeaderMap, body: Option<Json<Value>>) -> ApiResult {
    patch_audio(&state, pid, Some(sid), &headers, body_object(body)?).await
}

/// Save defaults or a scene's overrides, with the dialogue clip imported or removed on request. Port of `patch_audio_settings`.
async fn patch_audio(state: &Arc<AppState>, pid: String, scene_id: Option<String>, headers: &HeaderMap, payload: Obj) -> ApiResult {
    let folder = views::resolve_project(&state.roots, &pid)?;
    let expected = if_match(headers)?;
    let env = state.builder_env();
    let _guard = state.edit_lock.lock().await;
    let (view, revision) = blocking(move || -> Result<(Value, i64), ApiError> {
        let loaded = load_session(&folder).map_err(|e| ApiError::internal(e.to_string()))?;
        if let Some(incoming) = expected.filter(|i| *i != loaded.revision) {
            return Err(ApiError::revision_conflict(loaded.revision, incoming));
        }
        let scene_id = scene_id.map(|id| scene_key(&loaded.session, &id));
        let session = Value::Object(loaded.session.clone());
        let segments = loaded.session.get("segments").and_then(Value::as_array).cloned().unwrap_or_default();
        let scene_number = scene_id.as_ref().map(|id| segments.iter().position(|s| s.get("id").and_then(Value::as_str) == Some(id.as_str())).ok_or_else(|| ApiError::scene_not_found(id, &pid))).transpose()?.map(|i| i + 1);
        let running = segments.iter().any(|s| s.get("video_status") == Some(&json!("running")));
        if loaded.session.get("timing_frozen").is_some_and(truthy) || running {
            return Err(ApiError::validation("Unfreeze timing and wait for scene rendering to finish before editing audio."));
        }
        speaking(&loaded.session)?;
        let settings = payload.get("settings").cloned().unwrap_or_else(|| json!({}));
        validate_settings(&settings, scene_id.is_some()).map_err(ApiError::validation)?;
        let dialogue = payload.get("dialogue").filter(|v| !v.is_null());
        let draft = match dialogue {
            Some(value) => {
                if scene_id.is_none() {
                    return Err(ApiError::validation("Dialogue drafts require an existing scene."));
                }
                let checked = validate_dialogue(value, false).map_err(ApiError::validation)?;
                if !text_or_empty(checked.get("speaker_id")).is_empty() {
                    dialogue_speaker(loaded.session.get("flux_reference_builder"), &text_or_empty(checked.get("speaker_id"))).map_err(ApiError::validation)?;
                }
                Some(checked)
            }
            None => None,
        };
        let clear = match payload.get("clear_audio") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(flag)) => *flag,
            Some(_) => return Err(ApiError::validation("clear_audio must be a boolean.")),
        };
        let audio_data = payload.get("audio_data").filter(|v| !v.is_null());
        if (clear || audio_data.is_some()) && scene_id.is_none() {
            return Err(ApiError::validation("Audio import/removal requires an existing scene."));
        }
        if audio_data.is_some() && clear {
            return Err(ApiError::validation("Choose either audio_data or clear_audio."));
        }
        let mut attachment = if clear { Some(json!({})) } else { None };
        if let Some(data) = audio_data {
            let (Some(text), Some(number)) = (data.as_str().filter(|t| !t.trim().is_empty()), scene_number) else {
                return Err(ApiError::validation("Non-empty audio_data and an existing scene are required."));
            };
            let name = Some(text_or_empty(payload.get("audio_name"))).filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "scene_audio.wav".into());
            let request: Obj = [
                ("project_folder".to_string(), json!(loaded.project_folder.display().to_string())),
                ("scene_number".into(), json!(number)),
                ("audio_data".into(), json!(text)),
                ("audio_name".into(), json!(name)),
                ("preserve_source".into(), json!(true)),
            ]
            .into_iter()
            .collect();
            let mut saved = h3_builder::assets::save_scene_audio(&env, &request).map_err(ApiError::validation)?;
            saved["audio_name"] = json!(name);
            if saved.get("duration").and_then(Value::as_f64).unwrap_or(0.0) <= 0.0 {
                return Err(ApiError::validation("The imported audio has no usable duration."));
            }
            attachment = Some(saved);
        }
        let updated = update_audio_settings(&session, &settings, scene_id.as_deref(), attachment.as_ref(), draft.as_ref()).map_err(ApiError::validation)?;
        let updated_map = updated.as_object().cloned().unwrap_or_default();
        let saved = persist_session(&loaded.project_folder, updated_map, Stamp::now()).map_err(|e| ApiError::new(StatusCode::CONFLICT, "REVISION_CONFLICT", e.to_string()).retryable())?;
        let view = match &scene_id {
            Some(id) => {
                let scene = scene_of(&saved.session, id).cloned().ok_or_else(|| ApiError::scene_not_found(id, &pid))?;
                audio_settings_view(&Value::Object(saved.session.clone()), &scene).map_err(ApiError::validation)?
            }
            None => saved.session.get("speaking_audio_defaults").cloned().unwrap_or_else(|| json!({})),
        };
        Ok((view, saved.revision))
    })
    .await??;
    Ok(success(view, Some(revision)))
}

// -- the key for every project (App Settings) ---------------------------------------------------------------------------------

/// `GET /setup/elevenlabs`: whether a key is saved for every project. The key is never returned.
pub async fn app_key_status(State(state): Shared) -> ApiResult {
    let configured = !text_or_empty(load_global_llm_keys(&state.builder_env().output).get("elevenlabs_api_key")).trim().is_empty();
    Ok(success(json!({"configured": configured}), None))
}

/// `PUT /setup/elevenlabs`: save (or, with an empty string, clear) the key every project uses unless it has its own.
pub async fn app_key_save(State(state): Shared, body: Option<Json<Value>>) -> ApiResult {
    let body = body_object(body)?;
    let Some(key) = body.get("api_key").filter(|v| !v.is_null()) else {
        return Err(ApiError::validation("api_key is required; use an empty string to clear it."));
    };
    let key = validate_api_key(key, true).map_err(ApiError::validation)?;
    let output = state.builder_env().output;
    blocking(move || save_global_llm_keys(&output, Some(&json!({"elevenlabs_api_key": key})))).await?.map_err(|e| ApiError::internal(format!("The key could not be saved: {e}")))?;
    app_key_status(State(state)).await
}

/// `POST /setup/elevenlabs/test`: read the voices once with the saved key, or with `api_key` when one is sent.
pub async fn app_key_test(State(state): Shared, body: Option<Json<Value>>) -> ApiResult {
    let body = body_object(body)?;
    let sent = text_or_empty(body.get("api_key")).trim().to_string();
    let key = if sent.is_empty() { text_or_empty(load_global_llm_keys(&state.builder_env().output).get("elevenlabs_api_key")).trim().to_string() } else { sent };
    if key.is_empty() {
        return Err(ApiError::validation("No ElevenLabs key is saved. Paste one and try again."));
    }
    elevenlabs::list_voices(&Network, &json!(key), &json!("")).await.map_err(ApiError::validation)?;
    Ok(success(json!({"connected": true}), None))
}
