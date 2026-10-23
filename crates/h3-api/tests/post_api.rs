//! LUT grading through the API with a real clip: upload an invert LUT, grade a red scene, check the pixels.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use h3_project::AppConfig;
use reqwest::{Client, Method};
use serde_json::{json, Value};

const INVERT_CUBE: &str = "TITLE \"Invert\"\nLUT_3D_SIZE 2\n1 1 1\n0 1 1\n1 0 1\n0 0 1\n1 1 0\n0 1 0\n1 0 0\n0 0 0\n";

fn make_video(path: &Path, color: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let source = format!("color=c={color}:s=64x64:d=2:r=24");
    let status = Command::new("ffmpeg").args(["-y", "-f", "lavfi", "-i", &source, "-pix_fmt", "yuv420p"]).arg(path).output().expect("ffmpeg is on the PATH").status;
    assert!(status.success());
}

/// The middle pixel of the first frame.
fn first_pixel(video: &Path, scratch: &Path) -> [u8; 3] {
    let frame = scratch.join("frame.png");
    let status = Command::new("ffmpeg").args(["-y", "-i"]).arg(video).args(["-frames:v", "1"]).arg(&frame).output().unwrap().status;
    assert!(status.success());
    let image = image::open(&frame).unwrap().to_rgb8();
    image.get_pixel(32, 32).0
}

struct Server {
    base: String,
    client: Client,
    project: PathBuf,
    dir: tempfile::TempDir,
}

async fn start() -> Server {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("projects");
    let project = root.join("Demo");
    std::fs::create_dir_all(&project).unwrap();
    let clip1 = project.join("rendered_scene_videos").join("video_0001-audio.mp4");
    let clip2 = project.join("rendered_scene_videos").join("video_0002-audio.mp4");
    make_video(&clip1, "red");
    make_video(&clip2, "red");
    let session = json!({
        "project_name": "Demo", "revision": 1, "video_engine": "minimax_h3",
        "segments": [
            {"id": "a", "start": 0.0, "end": 2.0, "label": "Scene 1", "video_path": clip1.display().to_string()},
            {"id": "b", "start": 2.0, "end": 4.0, "label": "Scene 2", "video_path": clip2.display().to_string()},
            {"id": "c", "start": 4.0, "end": 6.0, "label": "Scene 3"}
        ]
    });
    std::fs::write(project.join("vrgdg_builder_session.json"), serde_json::to_string_pretty(&session).unwrap()).unwrap();
    let cfg = AppConfig { project_root: root, ..AppConfig::default() };
    let state = h3_api::build_state(cfg, &dir.path().join("jobs.db")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, h3_api::router(state)).await.unwrap() });
    Server { base: format!("http://{addr}/api/v1"), client: Client::builder().timeout(Duration::from_secs(60)).build().unwrap(), project, dir }
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
async fn a_lut_is_uploaded_and_grades_scene_clips() {
    let s = start().await;
    let (code, body) = s.call(Method::GET, "/post/luts", None).await;
    assert_eq!((code, body["data"]["luts"].as_array().map(Vec::len)), (200, Some(0)));

    // Upload as raw body, a wrong type and as JSON.
    let response = s.client.post(format!("{}/post/luts/upload?filename=Invert.cube", s.base)).body(INVERT_CUBE).send().await.unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let (code, _) = s.call(Method::POST, "/post/luts/upload?filename=notes.txt", None).await;
    assert_eq!(code, 400);
    let encoded = {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(INVERT_CUBE)
    };
    let (code, body) = s.call(Method::POST, "/post/luts/upload", Some(json!({"filename": "Second_Look.cube", "data": encoded}))).await;
    assert_eq!(code, 200, "{body}");
    let (_, body) = s.call(Method::GET, "/post/luts", None).await;
    let names: Vec<&str> = body["data"]["luts"].as_array().unwrap().iter().map(|l| l["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Invert.cube", "Second_Look.cube"]);
    assert_eq!(body["data"]["luts"][1]["label"], "Second Look");

    // Full strength inverts red to cyan.
    let job = s.job("/projects/Demo/scenes/a/post/lut", json!({"lut_name": "Invert.cube", "strength": 10})).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    let graded = PathBuf::from(job["result"]["video_path"].as_str().unwrap());
    assert!(graded.is_file() && graded.file_name().unwrap().to_string_lossy().starts_with("video_0001-lut_"));
    let [r, g, b] = first_pixel(&graded, s.dir.path());
    assert!(r < 90 && g > 160 && b > 160, "{r} {g} {b}");
    assert_eq!(s.scene(0)["video_path"].as_str().unwrap(), graded.display().to_string());
    assert!(job["result"]["history_count"].as_u64().unwrap() >= 1);

    // Half strength blends with the source: neither red nor cyan.
    let job = s.job("/projects/Demo/scenes/b/post/lut", json!({"lut_name": "Invert.cube", "strength": 5})).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    let [r, g, b] = first_pixel(Path::new(job["result"]["video_path"].as_str().unwrap()), s.dir.path());
    assert!((70..190).contains(&r) && (70..190).contains(&g) && b > 60, "{r} {g} {b}");

    // Things that are refused.
    for (scene, body, wanted) in [
        ("c", json!({"lut_name": "Invert.cube"}), "has no rendered video"),
        ("a", json!({"lut_name": "Missing.cube"}), "LUT file was not found"),
        ("a", json!({"lut_name": ""}), "LUT name is required"),
        ("a", json!({"lut_name": "Invert.cube", "media_type": "image"}), "not supported"),
    ] {
        let job = s.job(&format!("/projects/Demo/scenes/{scene}/post/lut"), body).await;
        assert_eq!((job["status"].as_str(), job["error"]["code"].as_str()), (Some("failed"), Some("VALIDATION_ERROR")), "{job}");
        assert!(job["error"]["message"].as_str().unwrap().contains(wanted), "{job}");
    }

    // The same grade on every scene that has a clip; adjust and grain are refused.
    let job = s.job("/projects/Demo/post/apply-all", json!({"stack": {"lut": {"lut_name": "Second_Look.cube", "strength": 10}}})).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    assert_eq!(job["result"]["count"], 2);
    let job = s.job("/projects/Demo/post/apply-all", json!({"stack": {"film_grain": {"enabled": true}}})).await;
    assert_eq!(job["error"]["code"], "VALIDATION_ERROR");
}
