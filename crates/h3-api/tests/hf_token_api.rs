//! The Hugging Face token of App Settings through the real router: saved in a file of its own next to the config file, never returned, checked for its
//! shape, and what lets the model installer start a gated file.

use std::path::Path;
use std::time::Duration;

use axum::routing::get;
use axum::{Json, Router};
use h3_project::AppConfig;
use reqwest::{Client, Method};
use serde_json::{json, Value};

const TOKEN: &str = "/api/v1/setup/hf_token";

/// A stand-in ComfyUI that reports its model folders under `models`.
async fn comfy(models: &Path) -> String {
    let folders = json!({
        "loras": [models.join("loras").display().to_string()],
        "diffusion_models": [models.join("unet").display().to_string(), models.join("diffusion_models").display().to_string()],
        "vae": [models.join("vae").display().to_string()],
    });
    let app = Router::new().route("/internal/folder_paths", get(move || {
        let folders = folders.clone();
        async move { Json(folders) }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

struct Server {
    base: String,
    client: Client,
    dir: tempfile::TempDir,
}

async fn start() -> Server {
    let dir = tempfile::tempdir().unwrap();
    let comfy_url = comfy(&dir.path().join("models")).await;
    let cfg = AppConfig { comfy_url, project_root: dir.path().join("projects"), ..AppConfig::default() };
    std::fs::create_dir_all(&cfg.project_root).unwrap();
    let state = h3_api::build_state(cfg, &dir.path().join("jobs.db")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, h3_api::router(state)).await.unwrap() });
    Server { base, client: Client::builder().timeout(Duration::from_secs(30)).build().unwrap(), dir }
}

impl Server {
    async fn send(&self, method: Method, path: &str, body: Value) -> (u16, Value) {
        let reply = self.client.request(method, format!("{}{path}", self.base)).json(&body).send().await.unwrap();
        (reply.status().as_u16(), reply.json().await.unwrap_or(Value::Null))
    }
}

#[tokio::test]
async fn the_token_is_saved_in_its_own_file_and_never_comes_back() {
    let server = start().await;
    // Nothing is saved at first.
    let (code, body) = server.send(Method::GET, TOKEN, Value::Null).await;
    assert_eq!((code, body["data"]["configured"].clone()), (200, json!(false)));

    // A token that is not one is refused, and so is a missing field.
    for bad in ["not_a_token", "hf_two words", "", "  "].iter().filter(|t| !t.trim().is_empty()) {
        let (code, body) = server.send(Method::PUT, TOKEN, json!({"api_key": bad})).await;
        assert_eq!(code, 400, "{bad}");
        assert!(body.to_string().contains("starts with hf_"), "{body}");
    }
    let (code, _) = server.send(Method::PUT, TOKEN, json!({})).await;
    assert_eq!(code, 400);
    let (code, _) = server.send(Method::PUT, TOKEN, json!({"api_key": format!("hf_{}", "a".repeat(500))})).await;
    assert_eq!(code, 400);

    let (code, body) = server.send(Method::PUT, TOKEN, json!({"api_key": "  hf_secretTOKEN123  "})).await;
    assert_eq!((code, body["data"]["configured"].clone()), (200, json!(true)));
    assert!(!body.to_string().contains("secretTOKEN123"), "the reply never carries the token");
    let (_, body) = server.send(Method::GET, TOKEN, Value::Null).await;
    assert_eq!(body["data"]["configured"], true);
    assert!(!body.to_string().contains("secretTOKEN123"));

    // It is in a file of its own, trimmed, and the config the Settings screen reads has nothing of it.
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(server.dir.path().join("hf_token.json")).unwrap()).unwrap();
    assert_eq!(saved["hf_token"], "hf_secretTOKEN123", "saved without the spaces the person pasted around it");
    let (_, config) = server.send(Method::GET, "/api/v1/setup/config", Value::Null).await;
    assert!(!config.to_string().contains("secretTOKEN123"), "{config}");
    let (_, status) = server.send(Method::GET, "/api/v1/setup/status", Value::Null).await;
    assert!(!status.to_string().contains("secretTOKEN123"));

    // An empty string clears it.
    let (code, body) = server.send(Method::PUT, TOKEN, json!({"api_key": ""})).await;
    assert_eq!((code, body["data"]["configured"].clone()), (200, json!(false)));
    assert!(!server.dir.path().join("hf_token.json").exists());
}

#[tokio::test]
async fn testing_without_a_token_says_to_paste_one() {
    let server = start().await;
    let (code, body) = server.send(Method::POST, &format!("{TOKEN}/test"), json!({})).await;
    assert_eq!(code, 400);
    assert!(body.to_string().contains("No Hugging Face token is saved"), "{body}");
}

#[tokio::test]
async fn the_models_dialog_saves_the_token_of_app_settings_and_never_gets_it_back() {
    let server = start().await;
    let models = "/vrgdg/video_builder/models";
    let save = format!("{models}/hf_token");
    let (code, status) = server.send(Method::GET, &format!("{models}/status"), Value::Null).await;
    assert_eq!((code, status["hf_token_saved"].clone()), (200, json!(false)));

    // What is not a token is refused with the same words as App Settings, and so is a missing field. Nothing is written.
    for body in [json!({"api_key": "not_a_token"}), json!({"api_key": "hf_two words"}), json!({"api_key": "hf_line\nbreak"}), json!({})] {
        let (code, reply) = server.send(Method::POST, &save, body.clone()).await;
        assert_eq!((code, reply["ok"].clone()), (400, json!(false)), "{body}");
    }
    let (_, reply) = server.send(Method::POST, &save, json!({"api_key": "nope"})).await;
    assert!(reply["error"].as_str().unwrap().contains("starts with hf_"), "{reply}");
    assert!(!server.dir.path().join("hf_token.json").exists());

    // Saved trimmed, in the file App Settings uses, so App Settings and the status both see it. No reply carries it.
    let (code, reply) = server.send(Method::POST, &save, json!({"api_key": "  hf_secretTOKEN123  "})).await;
    assert_eq!((code, reply["ok"].clone(), reply["configured"].clone()), (200, json!(true), json!(true)));
    assert!(!reply.to_string().contains("secretTOKEN123"), "{reply}");
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(server.dir.path().join("hf_token.json")).unwrap()).unwrap();
    assert_eq!(saved["hf_token"], "hf_secretTOKEN123");
    let (_, settings) = server.send(Method::GET, TOKEN, Value::Null).await;
    assert_eq!(settings["data"]["configured"], true);
    let (_, status) = server.send(Method::GET, &format!("{models}/status"), Value::Null).await;
    assert_eq!(status["hf_token_saved"], true);
    assert!(!status.to_string().contains("secretTOKEN123"));

    // An empty string clears it.
    let (code, reply) = server.send(Method::POST, &save, json!({"api_key": ""})).await;
    assert_eq!((code, reply["configured"].clone()), (200, json!(false)));
    assert!(!server.dir.path().join("hf_token.json").exists());
    let (_, status) = server.send(Method::GET, &format!("{models}/status"), Value::Null).await;
    assert_eq!(status["hf_token_saved"], false);
}

#[tokio::test]
async fn testing_from_the_models_dialog_needs_a_token_and_refuses_one_that_is_not_shaped_like_one() {
    let server = start().await;
    let test = "/vrgdg/video_builder/models/hf_token/test";
    let (code, reply) = server.send(Method::POST, test, json!({})).await;
    assert_eq!(code, 400);
    assert!(reply["error"].as_str().unwrap().contains("No Hugging Face token is saved"), "{reply}");
    // A pasted token with a line break is refused before anything is asked of Hugging Face.
    let (code, reply) = server.send(Method::POST, test, json!({"api_key": "hf_a\nX-Evil: 1"})).await;
    assert_eq!(code, 400);
    assert!(reply["error"].as_str().unwrap().contains("starts with hf_"), "{reply}");
}

#[tokio::test]
async fn a_gated_file_starts_only_with_a_saved_token_and_the_status_says_so() {
    let server = start().await;
    let models = "/vrgdg/video_builder/models";
    let gated = |body: &Value| body["models"].as_array().unwrap().iter().find(|m| m["id"] == "flux_klein_9b_diffusion").unwrap().clone();

    // No token: the file is blocked, the status says what to do, and an install is refused before anything starts.
    let (code, status) = server.send(Method::GET, &format!("{models}/status"), Value::Null).await;
    assert_eq!((code, status["hf_token_saved"].clone()), (200, json!(false)));
    let row = gated(&status);
    assert_eq!((row["gated"].clone(), row["token_saved"].clone()), (json!(true), json!(false)));
    assert!(row["gated_message"].as_str().unwrap().contains("App Settings"));
    let (code, body) = server.send(Method::POST, &format!("{models}/install"), json!({"ids": ["flux_klein_9b_diffusion"]})).await;
    assert!(code >= 400, "{code}");
    assert!(body.to_string().contains("App Settings") && body.to_string().contains("9B diffusion model"), "{body}");

    // With a token the status unblocks it. The diffusion model goes to diffusion_models, not the legacy unet folder.
    server.send(Method::PUT, TOKEN, json!({"api_key": "hf_secretTOKEN123"})).await;
    let (_, status) = server.send(Method::GET, &format!("{models}/status"), Value::Null).await;
    assert_eq!(status["hf_token_saved"], true);
    let row = gated(&status);
    assert_eq!((row["gated"].clone(), row["token_saved"].clone(), row["gated_message"].clone()), (json!(true), json!(true), Value::Null));
    assert!(row["destination"].as_str().unwrap().ends_with("diffusion_models"), "{row}");
    assert!(!status.to_string().contains("secretTOKEN123"), "the status never carries the token");
}
