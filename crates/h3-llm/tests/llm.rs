//! Reply cleaning against the pack, and the LM Studio rule against a mock server.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use h3_llm::clean::{clean_plain_text, strip_thinking_text};
use h3_llm::{chat, describe_active, ChatRequest, LlmConfig, LlmError};
use serde_json::{json, Value};

#[test]
fn cleaning_matches_the_pack() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/llm/clean.json");
    let cases: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    for case in cases["cases"].as_array().unwrap() {
        let input = case["input"].as_str().unwrap();
        assert_eq!(strip_thinking_text(input), case["strip"].as_str().unwrap(), "strip {input:?}");
        assert_eq!(clean_plain_text(input), case["plain"].as_str().unwrap(), "plain {input:?}");
    }
}

#[test]
fn output_checks_match_the_pack() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/llm/checks.json");
    let data: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    for case in data["checks"].as_array().unwrap() {
        let text = case["text"].as_str().unwrap();
        assert_eq!(h3_llm::checks::looks_like_repeat_failure(text), case["repeat"].as_bool().unwrap(), "repeat {text:?}");
        assert_eq!(h3_llm::checks::looks_like_unfilled_template(text), case["template"].as_bool().unwrap(), "template {text:?}");
        assert_eq!(h3_llm::checks::looks_like_bad_reference_description(text), case["bad_reference"].as_bool().unwrap(), "bad reference {text:?}");
    }
    for case in data["visual"].as_array().unwrap() {
        let input = case["input"].as_str().unwrap();
        assert_eq!(h3_llm::checks::clean_visual_text(input), case["return"].as_str().unwrap(), "visual {input:?}");
    }
}

#[derive(Default)]
struct Mock {
    models: Mutex<Vec<Value>>,
    chats: Mutex<Vec<Value>>,
    completions: Mutex<Vec<Value>>,
    model_calls: Mutex<u32>,
}

async fn start(models: Vec<Value>) -> (String, Arc<Mock>) {
    let mock = Arc::new(Mock { models: Mutex::new(models), ..Mock::default() });
    let app = Router::new()
        .route("/api/v0/models", get(|State(m): State<Arc<Mock>>| async move {
            *m.model_calls.lock().unwrap() += 1;
            Json(json!({"data": m.models.lock().unwrap().clone()}))
        }))
        .route("/api/v1/chat", post(|State(m): State<Arc<Mock>>, Json(body): Json<Value>| async move {
            m.chats.lock().unwrap().push(body);
            Json(json!({"output": [{"type": "reasoning", "content": "hmm"}, {"type": "message", "content": [{"type": "text", "text": " Hello there. "}]}]}))
        }))
        .route("/v1/chat/completions", post(|State(m): State<Arc<Mock>>, Json(body): Json<Value>| async move {
            m.completions.lock().unwrap().push(body);
            Json(json!({"choices": [{"message": {"content": "{\"ok\": true}"}}]}))
        }))
        .route("/own/chat/completions", post(|State(m): State<Arc<Mock>>, Json(body): Json<Value>| async move {
            m.completions.lock().unwrap().push(body);
            Json(json!({"choices": [{"message": {"content": [{"type": "text", "text": "from own server"}]}}]}))
        }))
        .with_state(Arc::clone(&mock));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), mock)
}

fn model(id: &str, state: &str, kind: &str, ctx: u64) -> Value {
    json!({"id": id, "state": state, "type": kind, "loaded_context_length": ctx})
}

fn cfg(base: &str) -> LlmConfig {
    LlmConfig { lm_studio_base_url: format!("{base}/v1"), ..LlmConfig::default() }
}

#[tokio::test]
async fn nothing_is_sent_when_no_model_is_loaded() {
    let (base, mock) = start(vec![model("big", "not-loaded", "llm", 0), model("emb", "loaded", "embeddings", 512)]).await;
    let err = chat(&cfg(&base), &ChatRequest::text("hi", 0.5, 0.9, 100)).await.unwrap_err();
    assert!(matches!(err, LlmError::Unavailable(ref m) if m.contains("no model loaded")), "{err}");
    assert!(mock.chats.lock().unwrap().is_empty() && mock.completions.lock().unwrap().is_empty(), "a request would have made LM Studio load a model");
    assert!(describe_active(&cfg(&base)).await.is_err());
    let down = chat(&cfg("http://127.0.0.1:9"), &ChatRequest::text("hi", 0.5, 0.9, 100)).await.unwrap_err();
    assert!(matches!(down, LlmError::Unavailable(ref m) if m.contains("Could not reach LM Studio")), "{down}");
}

#[tokio::test]
async fn the_loaded_model_is_used_even_when_another_is_saved() {
    let (base, mock) = start(vec![model("saved-but-not-loaded", "not-loaded", "llm", 0), model("loaded-one", "loaded", "llm", 8192)]).await;
    let mut config = cfg(&base);
    config.lm_studio_model = "saved-but-not-loaded".into();
    config.lm_studio_context_limit = 32768;
    config.lm_studio_output_limit = 4096;
    config.lm_studio_reasoning = "off".into();
    let reply = chat(&config, &ChatRequest::text("Write a line.", 0.45, 0.92, 4000)).await.unwrap();
    assert_eq!((reply.text.as_str(), reply.model.as_str()), ("Hello there.", "loaded-one"));
    let body = mock.chats.lock().unwrap()[0].clone();
    assert_eq!(body["model"], "loaded-one");
    assert_eq!(body["context_length"], 8192, "capped to what is loaded");
    assert_eq!(body["max_output_tokens"], 4096);
    assert_eq!((body["store"].clone(), body["stream"].clone(), body["reasoning"].clone()), (json!(false), json!(false), json!("off")));
    assert_eq!(body["input"], "Write a line.");
    let active = describe_active(&config).await.unwrap();
    assert_eq!((active.model.as_str(), active.context_length, active.saved_model_setting.as_str()), ("loaded-one", 8192, "saved-but-not-loaded"));

    // The saved model is used when it is the one that is loaded.
    mock.models.lock().unwrap().push(model("saved-now-loaded", "loaded", "vlm", 4096));
    config.lm_studio_model = "saved-now-loaded".into();
    assert_eq!(chat(&config, &ChatRequest::text("x", 0.5, 0.9, 10)).await.unwrap().model, "saved-now-loaded");
}

#[tokio::test]
async fn a_long_prompt_shrinks_the_reply_or_is_refused() {
    let (base, mock) = start(vec![model("m", "loaded", "llm", 2000)]).await;
    let config = cfg(&base);
    chat(&config, &ChatRequest::text(&"word ".repeat(900), 0.5, 0.9, 100)).await.unwrap();
    let sent = mock.chats.lock().unwrap()[0]["max_output_tokens"].as_i64().unwrap();
    assert!((256..8192).contains(&sent), "{sent}");
    let err = chat(&config, &ChatRequest::text(&"word ".repeat(4000), 0.5, 0.9, 100)).await.unwrap_err();
    assert!(matches!(err, LlmError::Context(_)), "{err}");
}

#[tokio::test]
async fn schema_requests_use_the_openai_endpoint_and_own_servers_work() {
    let (base, mock) = start(vec![model("m", "loaded", "llm", 8192)]).await;
    let mut request = ChatRequest::text("give json", 0.2, 0.9, 500);
    request.json_schema = Some(json!({"type": "object"}));
    let reply = chat(&cfg(&base), &request).await.unwrap();
    assert_eq!(reply.text, "{\"ok\": true}");
    let body = mock.completions.lock().unwrap()[0].clone();
    assert_eq!(body["response_format"]["json_schema"]["strict"], true);
    assert_eq!(body["messages"][0]["content"], "give json");

    let own = LlmConfig { runner: "own_server".into(), own_server_url: format!("{base}/own"), own_server_model: "gpt-x".into(), ..LlmConfig::default() };
    let reply = chat(&own, &ChatRequest::text("hi", 0.5, 0.9, 100)).await.unwrap();
    assert_eq!((reply.text.as_str(), reply.model.as_str()), ("from own server", "gpt-x"));
    assert!(matches!(chat(&LlmConfig { runner: "nope".into(), ..LlmConfig::default() }, &ChatRequest::text("x", 0.5, 0.9, 10)).await, Err(LlmError::Unavailable(_))));
}

#[test]
fn keys_are_hidden_in_the_settings_view() {
    let config = LlmConfig { api_key: "secret".into(), ..LlmConfig::default() };
    let view = config.redacted();
    assert_eq!((view["api_key"].as_str(), view["api_key_set"].as_bool()), (Some(""), Some(true)));
    assert_eq!(view["own_server_api_key_set"], false);
}

#[test]
fn reference_descriptions_are_cleaned_like_the_builder_does() {
    use h3_llm::describe::{clean_reference_description as clean, instruction};
    let out = clean("Character: The character has pale skin tone, wears a red leather jacket, black boots and silver rings, with short dark hair.", "subject", "Ava").unwrap();
    assert!(out.contains("Ava") && !out.to_lowercase().contains("skin") && !out.to_lowercase().contains("the character"), "{out}");
    let long = (1..=130).map(|i| format!("zq{}{}", (b'a' + (i % 26) as u8) as char, (b'a' + (i / 26) as u8) as char)).collect::<Vec<_>>().join(" ");
    let capped = clean(&long, "location", "").unwrap();
    assert_eq!(capped.split_whitespace().count(), 100);
    assert!(capped.ends_with('.'));
    assert!(clean("hair hair hair hair hair hair hair hair", "subject", "").is_err(), "repeated text is refused");
    assert!(clean("", "subject", "").is_err());
    assert_eq!(clean("Sure. a photo of me thoughta photo of a woman with green eyes and a round face and straight dark hair.", "face", "").unwrap(), "a photo of a woman with green eyes and a round face and straight dark hair.");
    assert!(instruction("subject", "Ava", 1, "").ends_with("Do not call them the character or the subject in the final description."));
    assert!(instruction("extra", "", 500, "gothic").contains("one of a group of 100 background performers in a gothic-appropriate wardrobe."));
    assert!(instruction("vehicle", "Truck", 1, "").contains("concise vehicle reference description") && instruction("vehicle", "Truck", 1, "").ends_with("Truck"));
    assert_eq!(h3_llm::describe::normalize_reference_type(" Weird "), "subject");
}

#[tokio::test]
async fn a_picture_is_sent_to_lm_studio_as_an_image_block_with_reasoning_off() {
    let (base, mock) = start(vec![model("vision-model", "loaded", "vlm", 8192)]).await;
    // The stand-in answers "Hello there.", which is too short to be a description, so it asks twice and then refuses.
    let err = h3_llm::describe::describe_reference(&cfg(&base), "character", "Ava", "data:image/png;base64,AAAA", 1, "").await.unwrap_err();
    assert!(err.to_string().contains("unusable"), "{err}");
    assert_eq!(mock.chats.lock().unwrap().len(), 2, "one calmer second try");
    assert!(mock.chats.lock().unwrap()[1]["input"][0]["content"].as_str().unwrap().contains("Previous output was rejected"));
    let body = mock.chats.lock().unwrap()[0].clone();
    assert_eq!(body["input"][0]["type"], "text");
    assert_eq!(body["input"][1], json!({"type": "image", "data_url": "data:image/png;base64,AAAA"}));
    assert_eq!(body["reasoning"], "off");
}
