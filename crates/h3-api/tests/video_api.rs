//! Takes, re-trim, choosing a clip, clearing it and the opening color match, through the API with real ffmpeg clips.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use h3_project::AppConfig;
use reqwest::{Client, Method};
use serde_json::{json, Value};

fn make_video(path: &Path, color: &str, seconds: u32) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let source = format!("color=c={color}:s=64x64:d={seconds}:r=24");
    let status = Command::new("ffmpeg").args(["-y", "-f", "lavfi", "-i", &source, "-pix_fmt", "yuv420p"]).arg(path).output().expect("ffmpeg is on the PATH").status;
    assert!(status.success());
}

fn duration(path: &Path) -> f64 {
    let out = Command::new("ffprobe").args(["-v", "error", "-show_entries", "format=duration", "-of", "default=noprint_wrappers=1:nokey=1"]).arg(path).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
}

struct Server {
    base: String,
    client: Client,
    project: PathBuf,
    output: PathBuf,
}

async fn start() -> (Server, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("projects");
    let project = root.join("Demo");
    let output = dir.path().join("comfy_output");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&output).unwrap();
    // Two raw takes in the scene's scratch folder, and a recorded one that is gone.
    let (scratch, _) = h3_graph::inputs::output_location_in(&output.display().to_string(), &project.display().to_string(), 1);
    let (older, newer) = (Path::new(&scratch).join("MiniMaxH3_scene_0001_00001-audio.mp4"), Path::new(&scratch).join("MiniMaxH3_scene_0001_00002-audio.mp4"));
    make_video(&older, "green", 3);
    std::thread::sleep(Duration::from_millis(1100));
    make_video(&newer, "yellow", 4);
    let clip1 = project.join("rendered_scene_videos").join("video_0001-audio.mp4");
    let clip2 = project.join("rendered_scene_videos").join("video_0002-audio.mp4");
    make_video(&clip1, "red", 2);
    make_video(&clip2, "blue", 2);
    let session = json!({
        "project_name": "Demo", "revision": 1, "video_engine": "minimax_h3",
        "segments": [
            {"id": "a", "start": 0.0, "end": 2.0, "label": "Scene 1", "video_path": clip1.display().to_string(),
             "raw_video_history": [older.display().to_string(), project.join("gone.mp4").display().to_string()], "raw_video_path": newer.display().to_string()},
            {"id": "b", "start": 2.0, "end": 4.0, "label": "Scene 2", "video_path": clip2.display().to_string()}
        ]
    });
    std::fs::write(project.join("vrgdg_builder_session.json"), serde_json::to_string_pretty(&session).unwrap()).unwrap();
    let cfg = AppConfig { comfy_url: "http://127.0.0.1:9".into(), comfy_output_dir: output.display().to_string(), project_root: root, ..AppConfig::default() };
    let state = h3_api::build_state(cfg, &dir.path().join("jobs.db")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, h3_api::router(state)).await.unwrap() });
    (Server { base: format!("http://{addr}/api/v1"), client: Client::builder().timeout(Duration::from_secs(60)).build().unwrap(), project, output }, dir)
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
        for _ in 0..400 {
            let (_, body) = self.call(Method::GET, &format!("/jobs/{id}"), None).await;
            if ["succeeded", "failed", "cancelled"].contains(&body["data"]["status"].as_str().unwrap_or("")) {
                return body["data"].clone();
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("job {id} did not finish");
    }

    fn scene(&self, index: usize) -> Value {
        let text = std::fs::read_to_string(self.project.join("vrgdg_builder_session.json")).unwrap();
        serde_json::from_str::<Value>(&text).unwrap()["segments"][index].clone()
    }
}

#[tokio::test]
async fn takes_are_listed_trimmed_chosen_and_cleared() {
    let (s, _dir) = start().await;
    let _ = &s.output;

    // Takes, newest first, with the recorded file that is gone marked as such.
    let (code, listing) = s.call(Method::GET, "/projects/Demo/scenes/a/video/takes", None).await;
    assert_eq!(code, 200, "{listing}");
    let takes = listing["data"]["takes"].as_array().unwrap();
    assert_eq!(takes.len(), 3, "{listing}");
    assert_eq!(listing["data"]["usable"], 2);
    assert!(takes[0]["name"].as_str().unwrap().ends_with("00002-audio.mp4") && takes[0]["is_latest_render"] == true);
    assert_eq!(takes[0]["frame_count"], 96);
    assert!(takes.iter().any(|t| t["exists"] == false && t["recorded"] == true));
    let (code, _) = s.call(Method::GET, "/projects/Demo/scenes/nope/video/takes", None).await;
    assert_eq!(code, 404);

    // Re-trim the latest take: one second from half a second in, saved as the scene's clip.
    let job = s.job("/projects/Demo/scenes/a/video/trim", json!({"take": "latest", "start": 0.5, "duration": 1.0, "frames": 24})).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    let trimmed = PathBuf::from(job["result"]["video_path"].as_str().unwrap());
    assert!(trimmed.is_file() && trimmed.file_name().unwrap().to_string_lossy().contains("retrim") && trimmed.to_string_lossy().ends_with("-audio.mp4"));
    assert!((duration(&trimmed) - 1.0).abs() < 0.2, "{}", duration(&trimmed));
    assert_eq!(s.scene(0)["video_path"].as_str().unwrap(), trimmed.display().to_string());

    // To the end of an older take by index.
    let job = s.job("/projects/Demo/scenes/a/video/trim", json!({"take": 1, "start": 1.0, "to_end": true})).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    assert!((duration(Path::new(job["result"]["video_path"].as_str().unwrap())) - 2.0).abs() < 0.3);
    let job = s.job("/projects/Demo/scenes/a/video/trim", json!({"take": 2})).await;
    assert_eq!((job["status"].as_str(), job["error"]["code"].as_str()), (Some("failed"), Some("VALIDATION_ERROR")), "{job}");
    assert!(job["error"]["message"].as_str().unwrap().contains("recorded but its file is gone"));
    let job = s.job("/projects/Demo/scenes/a/video/trim", json!({"take": 9})).await;
    assert!(job["error"]["message"].as_str().unwrap().contains("has no take 9"));

    // Choose a clip: it becomes video_0001-audio.mp4 and the clip that was there goes to the backup folder.
    let chosen = trimmed.display().to_string();
    let (code, body) = s.call(Method::POST, "/projects/Demo/scenes/a/video/select", Some(json!({"source_path": chosen}))).await;
    assert_eq!(code, 200, "{body}");
    let target = s.project.join("rendered_scene_videos").join("video_0001-audio.mp4");
    assert_eq!(body["data"]["video_path"].as_str().unwrap(), target.display().to_string());
    assert!(std::fs::read_dir(s.project.join("rendered_scene_videos_backup/scene_0001")).unwrap().count() == 1);
    let (code, _) = s.call(Method::POST, "/projects/Demo/scenes/a/video/select", Some(json!({"source_path": "Z:\\nowhere.mp4"}))).await;
    assert_eq!(code, 400);
    let (code, _) = s.call(Method::POST, "/projects/Demo/scenes/a/video/select", Some(json!({}))).await;
    assert_eq!(code, 400);
    let (code, _) = s.call(Method::POST, "/projects/Demo/scenes/b/video/recover", Some(json!({}))).await;
    assert_eq!(code, 400, "nothing to recover");

    // Clear the clip, the files stay.
    let (code, body) = s.call(Method::DELETE, "/projects/Demo/scenes/a/video", None).await;
    assert_eq!((code, body["data"]["cleared"].as_bool()), (200, Some(true)));
    assert_eq!(s.scene(0)["video_path"], "");
    assert!(target.is_file());
}

#[tokio::test]
async fn the_opening_color_is_matched_to_the_previous_scene() {
    let (s, _dir) = start().await;
    let before = std::fs::metadata(s.scene(1)["video_path"].as_str().unwrap()).unwrap().len();
    let job = s.job("/projects/Demo/scenes/b/video/match-start-color", json!({"fade_seconds": 1.0, "strength": 0.9})).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    let clip = job["result"]["video_path"].as_str().unwrap();
    assert!(std::fs::metadata(clip).unwrap().len() != before, "the clip was re-encoded");
    let job = s.job("/projects/Demo/scenes/a/video/match-start-color", json!({})).await;
    assert_eq!((job["status"].as_str(), job["error"]["code"].as_str()), (Some("failed"), Some("VALIDATION_ERROR")), "scene 1 has no previous scene");
}
