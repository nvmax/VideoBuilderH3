//! Edit one picture through a hosted image API: Gemini, Grok (xAI) or OpenAI. Port of the pack's `llm/api_image.py`.
//!
//! The Thumbnail dialog's "API image" engine uses this. The provider and the key are the ones set in the LLM Runner, the image model is
//! chosen next to the engine. The key travels in a header, never in the URL, so it cannot end up in an error message. These models are kept
//! apart from the chat models in `providers.rs` on purpose. Requests go out through `tls.rs`, like every hosted API.

use std::fmt;
use std::io::Cursor;
use std::time::Duration;

use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use base64::Engine;
use image::imageops::{self, FilterType};
use image::{ImageFormat, RgbImage};
use serde_json::{json, Value};

use crate::tls;

/// The image providers: key, name shown in the dialog, and the image models offered (the first one is the default). A model typed in the
/// dialog is used as it is.
const PROVIDERS: [(&str, &str, [&str; 2]); 3] = [
    ("gemini", "Gemini", ["gemini-3-pro-image-preview", "gemini-3.1-flash-image-preview"]),
    ("grok", "Grok", ["grok-imagine-image-2.0", "grok-imagine-image"]),
    ("openai", "OpenAI", ["gpt-image-2", "gpt-image-1"]),
];
/// LLM Runner provider names -> the provider here. `xai` is the old name of Grok.
const ALIASES: [(&str, &str); 5] = [("google", "gemini"), ("gemini", "gemini"), ("grok", "grok"), ("xai", "grok"), ("openai", "openai")];
const USER_AGENT: &str = "VRGDG-LLM-Multi/1.0 (+ComfyUI)";
const TIMEOUT: Duration = Duration::from_secs(240);
const MAX_SOURCE_SIDE: u32 = 2048;
const WIDE: f64 = 16.0 / 9.0;
const WIDE_TOLERANCE: f64 = 0.03;
const OPENAI_SIZE: &str = "1536x1024";

/// Where each provider is reached. [`Endpoints::hosted`] is the real one, a test points them at a local server.
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub gemini: String,
    pub grok: String,
    pub openai: String,
    /// A picture a provider answers with as a link is only downloaded from `https://`. A test turns this off for its local server.
    pub https_links_only: bool,
}

impl Endpoints {
    pub fn hosted() -> Self {
        Self {
            gemini: "https://generativelanguage.googleapis.com/v1beta".into(),
            grok: "https://api.x.ai/v1".into(),
            openai: "https://api.openai.com/v1".into(),
            https_links_only: true,
        }
    }
}

/// A provider refused the request or answered without a picture. The text is shown to the person.
#[derive(Debug)]
struct Failure {
    message: String,
    status: u16,
}

impl Failure {
    fn new(message: impl Into<String>) -> Self {
        Self { message: message.into(), status: 0 }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

type Drawn = Result<RgbImage, Failure>;

/// The providers and their image models, for the Thumbnail dialog's choice (the single source of the lists).
pub fn capabilities() -> Value {
    let mut providers = serde_json::Map::new();
    for (key, label, models) in PROVIDERS {
        providers.insert(key.into(), json!({"label": label, "models": models, "default": models[0]}));
    }
    json!({"providers": providers})
}

/// The provider behind an LLM Runner provider name (`google` is Gemini). An error says what to pick for one that cannot draw.
pub fn provider(name: &str) -> Result<&'static str, String> {
    let clean = name.trim().to_lowercase();
    if clean.is_empty() {
        return Err("The LLM Runner has no API provider. Pick Gemini (google), Grok or OpenAI there and add its key.".into());
    }
    ALIASES
        .iter()
        .find(|(alias, _)| *alias == clean)
        .map(|(_, provider)| *provider)
        .ok_or_else(|| format!("The LLM Runner provider '{clean}' cannot draw pictures here. Pick Gemini (google), Grok or OpenAI in the LLM Runner."))
}

/// The provider's first image model.
pub fn default_model(provider: &str) -> String {
    PROVIDERS.iter().find(|(key, _, _)| *key == provider).map(|(_, _, models)| models[0].to_string()).unwrap_or_default()
}

fn label(provider: &str) -> &'static str {
    PROVIDERS.iter().find(|(key, _, _)| *key == provider).map(|(_, label, _)| *label).unwrap_or("The provider")
}

/// The source frame as PNG, shrunk to [`MAX_SOURCE_SIDE`] on its long side so the upload stays small.
fn png_bytes(source: &RgbImage) -> Result<Vec<u8>, Failure> {
    let longest = source.width().max(source.height());
    let mut out = Vec::new();
    let write = |picture: &RgbImage, out: &mut Vec<u8>| picture.write_to(&mut Cursor::new(out), ImageFormat::Png);
    let written = if longest > MAX_SOURCE_SIDE {
        let scale = f64::from(MAX_SOURCE_SIDE) / f64::from(longest);
        let size = |side: u32| ((f64::from(side) * scale).round_ties_even() as u32).max(1);
        write(&imageops::resize(source, size(source.width()), size(source.height()), FilterType::Lanczos3), &mut out)
    } else {
        write(source, &mut out)
    };
    written.map_err(|e| Failure::new(format!("The frame could not be prepared: {e}")))?;
    Ok(out)
}

/// Send one request. A refusal reads `<provider> HTTP <code>: <what it said>`, a connection problem `<provider> network error: ...`.
/// Nothing the request carried (the key is in a header) is put in either.
async fn read(provider: &str, build: impl Fn(&reqwest::Client) -> reqwest::RequestBuilder + Sync) -> Result<Vec<u8>, Failure> {
    let response = tls::send(|client| build(client).timeout(TIMEOUT)).await.map_err(|e| Failure::new(format!("{provider} network error: {e}")))?;
    let status = response.status();
    let body = response.bytes().await.map_err(|e| Failure::new(format!("{provider} network error: {e}")))?;
    if status.is_success() {
        return Ok(body.to_vec());
    }
    let text = String::from_utf8_lossy(&body).trim().to_string();
    let hint = if provider == "Grok" && status.as_u16() == 403 && text.contains("1010") { " xAI blocked the request (code 1010): check the account, region or firewall." } else { "" };
    let shown: String = text.chars().take(600).collect();
    Err(Failure { message: format!("{provider} HTTP {}: {shown}{hint}", status.as_u16()), status: status.as_u16() })
}

fn parse_json(raw: &[u8], provider: &str) -> Result<Value, Failure> {
    match serde_json::from_slice::<Value>(raw) {
        Ok(data @ Value::Object(_)) => Ok(data),
        Ok(_) => Err(Failure::new(format!("{provider} answered with an unexpected reply."))),
        Err(_) => Err(Failure::new(format!("{provider} answered with something that is not JSON."))),
    }
}

async fn post_json(url: &str, headers: &[(&str, String)], body: &Value, provider: &str) -> Result<Value, Failure> {
    let bytes = serde_json::to_vec(body).map_err(|e| Failure::new(e.to_string()))?;
    let raw = read(provider, |client| {
        let mut request = client.post(url).header("Content-Type", "application/json").header("Accept", "application/json").header("User-Agent", USER_AGENT);
        for (name, value) in headers {
            request = request.header(*name, value);
        }
        request.body(bytes.clone())
    })
    .await?;
    parse_json(&raw, provider)
}

/// A multipart form by hand: text fields, then files as `(field, file name, media type, bytes)`.
async fn post_multipart(url: &str, headers: &[(&str, String)], fields: &[(&str, String)], files: &[(&str, &str, &str, Vec<u8>)], provider: &str) -> Result<Value, Failure> {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or_default();
    let boundary = format!("----VRGDG{nanos:x}");
    let mut body: Vec<u8> = Vec::new();
    for (name, value) in fields {
        body.extend(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").bytes());
    }
    for (name, file, media, data) in files {
        body.extend(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"{file}\"\r\nContent-Type: {media}\r\n\r\n").bytes());
        body.extend(data);
        body.extend(b"\r\n");
    }
    body.extend(format!("--{boundary}--\r\n").bytes());
    let raw = read(provider, |client| {
        let mut request = client.post(url).header("Content-Type", format!("multipart/form-data; boundary={boundary}")).header("Accept", "application/json").header("User-Agent", USER_AGENT);
        for (name, value) in headers {
            request = request.header(*name, value);
        }
        request.body(body.clone())
    })
    .await?;
    parse_json(&raw, provider)
}

/// Post `body`. When the provider answers 400 (an option it does not know) post `trimmed`, the plain request, once.
async fn post_json_trimmed(url: &str, headers: &[(&str, String)], body: &Value, trimmed: &Value, provider: &str) -> Result<Value, Failure> {
    match post_json(url, headers, body, provider).await {
        Err(failure) if failure.status == 400 => post_json(url, headers, trimmed, provider).await,
        other => other,
    }
}

fn open_image(raw: &[u8], provider: &str) -> Drawn {
    image::load_from_memory(raw).map(|picture| picture.to_rgb8()).map_err(|_| Failure::new(format!("{provider} sent data that is not a picture.")))
}

/// Base64 as the providers send it: whitespace is ignored and the padding may be missing.
fn decode_base64(text: &str, provider: &str) -> Result<Vec<u8>, Failure> {
    let clean: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    STANDARD.decode(clean.trim_end_matches('=')).or_else(|_| STANDARD_NO_PAD.decode(clean.trim_end_matches('='))).map_err(|_| Failure::new(format!("{provider} sent a picture that could not be decoded.")))
}

/// The first picture of an OpenAI style `{"data": [{"b64_json" | "url": ...}]}` reply.
async fn picture_from_data(reply: &Value, provider: &str, https_only: bool) -> Drawn {
    let first = reply.get("data").and_then(Value::as_array).and_then(|items| items.first()).filter(|item| item.is_object());
    if let Some(encoded) = first.and_then(|item| item.get("b64_json")).and_then(Value::as_str).filter(|text| !text.trim().is_empty()) {
        return open_image(&decode_base64(encoded, provider)?, provider);
    }
    if let Some(link) = first.and_then(|item| item.get("url")).and_then(Value::as_str).filter(|link| !https_only || link.starts_with("https://")) {
        let raw = read(provider, |client| client.get(link).header("User-Agent", USER_AGENT)).await?;
        return open_image(&raw, provider);
    }
    let detail = match reply.get("error") {
        Some(Value::Object(error)) => error.get("message").map(plain_text).unwrap_or_default(),
        Some(other) if !other.is_null() => plain_text(other),
        _ => String::new(),
    };
    let detail: String = detail.chars().take(300).collect();
    Err(Failure::new(format!("{provider} returned no picture.{}", if detail.is_empty() { String::new() } else { format!(" {detail}") })))
}

fn plain_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

async fn gemini_image(endpoints: &Endpoints, api_key: &str, model: &str, prompt: &str, source: &RgbImage) -> Drawn {
    let url = format!("{}/models/{}:generateContent", endpoints.gemini.trim_end_matches('/'), quote(model));
    let headers = [("x-goog-api-key", api_key.to_string())];
    let parts = json!([{"text": prompt}, {"inlineData": {"mimeType": "image/png", "data": STANDARD.encode(png_bytes(source)?)}}]);
    let plain = json!({"contents": [{"role": "user", "parts": parts}]});
    let mut wide = plain.clone();
    wide["generationConfig"] = json!({"responseModalities": ["TEXT", "IMAGE"], "imageConfig": {"aspectRatio": "16:9"}});
    let reply = post_json_trimmed(&url, &headers, &wide, &plain, "Gemini").await?;
    let mut said: Vec<String> = Vec::new();
    for candidate in reply.get("candidates").and_then(Value::as_array).into_iter().flatten() {
        for part in candidate.pointer("/content/parts").and_then(Value::as_array).into_iter().flatten() {
            let inline = part.get("inlineData").or_else(|| part.get("inline_data"));
            if let Some(data) = inline.and_then(|inline| inline.get("data")).and_then(Value::as_str).filter(|data| !data.is_empty()) {
                return open_image(&decode_base64(data, "Gemini")?, "Gemini");
            }
            if let Some(text) = part.get("text").and_then(Value::as_str) {
                said.push(text.trim().to_string());
            }
        }
    }
    let reason = reply.pointer("/promptFeedback/blockReason").map(plain_text).unwrap_or_default();
    let detail = if reason.is_empty() { String::new() } else { format!(" Blocked: {reason}.") };
    let text: String = said.iter().filter(|item| !item.is_empty()).cloned().collect::<Vec<_>>().join(" ").chars().take(300).collect();
    Err(Failure::new(format!("Gemini returned no picture.{detail}{}", if text.is_empty() { String::new() } else { format!(" It said: {text}") })))
}

async fn grok_image(endpoints: &Endpoints, api_key: &str, model: &str, prompt: &str, source: &RgbImage) -> Drawn {
    let data_uri = format!("data:image/png;base64,{}", STANDARD.encode(png_bytes(source)?));
    let headers = [("Authorization", format!("Bearer {api_key}"))];
    let plain = json!({"model": model, "prompt": prompt, "image": {"url": data_uri, "type": "image_url"}});
    let mut body = plain.clone();
    body["response_format"] = json!("b64_json");
    let reply = post_json_trimmed(&format!("{}/images/edits", endpoints.grok.trim_end_matches('/')), &headers, &body, &plain, "Grok").await?;
    picture_from_data(&reply, "Grok", endpoints.https_links_only).await
}

async fn openai_image(endpoints: &Endpoints, api_key: &str, model: &str, prompt: &str, source: &RgbImage) -> Drawn {
    let fields = [("model", model.to_string()), ("prompt", prompt.to_string()), ("size", OPENAI_SIZE.to_string())];
    let files = [("image", "frame.png", "image/png", png_bytes(source)?)];
    let reply = post_multipart(&format!("{}/images/edits", endpoints.openai.trim_end_matches('/')), &[("Authorization", format!("Bearer {api_key}"))], &fields, &files, "OpenAI").await?;
    picture_from_data(&reply, "OpenAI", endpoints.https_links_only).await
}

/// A path segment as the pack's `urllib.parse.quote(model, safe='-_.~')` writes it.
fn quote(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Centre-crop to 16:9 when a provider answered with another shape (OpenAI has no 16:9 size). 16:9 stays as it is.
pub fn fit_wide(picture: RgbImage) -> RgbImage {
    let (width, height) = picture.dimensions();
    let ratio = f64::from(width) / f64::from(height);
    if (ratio - WIDE).abs() / WIDE <= WIDE_TOLERANCE {
        return picture;
    }
    if ratio < WIDE {
        let crop = ((f64::from(width) / WIDE).round_ties_even() as u32).clamp(1, height);
        let top = (height - crop) / 2;
        return imageops::crop_imm(&picture, 0, top, width, crop).to_image();
    }
    let crop = ((f64::from(height) * WIDE).round_ties_even() as u32).clamp(1, width);
    let left = (width - crop) / 2;
    imageops::crop_imm(&picture, left, 0, crop, height).to_image()
}

/// Send the frame and the prompt to the provider and return the picture it drew, 16:9. `provider` is one of the keys [`provider`] gives. The
/// error is a readable message: a missing key or prompt, or what the provider answered.
pub async fn edit_with_api(provider: &str, api_key: &str, model: &str, prompt: &str, source: &RgbImage) -> Result<RgbImage, String> {
    edit_with_api_at(&Endpoints::hosted(), provider, api_key, model, prompt, source).await
}

/// [`edit_with_api`] with the addresses of the providers.
pub async fn edit_with_api_at(endpoints: &Endpoints, provider: &str, api_key: &str, model: &str, prompt: &str, source: &RgbImage) -> Result<RgbImage, String> {
    if !PROVIDERS.iter().any(|(key, _, _)| *key == provider) {
        return Err(format!("Unknown image provider: {provider}"));
    }
    if api_key.trim().is_empty() {
        return Err(format!("Add your {} API key in the LLM Runner first.", label(provider)));
    }
    if prompt.trim().is_empty() {
        return Err("Write the prompt first.".into());
    }
    let (key, prompt) = (api_key.trim(), prompt.trim());
    let drawn = match provider {
        "gemini" => gemini_image(endpoints, key, model, prompt, source).await,
        "grok" => grok_image(endpoints, key, model, prompt, source).await,
        _ => openai_image(endpoints, key, model, prompt, source).await,
    };
    drawn.map(fit_wide).map_err(|failure| failure.message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_llm_runner_provider_names_map_to_the_image_providers() {
        assert_eq!(provider("google").unwrap(), "gemini");
        assert_eq!(provider(" Grok ").unwrap(), "grok");
        assert_eq!(provider("xai").unwrap(), "grok");
        assert_eq!(provider("openai").unwrap(), "openai");
    }

    #[test]
    fn a_provider_that_cannot_draw_is_refused_with_what_to_pick() {
        assert!(provider("anthropic").unwrap_err().contains("Gemini (google), Grok or OpenAI"));
        assert!(provider("").unwrap_err().contains("no API provider"));
        assert!(provider("  ").unwrap_err().contains("no API provider"));
    }

    #[test]
    fn the_capabilities_list_every_provider_with_its_models_and_a_default() {
        let all = capabilities();
        let providers = all["providers"].as_object().unwrap();
        assert_eq!(providers.keys().cloned().collect::<Vec<_>>(), ["gemini", "grok", "openai"]);
        for info in providers.values() {
            assert_eq!(info["default"], info["models"][0]);
            assert!(!info["label"].as_str().unwrap().is_empty());
        }
        assert_eq!(all["providers"]["gemini"]["default"], "gemini-3-pro-image-preview");
        assert_eq!(default_model("openai"), "gpt-image-2");
    }

    #[test]
    fn the_grok_image_models_stay_out_of_the_chat_model_lists() {
        let chat = crate::providers::choices().to_string();
        assert!(!chat.contains("grok-imagine"), "the image models belong here only");
    }

    #[test]
    fn sixteen_by_nine_is_left_alone_and_other_shapes_are_cropped_in_the_middle() {
        assert_eq!(fit_wide(RgbImage::new(1920, 1080)).dimensions(), (1920, 1080));
        assert_eq!(fit_wide(RgbImage::new(1000, 1000)).dimensions(), (1000, 562));
        assert_eq!(fit_wide(RgbImage::new(2400, 1000)).dimensions(), (1778, 1000));
        assert_eq!(fit_wide(RgbImage::new(1536, 1024)).dimensions(), (1536, 864));
    }

    #[test]
    fn the_crop_keeps_the_middle() {
        let mut tall = RgbImage::from_pixel(160, 160, image::Rgb([0, 0, 0]));
        for y in 60..100 {
            for x in 0..160 {
                tall.put_pixel(x, y, image::Rgb([255, 255, 255]));
            }
        }
        let cropped = fit_wide(tall);
        assert_eq!(cropped.dimensions(), (160, 90));
        assert_eq!(cropped.get_pixel(80, 45).0, [255, 255, 255]);
    }

    #[test]
    fn a_big_frame_is_shrunk_before_it_is_sent() {
        let big = image::load_from_memory(&png_bytes(&RgbImage::new(4096, 2304)).unwrap()).unwrap();
        assert_eq!((big.width(), big.height()), (2048, 1152));
        let small = image::load_from_memory(&png_bytes(&RgbImage::new(640, 360)).unwrap()).unwrap();
        assert_eq!((small.width(), small.height()), (640, 360));
    }

    #[test]
    fn a_model_name_is_written_into_the_address_like_the_pack_does() {
        assert_eq!(quote("gemini-3-pro-image-preview"), "gemini-3-pro-image-preview");
        assert_eq!(quote("my-own-model.2"), "my-own-model.2");
        assert_eq!(quote("a:b c"), "a%3Ab%20c");
    }

    #[test]
    fn base64_may_have_line_breaks_and_no_padding() {
        assert_eq!(decode_base64("aGk=", "X").unwrap(), b"hi");
        assert_eq!(decode_base64("aG k", "X").unwrap(), b"hi");
        assert_eq!(decode_base64("aGk\n", "X").unwrap(), b"hi");
        assert!(decode_base64("!!!", "X").unwrap_err().message.contains("could not be decoded"));
    }
}
