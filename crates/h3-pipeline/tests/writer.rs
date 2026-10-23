//! The prompt writer against `fixtures/prompt/writer.json`: the pack's final prompt for the same session and the same
//! LLM reply. The LLM is a small local server that answers each case's canned reply.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use h3_llm::LlmConfig;
use h3_pipeline::minimax_prompts::{compose_request, generate_scene_prompt, PromptError, PromptParams};
use serde_json::{json, Value};

type Shared = Arc<Mutex<(String, Vec<Value>)>>;

async fn start() -> (LlmConfig, Shared) {
    let shared: Shared = Arc::new(Mutex::new((String::new(), vec![])));
    let app = Router::new()
        .route("/v1/chat/completions", post(|State(s): State<Shared>, Json(body): Json<Value>| async move {
            let mut guard = s.lock().unwrap();
            guard.1.push(body);
            Json(json!({"choices": [{"message": {"content": guard.0.clone()}}]}))
        }))
        .with_state(Arc::clone(&shared));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let cfg = LlmConfig { runner: "own_server".into(), own_server_url: format!("http://{addr}/v1"), own_server_model: "test-model".into(), ..LlmConfig::default() };
    (cfg, shared)
}

#[tokio::test]
async fn prompts_match_the_pack() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/prompt/writer.json");
    let cases: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let (cfg, shared) = start().await;
    let mut compared = 0;
    for case in cases["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        shared.lock().unwrap().0 = case["reply"].as_str().unwrap().to_string();
        let session = case["session"].as_object().unwrap();
        let result = generate_scene_prompt(&cfg, session, case["index"].as_u64().unwrap() as usize, &PromptParams::default()).await;
        match (result, case.get("error")) {
            (Err(PromptError::Invalid(m)), Some(want)) => assert_eq!(&json!(m), want, "{name}"),
            (Ok(written), None) => {
                assert_eq!(written.prompt, case["prompt"].as_str().unwrap(), "{name}: prompt");
                assert_eq!(json!(written.characters), case["characters"], "{name}");
                assert_eq!(json!(written.shots), case["shots"], "{name}");
                assert_eq!(written.attempts, 1, "{name}");
                compared += 1;
            }
            (other, want) => panic!("{name}: {other:?} vs {want:?}"),
        }
    }
    assert_eq!(compared, 24);
    // The request carries the pack's instructions, the shot task and the default notes.
    let sent = shared.lock().unwrap().1[0].clone();
    let text = sent["messages"][0]["content"].as_str().unwrap();
    assert!(text.starts_with("You are") || text.len() > 5000, "{}", &text[..80.min(text.len())]);
    assert!(text.contains("Scene concept:\nMiniMax H3 shot-description task."));
    assert!(text.contains("User motion/camera notes:\nCreate cinematic camera movement"));
    assert!(compose_request("T", "", "").ends_with("fits the scene.\n\n"));
}

#[tokio::test]
async fn a_bad_reply_is_an_error_not_a_prompt() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/prompt/writer.json");
    let cases: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let case = &cases["cases"][0];
    let (cfg, shared) = start().await;
    for reply in ["not json at all", "{\"shots\": []}", "{\"shots\": [{\"description\": \"[Shot 1] labels inside\"}]}"] {
        shared.lock().unwrap().0 = reply.to_string();
        let result = generate_scene_prompt(&cfg, case["session"].as_object().unwrap(), 0, &PromptParams::default()).await;
        // An empty list is padded with the fallback shot, the others are refused.
        if reply.contains("\"shots\": []") {
            assert!(result.is_ok(), "{result:?}");
        } else {
            assert!(matches!(result, Err(PromptError::Shot(_))), "{reply}: {result:?}");
        }
    }
}
