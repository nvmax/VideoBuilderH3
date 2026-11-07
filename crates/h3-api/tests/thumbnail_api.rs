//! The Thumbnail routes of the Builder through the real router: what the build can do, keeping a picture ComfyUI made, and the intro that
//! goes in front of a stitched video (with a real ffmpeg). The prompt route needs an LLM, so only its answer to a request without a picture is checked here.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use h3_project::AppConfig;
use reqwest::Client;
use serde_json::{json, Value};

struct Server {
    root: String,
    client: Client,
    dir: tempfile::TempDir,
}

/// The API with ComfyUI's output folder set in the config, so no ComfyUI is needed to find it.
async fn start() -> Server {
    let dir = tempfile::tempdir().unwrap();
    let projects = dir.path().join("projects");
    std::fs::create_dir_all(&projects).unwrap();
    let cfg = AppConfig {
        comfy_url: "http://127.0.0.1:9".into(),
        comfy_output_dir: dir.path().join("output").display().to_string(),
        project_root: projects,
        ..AppConfig::default()
    };
    let state = h3_api::build_state(cfg, &dir.path().join("jobs.db")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, h3_api::router(state)).await.unwrap() });
    Server { root: format!("http://{addr}/vrgdg/music_builder/thumbnail"), client: Client::builder().timeout(Duration::from_secs(120)).build().unwrap(), dir }
}

impl Server {
    async fn post(&self, route: &str, body: Value) -> (u16, Value) {
        let response = self.client.post(format!("{}/{route}", self.root)).json(&body).send().await.unwrap();
        (response.status().as_u16(), response.json().await.unwrap())
    }
}

fn make_video(path: &Path) {
    let status = Command::new("ffmpeg")
        .args(["-y", "-f", "lavfi", "-i", "testsrc2=s=160x90:r=24:d=3", "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=3", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest"])
        .arg(path)
        .output()
        .expect("ffmpeg is on the PATH")
        .status;
    assert!(status.success());
}

/// How many video frames a file holds, counted from its packets.
fn frame_count(path: &Path) -> i64 {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-count_packets", "-select_streams", "v:0", "-show_entries", "stream=nb_read_packets", "-of", "default=noprint_wrappers=1:nokey=1"])
        .arg(path)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
}

#[tokio::test]
async fn the_build_says_it_can_make_thumbnails() {
    let server = start().await;
    let answer: Value = server.client.get(format!("{}/capabilities", server.root)).send().await.unwrap().json().await.unwrap();
    assert_eq!(answer, json!({"ok": true, "version": 1, "frame": true}));
}

#[tokio::test]
async fn one_exact_frame_of_a_scene_video_is_kept_in_the_project() {
    let server = start().await;
    let project = server.dir.path().join("projects").join("Demo");
    std::fs::create_dir_all(&project).unwrap();
    let video = project.join("video_0001-audio.mp4");
    make_video(&video);

    let (status, frame) = server.post("frame", json!({"project_folder": project.display().to_string(), "video_path": video.display().to_string(), "seconds": 1.0})).await;
    assert_eq!(status, 200, "{frame}");
    assert_eq!((frame["ok"].clone(), frame["frame"].clone(), frame["fps"].clone()), (json!(true), json!(24), json!(24.0)));
    let path = frame["image_path"].as_str().unwrap();
    assert!(path.ends_with("source_0001.png") && Path::new(path).is_file(), "{frame}");
    assert_eq!((frame["width"].clone(), frame["height"].clone()), (json!(160), json!(90)));

    let (status, missing) = server.post("frame", json!({"project_folder": project.display().to_string(), "video_path": "Y:\\nowhere\\a.mp4", "seconds": 1})).await;
    assert_eq!(status, 400);
    assert_eq!(missing["ok"], false);
    assert!(missing["error"].as_str().unwrap().starts_with("The video to take the frame from was not found"));
}

#[tokio::test]
async fn a_picture_comfyui_made_is_kept_in_the_project_and_goes_in_front_of_the_video() {
    let server = start().await;
    let output = server.dir.path().join("output");
    std::fs::create_dir_all(&output).unwrap();
    image::RgbImage::from_pixel(320, 180, image::Rgb([220, 30, 30])).save(output.join("gen.png")).unwrap();
    let project = server.dir.path().join("projects").join("Demo");
    std::fs::create_dir_all(&project).unwrap();

    let (status, saved) = server.post("save", json!({"project_folder": project.display().to_string(), "image": {"filename": "gen.png", "subfolder": "", "type": "output"}})).await;
    assert_eq!(status, 200, "{saved}");
    assert_eq!(saved["ok"], true);
    let thumbnail = saved["image_path"].as_str().unwrap().to_string();
    assert!(thumbnail.ends_with("thumbnail_0001.png") && Path::new(&thumbnail).is_file(), "{saved}");

    let video = project.join("FINAL_VIDEO.mp4");
    make_video(&video);
    assert_eq!(frame_count(&video), 72);
    // The picture is the first frame of the video: one frame, whatever length the request says.
    let (status, joined) = server.post("prepend", json!({"input_path": video.display().to_string(), "image_path": thumbnail, "seconds": 5})).await;
    assert_eq!(status, 200, "{joined}");
    assert_eq!(joined["final_video_path"], video.display().to_string());
    assert_eq!(joined["intro_frames"], 1);
    assert_eq!(frame_count(&video), 73, "{joined}");
}

#[tokio::test]
async fn bad_requests_answer_with_the_error_envelope() {
    let server = start().await;
    let (status, missing) = server.post("prepend", json!({"input_path": "Y:\\nowhere\\a.mp4", "image_path": "Y:\\nowhere\\a.png"})).await;
    assert_eq!(status, 400);
    assert_eq!(missing["ok"], false);
    assert!(missing["error"].as_str().unwrap().starts_with("The video to put the thumbnail in front of was not found"));

    let (status, empty) = server.post("save", json!({"project_folder": "", "source_path": "x.png"})).await;
    assert_eq!(status, 400);
    assert_eq!(empty["error"], "Project folder is empty.");

    let (status, no_picture) = server.post("prompt", json!({"title": "x"})).await;
    assert_eq!(status, 500);
    assert!(no_picture["error"].as_str().unwrap().starts_with("Choose a scene with an image first"));
}
