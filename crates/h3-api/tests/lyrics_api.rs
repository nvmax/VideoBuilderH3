//! Lyric timing and scenes from timed lines through the API, against a small stand-in for ComfyUI.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use h3_project::AppConfig;
use reqwest::{Client, Method};
use serde_json::{json, Value};

const LYRICS: &str = "[Verse 1]\nWalking down the empty street\nLights are fading under my feet\n\n[Chorus]\nSing it loud tonight\nHold the morning light";

struct Server {
    base: String,
    client: Client,
    dir: tempfile::TempDir,
    queued: Arc<Mutex<Vec<Value>>>,
}

fn line(text: &str, start: f64, end: f64) -> Value {
    json!({"text": text, "start": start, "end": end, "type": "vocal", "words": []})
}

async fn start() -> Server {
    let queued: Arc<Mutex<Vec<Value>>> = Arc::default();
    let timed = json!({"duration": 30.0, "segments": [
        line("Walking down the empty street", 1.0, 5.0),
        line("Lights are fading under my feet", 5.0, 9.5),
        line("Sing it loud tonight", 14.0, 19.0),
        line("Hold the morning light", 19.0, 24.0),
    ]})
    .to_string();
    let comfy_dir = tempfile::tempdir().unwrap();
    let main_py = comfy_dir.path().join("ComfyUI").join("main.py").display().to_string();
    let comfy = Router::new()
        .route("/system_stats", get(move || async move { Json(json!({"system": {"argv": [main_py]}, "devices": []})) }))
        .route("/object_info", get(|| async { Json(json!({"VRGDG_TimestampedLyricsExtractor": {}, "VRGDG_GetStems": {}, "VRGDG_ShowText": {}})) }))
        .route(
            "/prompt",
            post(|State(seen): State<Arc<Mutex<Vec<Value>>>>, Json(body): Json<Value>| async move {
                seen.lock().unwrap().push(body["prompt"].clone());
                Json(json!({"prompt_id": "p1", "number": 1, "node_errors": {}}))
            }),
        )
        .route("/history/{id}", get(move |axum::extract::Path(id): axum::extract::Path<String>| {
            let timed = timed.clone();
            async move { Json(json!({ id: {"status": {"status_str": "success", "messages": []}, "outputs": {"963": {"text": [timed]}}} })) }
        }))
        .with_state(Arc::clone(&queued));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let comfy_url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, comfy).await.unwrap() });

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("projects");
    let project = root.join("Demo");
    std::fs::create_dir_all(&project).unwrap();
    let song = project.join("song.wav");
    std::fs::write(&song, b"RIFF").unwrap();
    let session = json!({
        "project_name": "Demo", "revision": 1, "video_engine": "minimax_h3", "audio_path": song.display().to_string(),
        "lyric_mapper": {"source_text": LYRICS, "lines": []}, "segments": []
    });
    std::fs::write(project.join("vrgdg_builder_session.json"), serde_json::to_string_pretty(&session).unwrap()).unwrap();
    let cfg = AppConfig { comfy_url, project_root: root, ..AppConfig::default() };
    let state = h3_api::build_state(cfg, &dir.path().join("jobs.db")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, h3_api::router(state)).await.unwrap() });
    // Keep the temp folder of the stand-in alive for the whole test.
    std::mem::forget(comfy_dir);
    Server { base: format!("http://{addr}/api/v1"), client: Client::builder().timeout(Duration::from_secs(30)).build().unwrap(), dir, queued }
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

    async fn job(&self, path: &str, body: Value) -> Value {
        let (code, response) = self.call(Method::POST, path, Some(body)).await;
        assert_eq!(code, 202, "{response}");
        let id = response["data"]["job_id"].as_str().unwrap().to_string();
        for _ in 0..200 {
            let (_, body) = self.call(Method::GET, &format!("/jobs/{id}"), None).await;
            if ["succeeded", "failed", "cancelled"].contains(&body["data"]["status"].as_str().unwrap_or("")) {
                return body["data"].clone();
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("job {id} did not finish");
    }

    fn segments(&self) -> Vec<Value> {
        let text = std::fs::read_to_string(self.dir.path().join("projects/Demo/vrgdg_builder_session.json")).unwrap();
        serde_json::from_str::<Value>(&text).unwrap()["segments"].as_array().cloned().unwrap_or_default()
    }
}

#[tokio::test]
async fn lyrics_are_timed_and_become_scenes() {
    let s = start().await;

    let job = s.job("/projects/Demo/lyrics/align", json!({"segment_mode": "reference_lines"})).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    assert_eq!(job["result"]["lines"], 4);
    let saved = s.dir.path().join("projects/Demo/project_context/timestamped_lyrics.json");
    assert!(saved.is_file());
    let graph = s.queued.lock().unwrap()[0].clone();
    assert_eq!(graph["962"]["inputs"]["segment_mode"], "reference_lines");
    assert!(graph["962"]["inputs"]["reference_lyrics"].as_str().unwrap().contains("Walking down the empty street"));
    assert!(graph["28:114"]["inputs"]["audio_file_path"].as_str().unwrap().ends_with("song.wav"));

    let job = s.job("/projects/Demo/timeline/from-lines", json!({"min_scene_seconds": 2.0, "max_scene_seconds": 8.0, "use_saved_alignment": true})).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    let scenes = s.segments();
    assert_eq!(job["result"]["scenes"].as_u64().unwrap() as usize, scenes.len());
    assert!(scenes.len() >= 4);
    assert!(scenes.iter().any(|x| x["lyric_text"] == "Sing it loud tonight" && x["lyric_section"] == "Chorus"));
    assert!(scenes.iter().any(|x| x["lyric_no_lip_sync"] == true), "the gap becomes an instrumental scene");
    assert!(scenes.windows(2).all(|w| w[0]["end"].as_f64().unwrap() <= w[1]["start"].as_f64().unwrap() + 1e-6));
    assert_eq!(s.queued.lock().unwrap().len(), 1, "a saved alignment is not timed again");

    // A project that has scenes needs replace_existing.
    let job = s.job("/projects/Demo/timeline/from-lines", json!({"use_saved_alignment": true})).await;
    assert_eq!((job["status"].as_str(), job["error"]["code"].as_str()), (Some("failed"), Some("VALIDATION_ERROR")));
    let job = s.job("/projects/Demo/timeline/from-lines", json!({"use_saved_alignment": true, "replace_existing": true, "segment_mode": "nonsense"})).await;
    assert_eq!(job["error"]["code"], "VALIDATION_ERROR");
    let job = s.job("/projects/Demo/timeline/from-lines", json!({"replace_existing": true})).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    assert_eq!(s.queued.lock().unwrap().len(), 2, "without use_saved_alignment the song is timed again");
}
