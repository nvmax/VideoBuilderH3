//! The ElevenLabs calls of the Speaking mode: the account's voices, Voice Design previews, saving a chosen voice, and one speech take. Port of
//! `builder/elevenlabs.py`, `builder/elevenlabs_speech.py` and `builder/elevenlabs_voice_design.py`.
//!
//! The network is a [`Transport`], so the checks and the error texts are tested without ElevenLabs. Nothing here retries a call that may have cost credits
//! or changed the account, and no error text carries a key or a response body.

use std::future::Future;
use std::pin::Pin;

use base64::Engine;
use h3_core::pyval::text_or_empty;
use h3_core::speaking::{create_request, create_result, design_request, design_result, error_status, parse_voice_list, validate_api_key, validate_dialogue, validate_voice, voice_request_error};
use serde_json::{json, Value};

const HOST: &str = "https://api.elevenlabs.io";
const MEGABYTE: usize = 1024 * 1024;

/// One call to ElevenLabs.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub method: &'static str,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    pub timeout_seconds: u64,
}

/// What came back. `body` holds at most one byte more than the limit the call allowed, which is enough to know the reply was too big.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub body: Vec<u8>,
    pub content_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Failure {
    /// ElevenLabs answered with an error status. `body` holds at most the first 8193 bytes.
    Status { code: u16, body: Vec<u8> },
    /// No answer: the connection failed or timed out.
    Connect,
}

pub type SendFuture<'a> = Pin<Box<dyn Future<Output = Result<Reply, Failure>> + Send + 'a>>;

pub trait Transport: Sync {
    /// Send the request and read at most `limit + 1` bytes of the reply.
    fn send(&self, request: Request, limit: usize) -> SendFuture<'_>;
}

/// Python's `urllib.parse.quote_plus`.
fn quote_plus(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' => out.push(byte as char),
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn headers(key: &str, accept: &str, with_json_body: bool) -> Vec<(String, String)> {
    let mut list = vec![("xi-api-key".to_string(), key.to_string())];
    if with_json_body {
        list.push(("Content-Type".into(), "application/json".into()));
    }
    list.push(("Accept".into(), accept.into()));
    list
}

/// `list_voices`: one page of the account's voices. Never exposes an upstream error body.
pub async fn list_voices(http: &dyn Transport, api_key: &Value, next_page_token: &Value) -> Result<Value, String> {
    let key = validate_api_key(api_key, false)?;
    let token = match next_page_token {
        Value::String(token) if token.chars().count() <= 2048 => token.as_str(),
        _ => return Err("Invalid voice page token.".into()),
    };
    let mut query = "page_size=100&include_total_count=false&sort=name".to_string();
    if !token.is_empty() {
        query.push_str(&format!("&next_page_token={}", quote_plus(token)));
    }
    let request = Request { method: "GET", url: format!("{HOST}/v2/voices?{query}"), headers: headers(&key, "application/json", false), body: None, timeout_seconds: 25 };
    let reply = match http.send(request, 4 * MEGABYTE).await {
        Ok(reply) => reply,
        Err(Failure::Status { code, body }) => return Err(voice_request_error(code, error_status(&body).as_deref(), false)),
        Err(Failure::Connect) => return Err("Could not connect to ElevenLabs. Check your connection and try again.".into()),
    };
    if reply.body.len() > 4 * MEGABYTE {
        return Err("ElevenLabs returned an oversized voice list.".into());
    }
    let data: Value = serde_json::from_slice(&reply.body).map_err(|_| "ElevenLabs returned an invalid voice list.".to_string())?;
    parse_voice_list(&data, token)
}

/// `generate_speech`: one MP3 take of the dialogue. No retry, no timeline change, no project write.
pub async fn generate_speech(http: &dyn Transport, api_key: &Value, voice_id: &str, dialogue: &Value) -> Result<Value, String> {
    let draft = validate_dialogue(dialogue, true)?;
    let voice = validate_voice(&json!({"enabled": true, "voice_id": voice_id}))?;
    let key = validate_api_key(api_key, false)?;
    let voice_id = text_or_empty(voice.get("voice_id"));
    let body = json!({"text": draft["text"], "model_id": draft["model_id"]});
    let request = Request {
        method: "POST",
        url: format!("{HOST}/v1/text-to-speech/{voice_id}?output_format=mp3_44100_128"),
        headers: headers(&key, "audio/mpeg", true),
        body: Some(body.to_string().into_bytes()),
        timeout_seconds: 180,
    };
    let reply = match http.send(request, 32 * MEGABYTE).await {
        Ok(reply) => reply,
        Err(Failure::Status { code, .. }) => {
            return Err(match code {
                401 => "ElevenLabs refused speech generation. Check the full key and Text to Speech permission.",
                403 => "ElevenLabs denied speech generation. Check Text to Speech access and key IP restrictions.",
                404 => "This voice or speech model is unavailable. Refresh voices or choose another model.",
                422 => "ElevenLabs rejected this speech request. Check the dialogue, voice and model.",
                429 => "ElevenLabs rate limit reached. Wait before generating another take.",
                _ => "ElevenLabs could not generate speech. Check your account credits and model access.",
            }
            .into());
        }
        Err(Failure::Connect) => return Err("Speech generation did not complete. Check your connection. The request may have used credits; it was not retried.".into()),
    };
    let media = reply.content_type.as_deref().unwrap_or("audio/mpeg").split(';').next().unwrap_or("");
    if reply.body.is_empty() || reply.body.len() > 32 * MEGABYTE || !matches!(media, "audio/mpeg" | "audio/mp3" | "application/octet-stream") {
        return Err("ElevenLabs returned empty, oversized or unsupported speech audio.".into());
    }
    Ok(json!({
        "audio_data": format!("data:audio/mpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(&reply.body)),
        "audio_name": "elevenlabs_dialogue.mp3",
        "voice_id": voice["voice_id"],
        "dialogue": draft,
    }))
}

const MAY_EXIST: &str = " The voice may already have been created. Refresh account voices before trying Save again.";

/// `_post_voice_design`: one explicit request. An account change is never retried.
async fn post_voice_design(http: &dyn Transport, api_key: &Value, path: &str, body: &Value, saving: bool) -> Result<Value, String> {
    let key = validate_api_key(api_key, false)?;
    let request = Request { method: "POST", url: format!("{HOST}{path}"), headers: headers(&key, "application/json", true), body: Some(body.to_string().into_bytes()), timeout_seconds: 120 };
    let reply = match http.send(request, 32 * MEGABYTE).await {
        Ok(reply) => reply,
        Err(Failure::Status { code, body }) => {
            let mut message = voice_request_error(code, error_status(&body).as_deref(), true);
            if saving && (code >= 500 || code == 408) {
                message.push_str(MAY_EXIST);
            }
            return Err(message);
        }
        Err(Failure::Connect) => {
            let mut message = "Could not complete the ElevenLabs voice-design request. Check your connection.".to_string();
            if saving {
                message.push_str(MAY_EXIST);
            }
            return Err(message);
        }
    };
    if reply.body.len() > 32 * MEGABYTE {
        return Err("ElevenLabs returned an oversized voice-design response.".into());
    }
    let data: Value = serde_json::from_slice(&reply.body).map_err(|_| "ElevenLabs returned an invalid voice-design response. Refresh account voices if you were saving.".to_string())?;
    if !data.is_object() {
        return Err("ElevenLabs returned an invalid voice-design response.".into());
    }
    Ok(data)
}

/// `design_voice`: preview candidates from a reviewed description, without saving a voice to the account.
pub async fn design_voice(http: &dyn Transport, api_key: &Value, payload: &Value) -> Result<Value, String> {
    let (body, description, model) = design_request(payload)?;
    let data = post_voice_design(http, api_key, "/v1/text-to-voice/design", &body, false).await?;
    let sent_text = payload.get("text").and_then(Value::as_str).unwrap_or("");
    design_result(&data, sent_text, &description, &model)
}

/// `create_designed_voice`: save one chosen preview to the account. Only ever on the user's explicit request.
pub async fn create_designed_voice(http: &dyn Transport, api_key: &Value, payload: &Value) -> Result<Value, String> {
    let (body, name, description) = create_request(payload)?;
    let data = post_voice_design(http, api_key, "/v1/text-to-voice", &body, true).await?;
    create_result(&data, &name, &description)
}
