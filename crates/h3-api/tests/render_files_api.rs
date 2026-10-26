//! An agent makes a RefMod from pictures and splits the song into stems: a request that cannot work is refused before a job exists, and a job that
//! cannot run says why.

use std::time::Duration;

use h3_project::AppConfig;
use reqwest::{Client, Method};
use serde_json::{json, Value};

struct Server {
    base: String,
    client: Client,
    dir: tempfile::TempDir,
}

async fn start() -> Server {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("projects");
    let with_song = root.join("Song");
    let without_song = root.join("Silent");
    std::fs::create_dir_all(&with_song).unwrap();
    std::fs::create_dir_all(&without_song).unwrap();
    let song = dir.path().join("song.wav");
    std::fs::write(&song, b"RIFF").unwrap();
    let session = |audio: Value| json!({"project_name": "x", "revision": 1, "video_engine": "minimax_h3", "audio_path": audio, "segments": []});
    std::fs::write(with_song.join("vrgdg_builder_session.json"), session(json!(song.display().to_string())).to_string()).unwrap();
    std::fs::write(without_song.join("vrgdg_builder_session.json"), session(json!("")).to_string()).unwrap();
    // Nothing listens on port 9, so ComfyUI is down.
    let cfg = AppConfig { comfy_url: "http://127.0.0.1:9".into(), project_root: root, ..AppConfig::default() };
    let state = h3_api::build_state(cfg, &dir.path().join("jobs.db")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, h3_api::router(state)).await.unwrap() });
    Server { base: format!("http://{addr}/api/v1"), client: Client::builder().timeout(Duration::from_secs(30)).build().unwrap(), dir }
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

    async fn finished(&self, started: &Value) -> Value {
        let id = started["data"]["job_id"].as_str().unwrap().to_string();
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
async fn a_refmod_request_that_cannot_work_is_refused_before_a_job_exists() {
    let s = start().await;
    let picture = s.dir.path().join("darrel.png");
    image::RgbImage::new(64, 64).save(&picture).unwrap();
    let picture = picture.display().to_string();
    let bad = [
        json!({}),
        json!({"name": "x", "type": "vehicle"}),
        json!({"name": "x", "type": "spaceship", "paths": [picture]}),
        json!({"name": "", "type": "vehicle", "paths": [picture]}),
        json!({"name": "x", "type": "vehicle", "paths": [s.dir.path().join("missing.png").display().to_string()]}),
        json!({"name": "x", "type": "vehicle", "paths": [picture], "quality": "perfect"}),
        json!({"name": "x", "type": "vehicle", "paths": [picture], "mode": "Half Reference"}),
        json!({"name": "x", "type": "vehicle", "paths": [picture], "steps": 9000}),
        json!({"name": "x", "type": "vehicle", "paths": [picture], "crops": [null, null]}),
    ];
    for body in bad {
        let (code, answer) = s.call(Method::POST, "/refmods", Some(body.clone())).await;
        assert_eq!((code, answer["error"]["code"].as_str()), (400, Some("VALIDATION_ERROR")), "{body}: {answer}");
    }
    let (_, jobs) = s.call(Method::GET, "/jobs", None).await;
    assert!(jobs["data"].as_array().is_none_or(Vec::is_empty), "no job was made: {jobs}");

    // A good request becomes a job. ComfyUI is down, so the job says so.
    let (code, started) = s.call(Method::POST, "/refmods", Some(json!({"name": "C8_Corvette", "type": "vehicle", "paths": [picture]}))).await;
    assert_eq!(code, 202, "{started}");
    let job = s.finished(&started).await;
    assert_eq!((job["type"].as_str(), job["status"].as_str()), (Some("refmod.create"), Some("failed")), "{job}");
    assert!(job["error"]["message"].as_str().is_some_and(|m| !m.is_empty()), "{job}");
}

#[tokio::test]
async fn the_song_is_split_by_a_job_and_a_project_without_a_song_is_refused() {
    let s = start().await;
    let (code, answer) = s.call(Method::POST, "/projects/Silent/audio/stems", Some(json!({}))).await;
    assert_eq!((code, answer["error"]["code"].as_str()), (400, Some("VALIDATION_ERROR")), "{answer}");
    assert!(answer["error"]["message"].as_str().unwrap().contains("no song to split"), "{answer}");
    let (code, answer) = s.call(Method::POST, "/projects/Nope/audio/stems", Some(json!({}))).await;
    assert_eq!((code, answer["error"]["code"].as_str()), (404, Some("PROJECT_NOT_FOUND")), "{answer}");
    let (code, answer) = s.call(Method::POST, "/projects/Silent/audio/stems", Some(json!({"audio_path": s.dir.path().join("missing.wav").display().to_string()}))).await;
    assert_eq!(code, 400, "{answer}");

    // A song that is there becomes a GPU job. A model that does not exist, and ComfyUI being down, each fail it with a reason.
    let (code, started) = s.call(Method::POST, "/projects/Song/audio/stems", Some(json!({"model_name": "demucs9"}))).await;
    assert_eq!(code, 202, "{started}");
    let job = s.finished(&started).await;
    assert_eq!((job["type"].as_str(), job["status"].as_str(), job["error"]["code"].as_str()), (Some("audio.stems"), Some("failed"), Some("VALIDATION_ERROR")), "{job}");
    assert!(job["error"]["message"].as_str().unwrap().contains("Unknown Demucs model"), "{job}");
    let (_, started) = s.call(Method::POST, "/projects/Song/audio/stems", Some(json!({}))).await;
    let job = s.finished(&started).await;
    assert_eq!(job["status"], "failed", "{job}");
}
