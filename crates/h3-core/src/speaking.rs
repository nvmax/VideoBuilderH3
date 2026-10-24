//! Speaking (short film) mode data, without any network: ElevenLabs voice assignments, dialogue drafts, Voice Design drafts and the scene audio settings
//! that place a scene's dialogue on the timeline. Port of `builder/elevenlabs.py`, `builder/elevenlabs_speech.py`, the checks of
//! `builder/elevenlabs_voice_design.py` and `builder/scene_audio_settings.py`.

use base64::engine::general_purpose::GeneralPurposeConfig;
use base64::engine::{DecodePaddingMode, GeneralPurpose};
use base64::{alphabet, Engine};
use fancy_regex::Regex;
use serde_json::{json, Map, Value};

use crate::pyval::{round_to, text_or_empty, to_float, truthy, Obj};
use crate::timeline::shift_segment_timing;

fn strip(value: Option<&Value>) -> String {
    text_or_empty(value).trim().to_string()
}

// -- ElevenLabs keys and voices -------------------------------------------------------------------------------------------------

/// `validate_api_key`: a credential, checked without putting it in an error message.
pub fn validate_api_key(value: &Value, allow_empty: bool) -> Result<String, String> {
    let Value::String(raw) = value else { return Err("ElevenLabs API key must be text.".into()) };
    let key = raw.trim();
    if (key.is_empty() && !allow_empty) || key.chars().count() > 512 || key.chars().any(|c| !(33..=126).contains(&(c as u32))) {
        return Err("Enter a valid ElevenLabs API key in Builder Settings.".into());
    }
    Ok(key.to_string())
}

/// `normalize_voice`: the voice assignment of a character, as the Reference Builder saves it.
pub fn normalize_voice(value: &Value) -> Value {
    let map = value.as_object();
    json!({
        "enabled": map.and_then(|m| m.get("enabled")) == Some(&Value::Bool(true)),
        "voice_id": strip(map.and_then(|m| m.get("voice_id"))),
        "name": strip(map.and_then(|m| m.get("name"))),
    })
}

/// `validate_voice`: refuse an enabled voice without an id and any field that is not part of the assignment.
pub fn validate_voice(value: &Value) -> Result<Value, String> {
    let message = "elevenlabs_voice must contain enabled, voice_id and name only.";
    let Some(map) = value.as_object() else { return Err(message.into()) };
    if map.keys().any(|k| !matches!(k.as_str(), "enabled" | "voice_id" | "name")) {
        return Err(message.into());
    }
    if map.get("enabled").is_some_and(|v| !v.is_boolean()) {
        return Err("elevenlabs_voice.enabled must be a boolean.".into());
    }
    if ["voice_id", "name"].iter().any(|k| map.get(*k).is_some_and(|v| !v.is_string())) {
        return Err("Voice ID and name must be text.".into());
    }
    let voice = normalize_voice(value);
    let voice_id = text_or_empty(voice.get("voice_id"));
    if voice_id.chars().count() > 128 || voice_id.chars().any(|c| !(c.is_ascii_alphanumeric() || c == '_' || c == '-')) {
        return Err("Invalid ElevenLabs voice ID.".into());
    }
    if voice["enabled"] == Value::Bool(true) && voice_id.is_empty() {
        return Err("Choose a voice before enabling ElevenLabs for this character.".into());
    }
    if text_or_empty(voice.get("name")).chars().count() > 256 {
        return Err("Voice name is too long.".into());
    }
    Ok(voice)
}

/// The `detail.status` of an ElevenLabs error body, when the body is small JSON that has one.
pub fn error_status(body: &[u8]) -> Option<String> {
    if body.len() > 8192 {
        return None;
    }
    let data: Value = serde_json::from_slice(body).ok()?;
    data.get("detail")?.as_object()?.get("status")?.as_str().map(str::to_string)
}

/// `_voice_request_error`: what to tell the user about an ElevenLabs error, without exposing the response text or any secret.
pub fn voice_request_error(code: u16, status: Option<&str>, voice_design: bool) -> String {
    if status == Some("missing_permissions") {
        return if voice_design {
            "ElevenLabs reports missing voice-design permission. Enable Voice Generation and Voices write access for this key, then try again.".into()
        } else {
            "ElevenLabs reports missing permission to list voices. In ElevenLabs, open Developers \u{2192} API Keys \u{2192} Edit this key and enable Voices \u{2192} Read (voices_read), then test again. A key used for text to speech may still lack voice-list access.".into()
        };
    }
    if status == Some("invalid_api_key") {
        return "ElevenLabs reports invalid_api_key. Enter the full API key in Builder Settings, rather than the key ID, name or masked value.".into();
    }
    if voice_design {
        return match code {
            401 => "ElevenLabs refused voice-design access (HTTP 401). Check your full key and voice-design permissions.",
            403 => "ElevenLabs denied voice-design access (HTTP 403). Check key permissions and IP restrictions.",
            409 => "ElevenLabs could not save this preview. It may already have been saved; refresh account voices before trying again.",
            422 => "ElevenLabs rejected the voice-design fields. Check the description, preview text and chosen preview.",
            429 => "ElevenLabs rate limit reached. Wait before trying voice design again.",
            _ => "ElevenLabs could not complete voice design. Check your account limits and try again later.",
        }
        .into();
    }
    match code {
        401 => "ElevenLabs refused voice access (HTTP 401). Check this key's Voices \u{2192} Read permission and that the full API key was entered. This response alone does not confirm the key is invalid.",
        403 => "ElevenLabs denied voice access (HTTP 403). Check this key's Voices \u{2192} Read permission and any IP allowlist restrictions.",
        429 => "ElevenLabs rate limit reached. Wait and refresh voices again.",
        _ => "ElevenLabs could not list voices. Try again later.",
    }
    .into()
}

/// The voices of one page of `GET /v2/voices`, and whether there is another page. `page_token` is the token the page was asked with.
pub fn parse_voice_list(data: &Value, page_token: &str) -> Result<Value, String> {
    let invalid = || "ElevenLabs returned an invalid voice list.".to_string();
    let Some(list) = data.get("voices").and_then(Value::as_array) else { return Err(invalid()) };
    let mut voices = Vec::new();
    for item in list {
        let Some(item) = item.as_object() else { continue };
        let Some(id) = item.get("voice_id").filter(|v| truthy(v)) else { continue };
        let id = text_or_empty(Some(id));
        let preview = text_or_empty(item.get("preview_url"));
        let preview = if secure_url(&preview) { preview } else { String::new() };
        let name = item.get("name").filter(|v| truthy(v)).map_or_else(|| id.clone(), |v| text_or_empty(Some(v)));
        voices.push(json!({"voice_id": id, "name": name, "category": text_or_empty(item.get("category")), "preview_url": preview}));
    }
    let token = data.get("next_page_token");
    let has_more = data.get("has_more").is_some_and(truthy);
    let usable = token.and_then(Value::as_str).filter(|t| !t.is_empty() && *t != page_token);
    if has_more && usable.is_none() {
        return Err("ElevenLabs returned an invalid voice page token. Refresh voices again.".into());
    }
    Ok(json!({"voices": voices, "has_more": has_more, "next_page_token": if has_more { token.cloned().unwrap_or_else(|| json!("")) } else { json!("") }}))
}

/// An https URL with a host and no user or password.
fn secure_url(url: &str) -> bool {
    let Some(rest) = url.get(..8).filter(|scheme| scheme.eq_ignore_ascii_case("https://")).map(|_| &url[8..]) else { return false };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    !authority.is_empty() && !authority.contains('@') && !authority.starts_with(':')
}

// -- dialogue drafts ---------------------------------------------------------------------------------------------------------------

/// The longest dialogue each speech model accepts.
fn speech_limit(model: &str) -> Option<usize> {
    match model {
        "eleven_v4" => Some(10000),
        "eleven_v3" => Some(5000),
        _ => None,
    }
}

/// `required_text`: stripped text of a length the provider accepts.
pub fn required_text(value: &Value, label: &str, minimum: usize, maximum: usize) -> Result<String, String> {
    match value {
        Value::String(text) if (minimum..=maximum).contains(&text.trim().chars().count()) => Ok(text.trim().to_string()),
        _ => Err(format!("{label} must be {minimum}\u{2013}{maximum} characters.")),
    }
}

fn clipped(value: Option<&Value>, limit: usize) -> String {
    text_or_empty(value).chars().take(limit).collect()
}

/// `normalize_dialogue`: the editor fields of a scene's dialogue, never generated audio or keys.
pub fn normalize_dialogue(value: &Value) -> Value {
    let map = value.as_object().cloned().unwrap_or_default();
    let model = map.get("model_id").and_then(Value::as_str).filter(|m| speech_limit(m).is_some()).unwrap_or("eleven_v4");
    json!({
        "speaker_id": clipped(map.get("speaker_id"), 256),
        "text": clipped(map.get("text"), speech_limit(model).unwrap_or(10000)),
        "delivery": clipped(map.get("delivery"), 4000),
        "allow_rewrite": map.get("allow_rewrite") == Some(&Value::Bool(true)),
        "model_id": model,
    })
}

/// `validate_dialogue`: an editor draft or a generation request, checked without coercion.
pub fn validate_dialogue(value: &Value, require_text: bool) -> Result<Value, String> {
    let Some(map) = value.as_object() else { return Err("Scene dialogue must be an object.".into()) };
    let model = match map.get("model_id") {
        None => "eleven_v4",
        Some(Value::String(m)) if speech_limit(m).is_some() => m.as_str(),
        _ => return Err("Choose Eleven v4 or Eleven v3 for tagged dialogue.".into()),
    };
    let limit = speech_limit(model).unwrap_or(10000);
    for (key, maximum) in [("speaker_id", 256), ("text", limit), ("delivery", 4000)] {
        let fine = match map.get(key) {
            None => true,
            Some(Value::String(s)) => s.chars().count() <= maximum,
            Some(_) => false,
        };
        if !fine {
            return Err(format!("{key} must be text with at most {maximum} characters."));
        }
    }
    if map.get("allow_rewrite").is_some_and(|v| !v.is_boolean()) {
        return Err("allow_rewrite must be a boolean.".into());
    }
    let mut result = normalize_dialogue(value);
    if require_text {
        let text = required_text(&result["text"], "Dialogue", 1, limit)?;
        result["text"] = json!(text);
    }
    Ok(result)
}

/// `dialogue_speaker`: the primary character whose enabled voice speaks the dialogue.
pub fn dialogue_speaker(references: Option<&Value>, speaker_id: &str) -> Result<Value, String> {
    let subject = references.and_then(|r| r.get("subjects")).and_then(Value::as_array).and_then(|subjects| subjects.iter().find(|s| s.get("id") == Some(&json!(speaker_id))));
    let primary = subject.filter(|s| s.get("reference_type").map_or("character".to_string(), |v| text_or_empty(Some(v))) == "character" && !s.get("extra_reference_for").is_some_and(truthy));
    let Some(subject) = primary else { return Err("Choose a primary character from Reference Builder.".into()) };
    let voice = validate_voice(subject.get("elevenlabs_voice").unwrap_or(&json!({})))?;
    if voice["enabled"] != Value::Bool(true) || voice["voice_id"] == json!("") {
        return Err("Assign and enable an ElevenLabs voice for this character in Reference Builder.".into());
    }
    let mut speaker = voice.as_object().cloned().unwrap_or_default();
    speaker.insert("speaker_id".into(), json!(speaker_id));
    speaker.insert("character_name".into(), subject.get("name").cloned().unwrap_or_else(|| json!("")));
    speaker.insert("character_description".into(), subject.get("description").cloned().unwrap_or_else(|| json!("")));
    Ok(Value::Object(speaker))
}

/// `spoken_words`: the actual words of a dialogue, so audio tags and punctuation can change while the words stay.
pub fn spoken_words(text: &str) -> Vec<String> {
    let tags = Regex::new(r"\[[^\]]*\]").expect("valid pattern");
    let words = Regex::new(r"\w+(?:['\x{2019}]\w+)*").expect("valid pattern");
    let bare = tags.replace_all(text, "");
    words.find_iter(&bare).flatten().map(|m| m.as_str().to_string()).collect()
}

// -- Voice Design ------------------------------------------------------------------------------------------------------------------

const DESIGN_MODELS: [&str; 2] = ["eleven_ttv_v3", "eleven_multilingual_ttv_v2"];
const DESIGN_LIMITS: [(&str, usize); 4] = [("user_input", 4000), ("voice_description", 1000), ("preview_text", 1000), ("voice_name", 256)];

/// `normalize_design_draft`: the editing fields of a Voice Design draft, never preview audio or temporary ids.
pub fn normalize_design_draft(value: &Value) -> Value {
    let map = value.as_object().cloned().unwrap_or_default();
    let mut out = Map::new();
    for (key, limit) in DESIGN_LIMITS {
        out.insert(key.into(), json!(clipped(map.get(key), limit)));
    }
    let model = map.get("model_id").and_then(Value::as_str).filter(|m| DESIGN_MODELS.contains(m)).unwrap_or(DESIGN_MODELS[0]);
    out.insert("model_id".into(), json!(model));
    Value::Object(out)
}

/// `validate_design_draft`: refuse a malformed draft and keep only the editor fields.
pub fn validate_design_draft(value: &Value) -> Result<Value, String> {
    let Some(map) = value.as_object() else { return Err("Voice Design settings must be an object.".into()) };
    for (key, maximum) in DESIGN_LIMITS {
        let fine = match map.get(key) {
            None => true,
            Some(Value::String(s)) => s.chars().count() <= maximum,
            Some(_) => false,
        };
        if !fine {
            return Err(format!("{key} must be text with at most {maximum} characters."));
        }
    }
    if !map.get("model_id").is_none_or(|m| m.as_str().is_some_and(|m| DESIGN_MODELS.contains(&m))) {
        return Err("Choose Voice Design v3 or v2.".into());
    }
    Ok(normalize_design_draft(value))
}

/// The request body of the Voice Design preview call, and the description it was written from.
pub fn design_request(payload: &Value) -> Result<(Value, String, String), String> {
    let description = required_text(payload.get("voice_description").unwrap_or(&Value::Null), "Voice description", 20, 1000)?;
    let model = match payload.get("model_id") {
        None => DESIGN_MODELS[0],
        Some(Value::String(m)) if DESIGN_MODELS.contains(&m.as_str()) => m.as_str(),
        _ => return Err("Choose Voice Design v3 or v2.".into()),
    };
    let text = match payload.get("text") {
        None => String::new(),
        Some(Value::String(t)) => t.clone(),
        Some(_) => return Err("Preview text must be text.".into()),
    };
    let mut body = Map::new();
    body.insert("voice_description".into(), json!(description));
    body.insert("model_id".into(), json!(model));
    body.insert("stream_previews".into(), json!(false));
    body.insert("should_enhance".into(), json!(false));
    if text.trim().is_empty() {
        body.insert("auto_generate_text".into(), json!(true));
    } else {
        body.insert("text".into(), json!(required_text(&json!(text), "Preview text", 100, 1000)?));
        body.insert("auto_generate_text".into(), json!(false));
    }
    Ok((Value::Object(body), description, model.to_string()))
}

fn audio_decoder() -> GeneralPurpose {
    GeneralPurpose::new(&alphabet::STANDARD, GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::RequireCanonical).with_decode_allow_trailing_bits(true))
}

/// The previews of a Voice Design reply, checked one by one.
pub fn design_result(data: &Value, text: &str, description: &str, model: &str) -> Result<Value, String> {
    let candidates = match data.get("previews") {
        Some(Value::Array(list)) if (1..=10).contains(&list.len()) => list,
        _ => return Err("ElevenLabs returned no usable voice previews.".into()),
    };
    let mut previews = Vec::new();
    for candidate in candidates {
        let Some(candidate) = candidate.as_object() else { return Err("ElevenLabs returned an invalid voice preview.".into()) };
        let voice = validate_voice(&json!({"enabled": true, "voice_id": candidate.get("generated_voice_id").cloned().unwrap_or(Value::Null)}))?;
        let audio = candidate.get("audio_base_64").and_then(Value::as_str).filter(|a| a.chars().count() <= 12 * 1024 * 1024);
        let media = candidate.get("media_type").and_then(Value::as_str);
        let (Some(audio), true) = (audio, matches!(media, Some("audio/mpeg" | "audio/mp3"))) else { return Err("ElevenLabs returned an unsupported voice preview.".into()) };
        match audio_decoder().decode(audio) {
            Ok(bytes) if !bytes.is_empty() => {}
            _ => return Err("ElevenLabs returned invalid preview audio.".into()),
        }
        previews.push(json!({"generated_voice_id": voice["voice_id"], "audio_base_64": audio, "media_type": "audio/mpeg"}));
    }
    let shown = data.get("text").filter(|v| truthy(v)).map_or_else(|| text.to_string(), |v| text_or_empty(Some(v)));
    Ok(json!({"previews": previews, "text": shown, "voice_description": description, "model_id": model}))
}

/// The request body of the call that saves a chosen preview as an account voice, with the name and description it uses.
pub fn create_request(payload: &Value) -> Result<(Value, String, String), String> {
    let name = required_text(payload.get("voice_name").unwrap_or(&Value::Null), "Voice name", 1, 256)?;
    let description = required_text(payload.get("voice_description").unwrap_or(&Value::Null), "Voice description", 20, 1000)?;
    let voice = validate_voice(&json!({"enabled": true, "voice_id": payload.get("generated_voice_id").cloned().unwrap_or(Value::Null)}))?;
    Ok((json!({"voice_name": name, "voice_description": description, "generated_voice_id": voice["voice_id"]}), name, description))
}

/// The saved voice of the account, from the reply of the save call.
pub fn create_result(data: &Value, name: &str, description: &str) -> Result<Value, String> {
    let saved = validate_voice(&json!({"enabled": true, "voice_id": data.get("voice_id").cloned().unwrap_or(Value::Null), "name": name}))?;
    Ok(json!({"voice_id": saved["voice_id"], "name": name, "category": "generated", "voice_description": description}))
}

// -- scene audio settings -----------------------------------------------------------------------------------------------------------

/// The settings that place a scene's dialogue: silence around it, and whether the scene grows or shrinks to fit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioSettings {
    pub silence_before: f64,
    pub silence_after: f64,
    pub fit_duration: bool,
}

const DEFAULT_SETTINGS: AudioSettings = AudioSettings { silence_before: 0.0, silence_after: 0.0, fit_duration: true };

/// `validate_settings`: a partial patch, checked without silently coercing bad values. A scene patch may also say whether to use the project defaults.
pub fn validate_settings(value: &Value, scene: bool) -> Result<Obj, String> {
    let keys: &[&str] = if scene { &["use_project_defaults", "silence_before", "silence_after", "fit_duration"] } else { &["silence_before", "silence_after", "fit_duration"] };
    let Some(map) = value.as_object().filter(|m| m.keys().all(|k| keys.contains(&k.as_str()))) else {
        return Err(format!("Audio settings must be an object with keys: {}.", keys.join(", ")));
    };
    let mut result = Obj::new();
    for (key, item) in map {
        if matches!(key.as_str(), "use_project_defaults" | "fit_duration") {
            if !item.is_boolean() {
                return Err(format!("{key} must be a boolean."));
            }
            result.insert(key.clone(), item.clone());
        } else {
            let Some(seconds) = item.as_f64().filter(|_| item.is_number()) else { return Err(format!("{key} must be a number in seconds.")) };
            if !seconds.is_finite() || !(0.0..=60.0).contains(&seconds) {
                return Err(format!("{key} must be between 0 and 60 seconds."));
            }
            result.insert(key.clone(), json!(round_to(seconds, 4)));
        }
    }
    Ok(result)
}

/// `require_speaking`: audio settings exist only in the Speaking video mode.
pub fn require_speaking(session: &Value) -> Result<(), String> {
    if session.get("video_type").and_then(Value::as_str) != Some("speaking") {
        return Err("Scene audio settings are available only in Speaking video mode.".into());
    }
    Ok(())
}

fn settings_from(base: AudioSettings, patch: &Obj) -> AudioSettings {
    AudioSettings {
        silence_before: patch.get("silence_before").and_then(Value::as_f64).unwrap_or(base.silence_before),
        silence_after: patch.get("silence_after").and_then(Value::as_f64).unwrap_or(base.silence_after),
        fit_duration: patch.get("fit_duration").and_then(Value::as_bool).unwrap_or(base.fit_duration),
    }
}

/// `effective_settings`: a scene's own settings, or the project's defaults when it uses them.
pub fn effective_settings(defaults: Option<&Value>, scene: &Value) -> Result<AudioSettings, String> {
    let given = |value: Option<&Value>| value.filter(|v| truthy(v)).cloned().unwrap_or_else(|| json!({}));
    let project = settings_from(DEFAULT_SETTINGS, &validate_settings(&given(defaults), false)?);
    let own = validate_settings(&given(scene.get("scene_audio_settings")), true)?;
    let uses_project = own.get("use_project_defaults").and_then(Value::as_bool).unwrap_or(true);
    Ok(if uses_project { project } else { settings_from(DEFAULT_SETTINGS, &own) })
}

fn key_error(key: &str) -> String {
    format!("'{key}'")
}

/// `float(value)`: with the message Python gives when it cannot.
fn float_of(value: Option<&Value>) -> Result<f64, String> {
    match value {
        Some(Value::Number(n)) => Ok(n.as_f64().unwrap_or(0.0)),
        Some(Value::Bool(b)) => Ok(f64::from(u8::from(*b))),
        Some(Value::String(s)) => to_float(&Value::String(s.clone())).ok_or_else(|| format!("could not convert string to float: '{s}'")),
        Some(Value::Null) | None => Err("float() argument must be a string or a real number, not 'NoneType'".into()),
        Some(other) => Err(format!("float() argument must be a string or a real number, not '{}'", crate::pyval::py_type_name(other))),
    }
}

fn required<'a>(map: &'a Value, key: &str) -> Result<&'a Value, String> {
    map.get(key).ok_or_else(|| key_error(key))
}

fn is_dialogue_clip(clip: &Value) -> bool {
    !clip.get("role").is_some_and(truthy) || clip.get("role") == Some(&json!("dialogue"))
}

/// The indexes of the dialogue clips a scene owns. Independent music stays separate.
fn owned_clips(clips: &[Value], scene_id: &Value) -> Vec<usize> {
    clips.iter().enumerate().filter(|(_, clip)| clip.get("scene_id") == Some(scene_id) && is_dialogue_clip(clip)).map(|(index, _)| index).collect()
}

/// `materialize_scene_clips`: turn the old per scene sources into clips, like the timeline does.
fn materialize_scene_clips(session: &mut Obj) -> Result<(), String> {
    if session.get("audio_clips").is_some_and(Value::is_array) {
        return Ok(());
    }
    let mut clips: Vec<Value> = Vec::new();
    for scene in session.get("segments").and_then(Value::as_array).cloned().unwrap_or_default() {
        if !scene.get("custom_audio_path").is_some_and(truthy) {
            continue;
        }
        let duration = match scene.get("custom_audio_duration").filter(|v| truthy(v)) {
            Some(value) => float_of(Some(value))?,
            None => float_of(scene.get("end"))? - float_of(scene.get("start"))?,
        };
        let name = ["custom_audio_name", "label"].iter().filter_map(|k| scene.get(*k)).find(|v| truthy(v)).cloned().unwrap_or_else(|| json!("Audio"));
        let id = text_or_empty(scene.get("id"));
        let optional = |key: &str| scene.get(key).filter(|v| truthy(v)).map_or(Ok(0.0), |v| float_of(Some(v)));
        clips.push(json!({
            "id": format!("audio_{id}"),
            "scene_id": scene.get("id").cloned().unwrap_or(Value::Null),
            "path": scene["custom_audio_path"],
            "name": name,
            "start": float_of(Some(scene.get("custom_audio_timeline_start").unwrap_or(required(&scene, "start")?)))?,
            "source_start": optional("custom_audio_source_start")?,
            "duration": duration,
            "full_duration": match scene.get("custom_audio_full_duration").filter(|v| truthy(v)) { Some(value) => float_of(Some(value))?, None => duration },
            "peaks": scene.get("custom_audio_peaks").filter(|v| truthy(v)).cloned().unwrap_or_else(|| json!([])),
            "lane": 0, "role": "dialogue", "volume": 1, "muted": false, "include_in_generation": true,
        }));
    }
    let project_audio = session.get("audio_path").filter(|v| truthy(v)).cloned();
    if clips.is_empty() {
        if let Some(path) = project_audio {
            let length = session.get("audio_duration").filter(|v| truthy(v)).map_or(Ok(0.0), |v| float_of(Some(v)))?;
            if length > 0.0 {
                let duration = session.get("audio_duration").cloned().unwrap_or(Value::Null);
                clips.push(json!({
                    "id": "project_audio", "scene_id": "", "path": path, "name": "Project dialogue", "start": 0, "source_start": 0, "duration": duration, "full_duration": duration,
                    "peaks": session.get("audio_peaks").filter(|v| truthy(v)).cloned().unwrap_or_else(|| json!([])),
                    "lane": 0, "role": "dialogue", "volume": 1, "muted": false, "include_in_generation": true,
                }));
            }
        }
    }
    session.insert("audio_clips".into(), Value::Array(clips));
    Ok(())
}

fn number_at(value: &Value, key: &str) -> Result<f64, String> {
    float_of(Some(required(value, key)?))
}

fn set_number(target: &mut Value, key: &str, number: f64) {
    if let Some(map) = target.as_object_mut() {
        map.insert(key.into(), json!(number));
    }
}

/// `apply_scene_timing`: put the dialogue after the opening silence and, when the scene fits its dialogue, move every later scene along.
fn apply_scene_timing(defaults: Option<&Value>, segments: &mut [Value], clips: &mut [Value], scene_index: usize) -> Result<(), String> {
    let scene_id = required(&segments[scene_index], "id")?.clone();
    let owned = owned_clips(clips, &scene_id);
    if owned.is_empty() {
        return Ok(());
    }
    let settings = effective_settings(defaults, &segments[scene_index])?;
    let old_end = number_at(&segments[scene_index], "end")?;
    let scene_start = number_at(&segments[scene_index], "start")?;
    let mut first = f64::INFINITY;
    for index in &owned {
        first = first.min(number_at(&clips[*index], "start")?);
    }
    let offset = scene_start + settings.silence_before - first;
    for index in &owned {
        let moved = round_to(number_at(&clips[*index], "start")? + offset, 4);
        set_number(&mut clips[*index], "start", moved);
    }
    set_number(&mut segments[scene_index], "custom_audio_timeline_start", round_to(scene_start + settings.silence_before, 4));
    if !settings.fit_duration {
        return Ok(());
    }
    let mut last = f64::NEG_INFINITY;
    for index in &owned {
        last = last.max(number_at(&clips[*index], "start")? + number_at(&clips[*index], "duration")?);
    }
    let new_end = last + settings.silence_after;
    let delta = round_to(new_end - old_end, 4);
    set_number(&mut segments[scene_index], "end", round_to(new_end, 4));
    if delta.abs() < 0.0001 {
        return Ok(());
    }
    let mut shifted: Vec<Value> = Vec::new();
    for later in segments.iter_mut() {
        if required(later, "id")? != &scene_id && number_at(later, "start")? >= old_end - 0.0001 {
            shift_segment_timing(later, delta);
            shifted.push(later["id"].clone());
        }
    }
    for clip in clips.iter_mut() {
        if clip.get("scene_id").is_some_and(|id| shifted.contains(id)) {
            let moved = round_to(number_at(clip, "start")? + delta, 4);
            set_number(clip, "start", moved);
        }
    }
    Ok(())
}

fn merged(base: Value, patch: &Obj) -> Value {
    let mut map = base.as_object().cloned().unwrap_or_default();
    map.extend(patch.clone());
    Value::Object(map)
}

fn scene_default_settings() -> Obj {
    json!({"use_project_defaults": true, "silence_before": 0.0, "silence_after": 0.0, "fit_duration": true}).as_object().cloned().unwrap_or_default()
}

/// `update_audio_settings`: an edited copy of the session. Source files, scene ids and file numbers stay. A project edit reaches only the scenes that
/// use the defaults. No scene is added, removed or renamed.
///
/// `attachment` is the metadata of an audio import (an empty object removes the scene's dialogue).
pub fn update_audio_settings(session: &Value, settings: &Value, scene_id: Option<&str>, attachment: Option<&Value>, dialogue: Option<&Value>) -> Result<Value, String> {
    require_speaking(session)?;
    if dialogue.is_some() && scene_id.is_none() {
        return Err("Dialogue drafts require an existing scene.".into());
    }
    let patch = validate_settings(settings, scene_id.is_some())?;
    let running = session.get("segments").and_then(Value::as_array).is_some_and(|all| all.iter().any(|s| s.get("video_status") == Some(&json!("running"))));
    if session.get("timing_frozen").is_some_and(truthy) || running {
        return Err("Unfreeze timing and wait for scene rendering to finish before editing audio.".into());
    }
    let mut result = session.as_object().cloned().unwrap_or_default();
    materialize_scene_clips(&mut result)?;
    let mut segments: Vec<Value> = result.get("segments").filter(|v| truthy(v)).and_then(Value::as_array).cloned().unwrap_or_default();
    let mut clips: Vec<Value> = result.get("audio_clips").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut starts: Vec<(f64, usize)> = Vec::new();
    for (index, scene) in segments.iter().enumerate() {
        starts.push((number_at(scene, "start")?, index));
    }
    starts.sort_by(|a, b| a.0.total_cmp(&b.0));
    let order: Vec<usize> = starts.into_iter().map(|(_, index)| index).collect();
    let mut target: Option<usize> = None;
    if let Some(id) = scene_id {
        for index in &order {
            if required(&segments[*index], "id")? == &json!(id) {
                target = Some(*index);
                break;
            }
        }
        if target.is_none() {
            return Err("Select an existing base scene.".into());
        }
    }
    let mut defaults = result.get("speaking_audio_defaults").cloned();
    let targets: Vec<usize> = if let (Some(index), Some(id)) = (target, scene_id) {
        if let Some(draft) = dialogue {
            let draft = validate_dialogue(draft, false)?;
            if draft["speaker_id"] != json!("") {
                dialogue_speaker(result.get("flux_reference_builder"), &text_or_empty(draft.get("speaker_id")))?;
            }
            segments[index]["scene_dialogue"] = draft;
        }
        let current = segments[index].get("scene_audio_settings").filter(|v| truthy(v)).cloned().unwrap_or_else(|| json!({}));
        segments[index]["scene_audio_settings"] = merged(merged(Value::Object(scene_default_settings()), current.as_object().unwrap_or(&Obj::new())), &patch);
        if let Some(attachment) = attachment {
            let scene_value = json!(id);
            let owned: Vec<Value> = owned_clips(&clips, &scene_value).iter().map(|i| clips[*i].clone()).collect();
            clips.retain(|clip| !owned.contains(clip));
            let start = required(&segments[index], "start")?.clone();
            let get = |key: &str, default: Value| attachment.get(key).cloned().unwrap_or(default);
            let scene = segments[index].as_object_mut().ok_or("Select an existing base scene.")?;
            scene.insert("custom_audio_path".into(), get("saved_path", json!("")));
            scene.insert("custom_audio_name".into(), get("audio_name", json!("")));
            scene.insert("custom_audio_duration".into(), get("duration", json!(0)));
            scene.insert("custom_audio_full_duration".into(), get("duration", json!(0)));
            scene.insert("custom_audio_source_start".into(), json!(0));
            scene.insert("custom_audio_timeline_start".into(), start.clone());
            scene.insert("custom_audio_peaks".into(), get("peaks", json!([])));
            scene.insert("custom_audio_beats".into(), json!([]));
            if truthy(attachment) {
                let name = attachment.get("audio_name").filter(|v| truthy(v)).cloned().unwrap_or_else(|| json!("Scene dialogue"));
                clips.push(json!({
                    "id": format!("audio_{id}"), "scene_id": id, "path": required(attachment, "saved_path")?, "name": name, "start": start, "source_start": 0,
                    "duration": required(attachment, "duration")?, "full_duration": required(attachment, "duration")?, "peaks": get("peaks", json!([])), "lane": 0, "role": "dialogue",
                    "volume": 1, "muted": false, "include_in_generation": true,
                }));
            }
        }
        vec![index]
    } else {
        let current = defaults.clone().filter(truthy).unwrap_or_else(|| json!({}));
        let mut project = Obj::new();
        project.insert("silence_before".into(), json!(0.0));
        project.insert("silence_after".into(), json!(0.0));
        project.insert("fit_duration".into(), json!(true));
        let updated = merged(merged(Value::Object(project), current.as_object().unwrap_or(&Obj::new())), &patch);
        defaults = Some(updated.clone());
        result.insert("speaking_audio_defaults".into(), updated);
        order.iter().copied().filter(|index| segments[*index].get("scene_audio_settings").filter(|v| truthy(v)).and_then(|s| s.get("use_project_defaults")).is_none_or(truthy)).collect()
    };
    let before_segments: Vec<Value> = session.get("segments").and_then(Value::as_array).cloned().unwrap_or_default();
    for index in targets {
        let id = required(&segments[index], "id")?.clone();
        let old = before_segments.iter().find(|s| s.get("id") == Some(&id)).ok_or("Select an existing base scene.")?;
        let before: Vec<Value> = owned_clips(&clips, &id).iter().map(|i| clips[*i].clone()).collect();
        apply_scene_timing(defaults.as_ref(), &mut segments, &mut clips, index)?;
        let after: Vec<Value> = owned_clips(&clips, &id).iter().map(|i| clips[*i].clone()).collect();
        let changed = attachment.is_some() || {
            let span_changed = number_at(&segments[index], "end")? - number_at(&segments[index], "start")? != number_at(old, "end")? - number_at(old, "start")?;
            span_changed || before != after || effective_settings(session.get("speaking_audio_defaults"), old)? != effective_settings(defaults.as_ref(), &segments[index])?
        };
        if changed && (segments[index].get("video_path").is_some_and(truthy) || segments[index].get("video_output").is_some_and(truthy)) {
            segments[index]["scene_audio_render_dirty"] = json!(true);
        }
    }
    if result.contains_key("segments") {
        result.insert("segments".into(), Value::Array(segments));
    }
    result.insert("audio_clips".into(), Value::Array(clips));
    Ok(Value::Object(result))
}

/// `audio_settings_view`: a scene's own settings, the settings that apply to it, and the timing of the dialogue it owns, as the agent API shows them.
pub fn audio_settings_view(session: &Value, scene: &Value) -> Result<Value, String> {
    require_speaking(session)?;
    let mut copy = session.as_object().cloned().unwrap_or_default();
    materialize_scene_clips(&mut copy)?;
    let all: Vec<Value> = copy.get("audio_clips").and_then(Value::as_array).cloned().unwrap_or_default();
    let scene_id = required(scene, "id")?.clone();
    let owned: Vec<Value> = owned_clips(&all, &scene_id).iter().map(|index| all[*index].clone()).collect();
    let mut span = 0.0;
    if !owned.is_empty() {
        let (mut first, mut last) = (f64::INFINITY, f64::NEG_INFINITY);
        for clip in &owned {
            first = first.min(number_at(clip, "start")?);
            last = last.max(number_at(clip, "start")? + number_at(clip, "duration")?);
        }
        span = last - first;
    }
    let effective = effective_settings(copy.get("speaking_audio_defaults"), scene)?;
    let own = scene.get("scene_audio_settings").filter(|v| truthy(v)).and_then(Value::as_object).cloned().unwrap_or_default();
    let defaults = copy.get("speaking_audio_defaults").filter(|v| truthy(v)).and_then(Value::as_object).cloned().unwrap_or_default();
    let project_defaults = merged(json!({"silence_before": 0.0, "silence_after": 0.0, "fit_duration": true}), &defaults);
    Ok(json!({
        "dialogue": normalize_dialogue(scene.get("scene_dialogue").unwrap_or(&Value::Null)),
        "scene_id": scene_id,
        "settings": merged(Value::Object(scene_default_settings()), &own),
        "effective_settings": {"silence_before": effective.silence_before, "silence_after": effective.silence_after, "fit_duration": effective.fit_duration},
        "project_defaults": project_defaults,
        "clips": owned,
        "audio_duration": round_to(span, 4),
        "total_duration": round_to(span + effective.silence_before + effective.silence_after, 4),
        "scene_duration": round_to(number_at(scene, "end")? - number_at(scene, "start")?, 4),
        "needs_render": scene.get("scene_audio_render_dirty").is_some_and(truthy),
    }))
}

/// The project's Speaking defaults as the agent API shows them.
pub fn project_audio_defaults(session: &Value) -> Result<Value, String> {
    require_speaking(session)?;
    let defaults = session.get("speaking_audio_defaults").filter(|v| truthy(v)).and_then(Value::as_object).cloned().unwrap_or_default();
    Ok(merged(json!({"silence_before": 0.0, "silence_after": 0.0, "fit_duration": true}), &defaults))
}
