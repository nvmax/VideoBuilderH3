//! Speaking (short film) routes of the Builder: ElevenLabs voices and speech, the dialogue and voice description helpers, and the scene audio settings
//! preview. They answer like the pack's: 400 for a request that cannot be done, and no key or response text in any error.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::post as axum_post;
use axum::Router;
use h3_builder::elevenlabs::{self, Failure, Reply, Request, SendFuture, Transport};
use h3_builder::prompt_writers::speech::{craft_dialogue, generate_voice_description};
use h3_builder::prompt_writers::RunnerLlm;
use h3_core::pyval::{text_or_empty, Obj};
use h3_core::speaking::{dialogue_speaker, require_speaking, update_audio_settings, validate_dialogue};
use serde_json::{json, Value};

use super::llm::base_config;
use super::{body_object, into_response, Fail, Out};
use crate::app::AppState;

/// The real network, for the ElevenLabs calls.
struct Network;

impl Transport for Network {
    fn send(&self, request: Request, limit: usize) -> SendFuture<'_> {
        Box::pin(async move {
            let client = reqwest::Client::builder().timeout(Duration::from_secs(request.timeout_seconds)).build().map_err(|_| Failure::Connect)?;
            let mut call = if request.method == "GET" { client.get(&request.url) } else { client.post(&request.url) };
            for (name, value) in &request.headers {
                call = call.header(name.as_str(), value.as_str());
            }
            if let Some(body) = request.body {
                call = call.body(body);
            }
            let mut response = call.send().await.map_err(|_| Failure::Connect)?;
            let code = response.status().as_u16();
            let content_type = response.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(str::to_string);
            // An error body is only read for its status code. A good one is read up to the limit, plus the byte that shows it was too big.
            let wanted = if code >= 400 { 8193 } else { limit + 1 };
            let mut body: Vec<u8> = Vec::new();
            while body.len() < wanted {
                match response.chunk().await {
                    Ok(Some(chunk)) => body.extend_from_slice(&chunk),
                    Ok(None) => break,
                    Err(_) if code >= 400 => break,
                    Err(_) => return Err(Failure::Connect),
                }
            }
            body.truncate(wanted);
            if code >= 400 { Err(Failure::Status { code, body }) } else { Ok(Reply { body, content_type }) }
        })
    }
}

fn done(result: Result<Value, String>) -> Response {
    into_response(result.map(Out::from).map_err(|message| Fail::new(StatusCode::BAD_REQUEST, message)))
}

/// Error text of an LLM helper without any of the request's keys.
fn redacted(mut message: String, payload: &Obj) -> String {
    for field in ["llm_api_key", "llm_api_key_project", "own_server_api_key", "own_server_api_key_project", "lmstudio_api_key"] {
        let secret = text_or_empty(payload.get(field));
        if !secret.is_empty() {
            message = message.replace(&secret, "[redacted]");
        }
    }
    message
}

/// Speaking routes work only in the Speaking video mode.
fn speaking_only(payload: &Obj) -> Result<(), String> {
    require_speaking(&json!({"video_type": payload.get("video_type").cloned().unwrap_or(Value::Null)}))
}

fn key_of(payload: &Obj) -> Value {
    payload.get("api_key").cloned().unwrap_or_else(|| json!(""))
}

async fn craft_scene_dialogue(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let payload = match body_object(&body) {
        Ok(payload) => payload,
        Err(fail) => return into_response(Err(fail)),
    };
    if let Err(message) = speaking_only(&payload) {
        return done(Err(message));
    }
    let base = base_config(&state);
    let llm = RunnerLlm { payload: &payload, base: &base };
    done(craft_dialogue(&llm, &payload).await.map_err(|message| redacted(message, &payload)))
}

async fn elevenlabs_voice_description(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let payload = match body_object(&body) {
        Ok(payload) => payload,
        Err(fail) => return into_response(Err(fail)),
    };
    if let Err(message) = speaking_only(&payload) {
        return done(Err(message));
    }
    let base = base_config(&state);
    let llm = RunnerLlm { payload: &payload, base: &base };
    done(generate_voice_description(&llm, &payload).await.map_err(|message| redacted(message, &payload)))
}

async fn generate_scene_speech(body: Bytes) -> Response {
    let payload = match body_object(&body) {
        Ok(payload) => payload,
        Err(fail) => return into_response(Err(fail)),
    };
    let result = async {
        speaking_only(&payload)?;
        let draft = validate_dialogue(payload.get("dialogue").unwrap_or(&Value::Null), true)?;
        let speaker = dialogue_speaker(payload.get("references"), &text_or_empty(draft.get("speaker_id")))?;
        elevenlabs::generate_speech(&Network, &key_of(&payload), &text_or_empty(speaker.get("voice_id")), &draft).await
    };
    done(result.await)
}

async fn elevenlabs_voices(body: Bytes) -> Response {
    let payload = match body_object(&body) {
        Ok(payload) => payload,
        Err(fail) => return into_response(Err(fail)),
    };
    let result = async {
        speaking_only(&payload)?;
        let token = payload.get("next_page_token").cloned().unwrap_or_else(|| json!(""));
        elevenlabs::list_voices(&Network, &key_of(&payload), &token).await
    };
    done(result.await)
}

async fn elevenlabs_voice_design(body: Bytes) -> Response {
    let payload = match body_object(&body) {
        Ok(payload) => payload,
        Err(fail) => return into_response(Err(fail)),
    };
    let result = async {
        speaking_only(&payload)?;
        elevenlabs::design_voice(&Network, &key_of(&payload), &Value::Object(payload.clone())).await
    };
    done(result.await)
}

async fn elevenlabs_voice_create(body: Bytes) -> Response {
    let payload = match body_object(&body) {
        Ok(payload) => payload,
        Err(fail) => return into_response(Err(fail)),
    };
    let result = async {
        speaking_only(&payload)?;
        elevenlabs::create_designed_voice(&Network, &key_of(&payload), &Value::Object(payload.clone())).await
    };
    done(result.await.map(|voice| json!({"voice": voice})))
}

async fn preview_scene_audio_settings(body: Bytes) -> Response {
    let payload = match body_object(&body) {
        Ok(payload) => payload,
        Err(fail) => return into_response(Err(fail)),
    };
    let result = tokio::task::spawn_blocking(move || {
        let session = payload.get("session").ok_or_else(|| "'session'".to_string())?;
        let settings = payload.get("settings").cloned().unwrap_or_else(|| json!({}));
        let present = |key: &str| payload.get(key).filter(|v| !v.is_null());
        let edited = update_audio_settings(session, &settings, present("scene_id").and_then(Value::as_str), present("attachment"), present("dialogue"))?;
        Ok::<Value, String>(json!({"session": edited}))
    })
    .await
    .unwrap_or_else(|_| Err("Could not preview the audio settings.".into()));
    done(result)
}

pub fn routes(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    let base = "/vrgdg/music_builder";
    router
        .route(&format!("{base}/craft_scene_dialogue"), axum_post(craft_scene_dialogue))
        .route(&format!("{base}/elevenlabs_voice_description"), axum_post(elevenlabs_voice_description))
        .route(&format!("{base}/generate_scene_speech"), axum_post(generate_scene_speech))
        .route(&format!("{base}/elevenlabs_voices"), axum_post(elevenlabs_voices))
        .route(&format!("{base}/elevenlabs_voice_design"), axum_post(elevenlabs_voice_design))
        .route(&format!("{base}/elevenlabs_voice_create"), axum_post(elevenlabs_voice_create))
        .route(&format!("{base}/preview_scene_audio_settings"), axum_post(preview_scene_audio_settings))
}
