//! Image presets, scene images and reference card pictures through the API. No ComfyUI runs: image jobs fail with
//! COMFY_UNAVAILABLE.

use std::path::Path;
use std::time::Duration;

use h3_project::AppConfig;
use reqwest::{Client, Method};
use serde_json::{json, Value};

/// A small valid PNG as base64.
fn png_base64() -> String {
    use base64::Engine;
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::RgbImage::from_pixel(8, 8, image::Rgb([20, 120, 220])).write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    base64::engine::general_purpose::STANDARD.encode(bytes.into_inner())
}
const PROJECT: &str = "/projects/Demo%20Project";

struct Server {
    base: String,
    client: Client,
    _dir: tempfile::TempDir,
}

fn session() -> Value {
    json!({
        "project_name": "Demo",
        "revision": 3,
        "builder_save_revision": 3,
        "segments": [
            {"id": "seg_0001", "start": 0.0, "end": 4.5, "t2i_prompt": "a lake at dawn"},
            {"id": "seg_0002", "start": 4.5, "end": 9.0}
        ],
        "flux_reference_builder": {"subjects": [{"id": "subj1", "name": "Ava", "description": "a singer", "image": {}}], "locations": []}
    })
}

async fn start() -> Server {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("projects").join("Demo Project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("vrgdg_builder_session.json"), serde_json::to_string_pretty(&session()).unwrap()).unwrap();
    // Nothing listens on port 9, so ComfyUI counts as down.
    let cfg = AppConfig { comfy_url: "http://127.0.0.1:9".into(), project_root: dir.path().join("projects"), ..AppConfig::default() };
    let state = h3_api::build_state(cfg, &dir.path().join("jobs.db")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, h3_api::router(state)).await.unwrap() });
    Server { base: format!("http://{addr}/api/v1"), client: Client::new(), _dir: dir }
}

impl Server {
    async fn call(&self, method: Method, path: &str, headers: &[(&str, &str)], body: Option<Value>) -> (u16, Value) {
        let mut request = self.client.request(method, format!("{}{}", self.base, path));
        for (k, v) in headers {
            request = request.header(*k, *v);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Value::Null))
    }

    async fn get(&self, path: &str) -> (u16, Value) {
        self.call(Method::GET, path, &[], None).await
    }

    async fn wait_job(&self, id: &str, status: &str) -> Value {
        for _ in 0..200 {
            let (_, body) = self.get(&format!("/jobs/{id}")).await;
            if body["data"]["status"] == status {
                return body["data"].clone();
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("job {id} never reached {status}");
    }
}

fn text_workflow() -> Value {
    json!({
        "1": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": "m.safetensors"}},
        "2": {"class_type": "CLIPTextEncode", "inputs": {"text": "x", "clip": ["1", 1]}},
        "3": {"class_type": "KSampler", "inputs": {"seed": 1, "steps": 4, "cfg": 1, "positive": ["2", 0], "negative": ["2", 0], "model": ["1", 0], "latent_image": ["4", 0]}},
        "4": {"class_type": "EmptyLatentImage", "inputs": {"width": 512, "height": 512, "batch_size": 1}},
        "5": {"class_type": "SaveImage", "inputs": {"filename_prefix": "t", "images": ["3", 0]}}
    })
}

fn file_name(path: &str) -> String {
    Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

#[tokio::test]
async fn image_presets_are_listed_imported_edited_and_deleted() {
    let s = start().await;
    let (_, body) = s.get("/image-presets").await;
    let ids: Vec<&str> = body["data"].as_array().unwrap().iter().map(|p| p["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["zimage", "flux_klein", "ernie"]);

    let editor = json!({"name": "Editor", "workflow": {"nodes": [], "links": []}});
    let (code, body) = s.call(Method::POST, "/image-presets/import", &[], Some(editor)).await;
    assert_eq!((code, body["error"]["code"].as_str()), (400, Some("VALIDATION_ERROR")));
    assert!(body["error"]["message"].as_str().unwrap().contains("Export (API)"));

    let (code, body) = s.call(Method::POST, "/image-presets/import", &[], Some(json!({"name": "My Model", "workflow": text_workflow()}))).await;
    assert_eq!(code, 201, "{body}");
    assert_eq!(body["data"]["id"], "my_model");
    let keys: Vec<&str> = body["data"]["bindings"].as_array().unwrap().iter().map(|b| b["key"].as_str().unwrap()).collect();
    assert!(keys.contains(&"prompt") && keys.contains(&"seed") && keys.contains(&"width"), "{keys:?}");

    // A dry run fills the graph without ComfyUI.
    let dry = json!({"values": {"prompt": "a lake", "seed": 9, "width": 640}});
    let (code, body) = s.call(Method::POST, "/image-presets/my_model/graph", &[], Some(dry)).await;
    assert_eq!(code, 200, "{body}");
    assert_eq!(body["data"]["graph"]["2"]["inputs"]["text"], "a lake");
    assert_eq!(body["data"]["graph"]["4"]["inputs"]["width"], 640);
    assert_eq!(body["data"]["seed"], 9);

    // A binding that points at a missing node is refused, and a built-in cannot change.
    let bad = json!({"bindings": [{"key": "prompt", "label": "P", "kind": "text", "targets": [{"node": "99", "input": "text"}]}]});
    let (code, body) = s.call(Method::PUT, "/image-presets/my_model", &[], Some(bad)).await;
    assert_eq!((code, body["error"]["code"].as_str()), (400, Some("VALIDATION_ERROR")));
    let (code, _) = s.call(Method::PUT, "/image-presets/zimage", &[], Some(json!({"name": "x"}))).await;
    assert_eq!(code, 400);
    let (code, body) = s.call(Method::PUT, "/image-presets/my_model", &[], Some(json!({"name": "Renamed"}))).await;
    assert_eq!((code, body["data"]["name"].as_str()), (200, Some("Renamed")));

    let (code, _) = s.call(Method::DELETE, "/image-presets/my_model", &[], None).await;
    assert_eq!(code, 200);
    let (code, body) = s.get("/image-presets/my_model").await;
    assert_eq!((code, body["error"]["code"].as_str()), (404, Some("PRESET_NOT_FOUND")));
}

#[tokio::test]
async fn a_scene_image_is_set_approved_reverted_and_cleared() {
    let s = start().await;
    let base = format!("{PROJECT}/scenes/1/image");
    let upload = json!({"image_data": format!("data:image/png;base64,{}", png_base64())});
    let (code, body) = s.call(Method::PUT, &base, &[], Some(upload)).await;
    assert_eq!(code, 200, "{body}");
    let saved = body["data"]["custom_image_path"].as_str().unwrap().to_string();
    assert!(saved.contains("scene_image_previews") && Path::new(&saved).is_file());
    let (_, body) = s.get(&format!("{PROJECT}/scenes/1")).await;
    assert_eq!(body["data"]["approved_image"]["filename"], file_name(&saved));

    let (code, body) = s.call(Method::POST, &format!("{base}/approve"), &[], Some(json!({"image_path": saved}))).await;
    assert_eq!(code, 200, "{body}");
    assert_eq!(body["data"]["approved_image_path"].as_str().unwrap().replace('\\', "/"), saved.replace('\\', "/"));
    let (code, body) = s.call(Method::POST, &format!("{base}/approve"), &[], Some(json!({"image_path": "nowhere.png"}))).await;
    assert_eq!((code, body["error"]["code"].as_str()), (400, Some("VALIDATION_ERROR")));
    let (code, body) = s.call(Method::POST, &format!("{base}/revert"), &[], Some(json!({}))).await;
    assert_eq!((code, body["error"]["code"].as_str()), (400, Some("VALIDATION_ERROR")), "no history yet");

    let (_, body) = s.get(&format!("{base}/history")).await;
    assert!(body["data"]["approved_image_path"].as_str().unwrap().len() > 3);
    let (code, body) = s.call(Method::DELETE, &base, &[("If-Match", "1")], None).await;
    assert_eq!((code, body["error"]["code"].as_str()), (409, Some("REVISION_CONFLICT")));
    let (code, _) = s.call(Method::DELETE, &base, &[], None).await;
    assert_eq!(code, 200);
    let (_, body) = s.get(&format!("{PROJECT}/scenes/1")).await;
    assert!(body["data"]["approved_image"].is_null());
}

#[tokio::test]
async fn a_reference_card_gets_its_picture_from_an_upload() {
    let s = start().await;
    let (_, body) = s.get(&format!("{PROJECT}/references")).await;
    assert_eq!(body["data"]["subjects"][0]["id"], "subj1");
    let (code, body) = s.call(Method::PUT, &format!("{PROJECT}/references/subjects/subj1/image"), &[], Some(json!({"image_data": png_base64()}))).await;
    assert_eq!(code, 200, "{body}");
    let path = body["data"]["image_path"].as_str().unwrap().to_string();
    assert!(path.replace('\\', "/").contains("subject_location/subject/Ava.png"), "{path}");
    let (_, body) = s.get(&format!("{PROJECT}/references")).await;
    assert_eq!(body["data"]["subjects"][0]["image"]["name"], "Ava.png");
    assert!(body["data"]["subjects"][0]["image"].get("data").is_none(), "previews stay out of the listing");
    // The card keeps its preview for the Builder.
    let project = Path::new(&path).parent().unwrap().parent().unwrap().parent().unwrap();
    let loaded = h3_project::load_session(project).unwrap();
    assert!(loaded.session["flux_reference_builder"]["subjects"][0]["image"]["data"].as_str().unwrap().starts_with("data:image/jpeg;base64,"));

    let (code, body) = s.call(Method::PUT, &format!("{PROJECT}/references/subjects/nobody/image"), &[], Some(json!({"image_data": png_base64()}))).await;
    assert_eq!((code, body["error"]["code"].as_str()), (400, Some("VALIDATION_ERROR")));
    let (code, _) = s.call(Method::POST, &format!("{PROJECT}/references/things/x/image/generate"), &[], None).await;
    assert_eq!(code, 400);
}

#[tokio::test]
async fn an_image_job_fails_cleanly_when_comfyui_is_down() {
    let s = start().await;
    let (code, body) = s.call(Method::POST, &format!("{PROJECT}/scenes/1/image/generate"), &[], Some(json!({"prompt": "a lake"}))).await;
    assert_eq!(code, 202, "{body}");
    let job = s.wait_job(body["data"]["job_id"].as_str().unwrap(), "failed").await;
    assert_eq!(job["error"]["code"], "COMFY_UNAVAILABLE");
    assert_eq!(job["type"], "image.generate");
    let (code, body) = s.call(Method::POST, &format!("{PROJECT}/references/subjects/subj1/image/generate"), &[], Some(json!({"preset_id": "flux_klein"}))).await;
    assert_eq!(code, 202);
    let job = s.wait_job(body["data"]["job_id"].as_str().unwrap(), "failed").await;
    assert_eq!(job["error"]["code"], "COMFY_UNAVAILABLE");
}
