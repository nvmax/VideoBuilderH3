//! The built-in Gemma through the API, against a stand-in ComfyUI that plays the VRGDG Gemma node.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use h3_project::AppConfig;
use reqwest::{Client, Method};
use serde_json::{json, Value};

type Graphs = Arc<Mutex<Vec<Value>>>;

struct Server {
    base: String,
    client: Client,
    graphs: Graphs,
    dir: tempfile::TempDir,
}

/// A ComfyUI stand-in. With `has_node` false it does not know the Gemma node.
async fn stand_in(has_node: bool, graphs: Graphs) -> String {
    let info = if has_node {
        json!({"VRGDG_SuperGemmaGGUFChat": {"input": {"required": {
            "model_file": [["gemma-small.gguf", "gemma-big.gguf"], {}],
            "mmproj_file": [["mmproj-small.gguf"], {}],
        }}}})
    } else {
        json!({})
    };
    let app = Router::new()
        .route("/object_info/{class}", get(move || { let info = info.clone(); async move { Json(info) } }))
        .route(
            "/prompt",
            post(|State(graphs): State<Graphs>, Json(body): Json<Value>| async move {
                graphs.lock().unwrap().push(body["prompt"].clone());
                Json(json!({"prompt_id": "p1", "number": 1, "node_errors": {}}))
            }),
        )
        .route(
            "/history/{id}",
            get(|State(graphs): State<Graphs>| async move {
                // The answer depends on what was asked: a picture gets a description, anything else gets OK.
                let asked_picture = graphs.lock().unwrap().last().is_some_and(|g| g.get("image1").is_some());
                let text = if asked_picture { "A young woman with short dark hair wears a red jacket." } else { "OK" };
                Json(json!({"p1": {"status": {"status_str": "success", "completed": true, "messages": []}, "outputs": {"show": {"text": [text]}}}}))
            }),
        )
        .route("/upload/image", post(|| async { Json(json!({"name": "uploaded.png", "subfolder": "", "type": "input"})) }))
        .with_state(graphs);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

async fn start(has_node: bool) -> Server {
    let graphs: Graphs = Arc::default();
    let comfy = stand_in(has_node, Arc::clone(&graphs)).await;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("projects");
    std::fs::create_dir_all(&root).unwrap();
    let cfg = AppConfig { comfy_url: comfy, project_root: root, ..AppConfig::default() };
    let state = h3_api::build_state(cfg, &dir.path().join("jobs.db")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, h3_api::router(state)).await.unwrap() });
    Server { base: format!("http://{addr}/api/v1"), client: Client::builder().timeout(Duration::from_secs(60)).build().unwrap(), graphs, dir }
}

impl Server {
    async fn call(&self, method: Method, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut request = self.client.request(method, format!("{}{}", self.base, path));
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Value::Null))
    }

    async fn wait_job(&self, id: &str) -> Value {
        for _ in 0..200 {
            let (_, body) = self.call(Method::GET, &format!("/jobs/{id}"), None).await;
            if ["succeeded", "failed", "cancelled"].contains(&body["data"]["status"].as_str().unwrap_or("")) {
                return body["data"].clone();
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("job {id} did not finish");
    }
}

#[tokio::test]
async fn gemma_in_comfyui_lists_its_files_and_answers() {
    let s = start(true).await;
    let (code, saved) = s.call(Method::PUT, "/llm/settings", Some(json!({"runner": "comfyui_gemma", "comfy_gemma_model": "gemma-big.gguf", "comfy_gemma_keep_loaded": true}))).await;
    assert_eq!(code, 200, "{saved}");

    let (_, listed) = s.call(Method::POST, "/llm/models", Some(json!({"runner": "comfyui_gemma"}))).await;
    let ids: Vec<&str> = listed["data"]["models"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap()).collect();
    assert_eq!((ids, listed["data"]["mmproj"].clone()), (vec!["gemma-small.gguf", "gemma-big.gguf"], json!(["mmproj-small.gguf"])), "{listed}");

    let (code, active) = s.call(Method::GET, "/llm/active", None).await;
    assert_eq!((code, active["data"]["model"].as_str(), active["data"]["runner"].as_str()), (200, Some("gemma-big.gguf"), Some("comfyui_gemma")), "{active}");

    let (code, test) = s.call(Method::POST, "/llm/test", None).await;
    assert_eq!(code, 200, "{test}");
    assert_eq!((test["data"]["reply"].as_str(), test["data"]["runner"].as_str()), (Some("OK"), Some("comfyui_gemma")));
    let graph = s.graphs.lock().unwrap().last().cloned().unwrap();
    assert_eq!(graph["gemma"]["class_type"], "VRGDG_SuperGemmaGGUFChat");
    assert_eq!(graph["gemma"]["inputs"]["model_file"], "gemma-big.gguf");
    assert_eq!(graph["gemma"]["inputs"]["unload_after_run"], false, "kept loaded as saved");
    assert!(graph["gemma"]["inputs"]["custom_instructions"].as_str().unwrap().contains("single word OK"));
}

#[tokio::test]
async fn a_picture_is_uploaded_and_passed_to_the_node() {
    let s = start(true).await;
    s.call(Method::PUT, "/llm/settings", Some(json!({"runner": "comfyui_gemma"}))).await;
    s.call(Method::POST, "/projects", Some(json!({"name": "Pic"}))).await;
    let picture = s.dir.path().join("ava.png");
    image::RgbImage::from_pixel(16, 16, image::Rgb([200, 40, 40])).save(&picture).unwrap();
    s.call(Method::PUT, "/projects/Pic/references/subjects/s", Some(json!({"name": "Ava", "image": {"path": picture.display().to_string()}}))).await;
    let (code, body) = s.call(Method::POST, "/projects/Pic/references/subjects/s/describe", None).await;
    assert_eq!(code, 202, "{body}");
    let job = s.wait_job(body["data"]["job_id"].as_str().unwrap()).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    assert!(job["result"]["description"].as_str().unwrap().contains("short dark hair"), "{job}");
    let graph = s.graphs.lock().unwrap().last().cloned().unwrap();
    assert_eq!(graph["image1"]["class_type"], "LoadImage");
    assert_eq!(graph["image1"]["inputs"]["image"], "uploaded.png");
    assert_eq!((graph["gemma"]["inputs"]["image_count"].as_i64(), graph["gemma"]["inputs"]["image1"].clone()), (Some(1), json!(["image1", 0])));
    assert_eq!(graph["gemma"]["inputs"]["mmproj_file"], "mmproj-small.gguf");
}

#[tokio::test]
async fn a_comfyui_without_the_gemma_node_says_which_pack_to_install() {
    let s = start(false).await;
    s.call(Method::PUT, "/llm/settings", Some(json!({"runner": "comfyui_gemma"}))).await;
    let (code, body) = s.call(Method::POST, "/llm/test", None).await;
    assert_eq!((code, body["error"]["code"].as_str()), (503, Some("LLM_UNAVAILABLE")), "{body}");
    assert!(body["error"]["message"].as_str().unwrap().contains("comfyui-vrgamedevgirl"), "{body}");
    let (code, _) = s.call(Method::PUT, "/llm/settings", Some(json!({"runner": "nope"}))).await;
    assert_eq!(code, 400);
}
