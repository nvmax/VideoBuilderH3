//! The Thumbnail dialog's API image engine against a mock server: what goes out to Gemini, Grok and OpenAI, and what is made of the answers.
//! Mirrors the pack's `tests/test_thumbnail_api_image.py`. No real call is made.

use std::collections::{HashMap, VecDeque};
use std::io::Cursor;
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::Router;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use h3_llm::api_image::{edit_with_api_at, Endpoints};
use image::{ImageFormat, Rgb, RgbImage};
use serde_json::{json, Value};

const KEY: &str = "sk-test-secret-key";

#[derive(Clone)]
struct Seen {
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

impl Seen {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }
}

enum Answer {
    Json(Value),
    Raw(Vec<u8>),
    Status(u16, &'static str),
}

#[derive(Default)]
struct Wire {
    answers: Mutex<VecDeque<Answer>>,
    seen: Mutex<Vec<Seen>>,
}

async fn answer(State(wire): State<Arc<Wire>>, uri: Uri, headers: HeaderMap, body: Bytes) -> Response {
    let headers = headers.iter().map(|(name, value)| (name.as_str().to_string(), value.to_str().unwrap_or("").to_string())).collect();
    wire.seen.lock().unwrap().push(Seen { path: uri.path().to_string(), headers, body: body.to_vec() });
    match wire.answers.lock().unwrap().pop_front().expect("the mock ran out of answers") {
        Answer::Json(value) => axum::Json(value).into_response(),
        Answer::Raw(bytes) => bytes.into_response(),
        Answer::Status(code, text) => (StatusCode::from_u16(code).unwrap(), text).into_response(),
    }
}

/// A server that answers each request with the next item and keeps the requests for the assertions.
async fn start(answers: Vec<Answer>) -> (Endpoints, Arc<Wire>) {
    let wire = Arc::new(Wire { answers: Mutex::new(answers.into()), ..Wire::default() });
    let app = Router::new().fallback(answer).with_state(wire.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let endpoints = Endpoints { gemini: format!("{base}/gemini/v1beta"), grok: format!("{base}/grok/v1"), openai: format!("{base}/openai/v1"), https_links_only: false };
    (endpoints, wire)
}

fn png(width: u32, height: u32, color: [u8; 3]) -> Vec<u8> {
    let mut out = Vec::new();
    RgbImage::from_pixel(width, height, Rgb(color)).write_to(&mut Cursor::new(&mut out), ImageFormat::Png).unwrap();
    out
}

fn b64(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

fn gemini_reply(image: &[u8]) -> Answer {
    Answer::Json(json!({"candidates": [{"content": {"parts": [{"text": "Here it is."}, {"inlineData": {"mimeType": "image/png", "data": b64(image)}}]}}]}))
}

fn b64_reply(image: &[u8]) -> Answer {
    Answer::Json(json!({"data": [{"b64_json": b64(image)}]}))
}

async fn edit(endpoints: &Endpoints, provider: &str, model: &str, key: &str) -> Result<RgbImage, String> {
    edit_with_api_at(endpoints, provider, key, model, "Add the title HEAT in graffiti.", &RgbImage::new(640, 360)).await
}

#[tokio::test]
async fn gemini_gets_the_frame_and_the_prompt_and_the_key_stays_in_a_header() {
    let (endpoints, wire) = start(vec![gemini_reply(&png(640, 360, [200, 40, 40]))]).await;
    edit(&endpoints, "gemini", "gemini-3-pro-image-preview", KEY).await.unwrap();
    let seen = wire.seen.lock().unwrap();
    assert_eq!(seen[0].path, "/gemini/v1beta/models/gemini-3-pro-image-preview:generateContent");
    assert!(!seen[0].path.contains(KEY));
    assert_eq!(seen[0].header("x-goog-api-key"), Some(KEY));
    let body = seen[0].json();
    assert_eq!(body["contents"][0]["parts"][0]["text"], "Add the title HEAT in graffiti.");
    assert_eq!(body["contents"][0]["parts"][1]["inlineData"]["mimeType"], "image/png");
    assert_eq!(body["generationConfig"]["imageConfig"]["aspectRatio"], "16:9");
}

#[tokio::test]
async fn the_picture_is_taken_from_the_inline_data() {
    let (endpoints, _) = start(vec![gemini_reply(&png(640, 360, [10, 200, 30]))]).await;
    let picture = edit(&endpoints, "gemini", "m", KEY).await.unwrap();
    assert_eq!(picture.get_pixel(5, 5).0, [10, 200, 30]);
}

#[tokio::test]
async fn an_option_the_model_does_not_know_is_retried_once_without_it() {
    let (endpoints, wire) = start(vec![Answer::Status(400, "imageConfig is not supported"), gemini_reply(&png(640, 360, [1, 2, 3]))]).await;
    edit(&endpoints, "gemini", "m", KEY).await.unwrap();
    let seen = wire.seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert!(seen[0].json().get("generationConfig").is_some());
    assert!(seen[1].json().get("generationConfig").is_none());
}

#[tokio::test]
async fn an_answer_with_words_only_says_what_the_model_said() {
    let (endpoints, _) = start(vec![Answer::Json(json!({"candidates": [{"content": {"parts": [{"text": "I cannot draw that."}]}}]}))]).await;
    let message = edit(&endpoints, "gemini", "m", KEY).await.unwrap_err();
    assert!(message.starts_with("Gemini returned no picture.") && message.contains("It said: I cannot draw that."), "{message}");
}

#[tokio::test]
async fn a_blocked_prompt_says_why() {
    let (endpoints, _) = start(vec![Answer::Json(json!({"promptFeedback": {"blockReason": "SAFETY"}}))]).await;
    let message = edit(&endpoints, "gemini", "m", KEY).await.unwrap_err();
    assert!(message.contains("Blocked: SAFETY."), "{message}");
}

#[tokio::test]
async fn a_refused_key_is_reported_with_the_status_and_never_shows_the_key() {
    let (endpoints, wire) = start(vec![Answer::Status(403, "API key not valid")]).await;
    let message = edit(&endpoints, "gemini", "m", KEY).await.unwrap_err();
    assert!(message.starts_with("Gemini HTTP 403: API key not valid"), "{message}");
    assert!(!message.contains(KEY));
    assert_eq!(wire.seen.lock().unwrap().len(), 1, "only a 400 is retried");
}

#[tokio::test]
async fn grok_gets_the_frame_as_a_data_uri_at_the_edit_endpoint() {
    let (endpoints, wire) = start(vec![b64_reply(&png(640, 360, [9, 9, 9]))]).await;
    edit(&endpoints, "grok", "grok-imagine-image-2.0", KEY).await.unwrap();
    let seen = wire.seen.lock().unwrap();
    assert_eq!(seen[0].path, "/grok/v1/images/edits");
    assert_eq!(seen[0].header("authorization"), Some(format!("Bearer {KEY}").as_str()));
    let body = seen[0].json();
    assert_eq!(body["model"], "grok-imagine-image-2.0");
    assert_eq!(body["image"]["type"], "image_url");
    assert!(body["image"]["url"].as_str().unwrap().starts_with("data:image/png;base64,"));
    assert_eq!(body["response_format"], "b64_json");
}

#[tokio::test]
async fn a_picture_sent_as_a_link_is_downloaded() {
    let (endpoints, wire) = start(vec![Answer::Json(json!({"data": [{"url": "PLACEHOLDER"}]})), Answer::Raw(png(640, 360, [1, 2, 3]))]).await;
    // The first answer has to name the mock's own address, which is only known once it runs.
    let link = format!("{}/one.png", endpoints.grok.trim_end_matches("/grok/v1"));
    wire.answers.lock().unwrap()[0] = Answer::Json(json!({"data": [{"url": link}]}));
    let picture = edit(&endpoints, "grok", "m", KEY).await.unwrap();
    assert_eq!(wire.seen.lock().unwrap()[1].path, "/one.png");
    assert_eq!(picture.get_pixel(0, 0).0, [1, 2, 3]);
}

#[tokio::test]
async fn a_link_that_is_not_https_is_not_followed_by_the_real_build() {
    let hosted = Endpoints::hosted();
    assert!(hosted.https_links_only);
    let (mut endpoints, wire) = start(vec![Answer::Json(json!({"data": [{"url": "http://127.0.0.1:9/one.png"}]}))]).await;
    endpoints.https_links_only = true;
    let message = edit(&endpoints, "grok", "m", KEY).await.unwrap_err();
    assert!(message.starts_with("Grok returned no picture."), "{message}");
    assert_eq!(wire.seen.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn an_option_grok_does_not_know_is_retried_once_with_the_plain_request() {
    let (endpoints, wire) = start(vec![Answer::Status(400, "unknown field response_format"), b64_reply(&png(640, 360, [5, 5, 5]))]).await;
    edit(&endpoints, "grok", "m", KEY).await.unwrap();
    let seen = wire.seen.lock().unwrap();
    assert!(seen[0].json().get("response_format").is_some());
    assert!(seen[1].json().get("response_format").is_none());
}

#[tokio::test]
async fn a_grok_reply_without_a_picture_is_an_error_and_a_blocked_account_gets_the_hint() {
    let (endpoints, _) = start(vec![Answer::Json(json!({"error": {"message": "content policy"}}))]).await;
    let message = edit(&endpoints, "grok", "m", KEY).await.unwrap_err();
    assert_eq!(message, "Grok returned no picture. content policy");

    let (endpoints, _) = start(vec![Answer::Status(403, "error code: 1010")]).await;
    let message = edit(&endpoints, "grok", "m", KEY).await.unwrap_err();
    assert!(message.contains("Grok HTTP 403") && message.contains("code 1010): check the account, region or firewall."), "{message}");
}

#[tokio::test]
async fn openai_gets_the_frame_as_a_form_and_the_three_by_two_answer_is_cropped_to_sixteen_by_nine() {
    let (endpoints, wire) = start(vec![b64_reply(&png(1536, 1024, [30, 30, 220]))]).await;
    let picture = edit(&endpoints, "openai", "gpt-image-2", KEY).await.unwrap();
    assert_eq!(picture.dimensions(), (1536, 864));
    let seen = wire.seen.lock().unwrap();
    assert_eq!(seen[0].path, "/openai/v1/images/edits");
    assert_eq!(seen[0].header("authorization"), Some(format!("Bearer {KEY}").as_str()));
    assert!(seen[0].header("content-type").unwrap().starts_with("multipart/form-data; boundary="));
    let body = String::from_utf8_lossy(&seen[0].body);
    for text in ["name=\"model\"\r\n\r\ngpt-image-2", "name=\"size\"\r\n\r\n1536x1024", "name=\"image\"; filename=\"frame.png\"", "Content-Type: image/png", "Add the title HEAT"] {
        assert!(body.contains(text), "missing {text:?}");
    }
}

#[tokio::test]
async fn a_missing_key_or_prompt_and_an_unknown_provider_are_refused_before_any_call() {
    let (endpoints, wire) = start(vec![]).await;
    assert_eq!(edit(&endpoints, "grok", "m", "  ").await.unwrap_err(), "Add your Grok API key in the LLM Runner first.");
    assert_eq!(edit_with_api_at(&endpoints, "grok", KEY, "m", "  ", &RgbImage::new(4, 4)).await.unwrap_err(), "Write the prompt first.");
    assert_eq!(edit(&endpoints, "anthropic", "m", KEY).await.unwrap_err(), "Unknown image provider: anthropic");
    assert!(wire.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_reply_that_is_not_a_picture_or_not_json_is_an_error_with_the_provider_name() {
    let (endpoints, _) = start(vec![b64_reply(b"not a picture")]).await;
    assert_eq!(edit(&endpoints, "openai", "m", KEY).await.unwrap_err(), "OpenAI sent data that is not a picture.");
    let (endpoints, _) = start(vec![Answer::Raw(b"<html>".to_vec())]).await;
    assert_eq!(edit(&endpoints, "grok", "m", KEY).await.unwrap_err(), "Grok answered with something that is not JSON.");
}

#[tokio::test]
async fn a_network_failure_names_the_provider() {
    // Nothing listens on port 9.
    let endpoints = Endpoints { openai: "http://127.0.0.1:9/v1".into(), ..Endpoints::hosted() };
    let message = edit(&endpoints, "openai", "m", KEY).await.unwrap_err();
    assert!(message.starts_with("OpenAI network error:"), "{message}");
    assert!(!message.contains(KEY));
}
