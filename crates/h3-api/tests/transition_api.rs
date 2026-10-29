//! The transition routes of the agent API: the catalog, a project's transitions as blocks of the timeline, and a stitch that draws them.
//! Needs ffmpeg and ffprobe on the PATH and a graphics adapter.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use axum::routing::get;
use axum::{Json, Router};
use h3_project::AppConfig;
use reqwest::{Client, Method};
use serde_json::{json, Value};

fn make_video(path: &Path, color: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let video = format!("color=c={color}:s=128x72:d=2:r=24");
    let status = Command::new("ffmpeg")
        .args(["-y", "-f", "lavfi", "-i", &video, "-f", "lavfi", "-i", "sine=frequency=440:d=2", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest"])
        .arg(path)
        .output()
        .expect("ffmpeg is on the PATH")
        .status;
    assert!(status.success());
}

/// Share of the pixels of the frame at `at` seconds that are clearly red and clearly blue.
fn red_and_blue(video: &Path, scratch: &Path, at: f64) -> (f64, f64) {
    let frame = scratch.join(format!("frame_{at}.png"));
    let status = Command::new("ffmpeg").args(["-y", "-ss", &at.to_string(), "-i"]).arg(video).args(["-frames:v", "1"]).arg(&frame).output().unwrap().status;
    assert!(status.success());
    let image = image::open(&frame).unwrap().to_rgb8();
    let (mut red, mut blue) = (0usize, 0usize);
    for p in image.pixels() {
        let [r, g, b] = p.0;
        if r > 170 && g < 80 && b < 80 {
            red += 1;
        }
        if b > 170 && r < 80 && g < 80 {
            blue += 1;
        }
    }
    let n = (image.width() * image.height()) as f64;
    (red as f64 / n, blue as f64 / n)
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
    make_video(&clip2, "blue");
    let session = json!({
        "project_name": "Demo", "revision": 1, "video_engine": "minimax_h3",
        "segments": [
            {"id": "a", "start": 0.0, "end": 2.0, "label": "Scene 1", "lyric_text": "first line", "video_path": clip1.display().to_string()},
            {"id": "b", "start": 2.0, "end": 4.0, "label": "Scene 2", "lyric_text": "second line", "video_path": clip2.display().to_string()}
        ]
    });
    std::fs::write(project.join("vrgdg_builder_session.json"), serde_json::to_string_pretty(&session).unwrap()).unwrap();
    let comfy = comfy_stand_in(dir.path()).await;
    let cfg = AppConfig { comfy_url: comfy, project_root: root, ..AppConfig::default() };
    let state = h3_api::build_state(cfg, &dir.path().join("jobs.db")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, h3_api::router(state)).await.unwrap() });
    Server { base: format!("http://{addr}/api/v1"), client: Client::builder().timeout(Duration::from_secs(120)).build().unwrap(), project, dir }
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

    async fn ok(&self, method: Method, path: &str, body: Option<Value>) -> Value {
        let (code, response) = self.call(method, path, body).await;
        assert_eq!(code, 200, "{path}: {response}");
        response["data"].clone()
    }

    async fn stitch(&self, body: Value) -> Value {
        let (code, response) = self.call(Method::POST, "/projects/Demo/stitch", Some(body)).await;
        assert_eq!(code, 202, "{response}");
        let id = response["data"]["job_id"].as_str().unwrap().to_string();
        for _ in 0..600 {
            let (_, body) = self.call(Method::GET, &format!("/jobs/{id}"), None).await;
            let status = body["data"]["status"].as_str().unwrap_or("").to_string();
            if status == "succeeded" {
                return body["data"]["result"].clone();
            }
            assert!(!["failed", "cancelled"].contains(&status.as_str()), "the stitch failed: {body}");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("job {id} did not finish");
    }

    fn session(&self) -> Value {
        serde_json::from_str(&std::fs::read_to_string(self.project.join("vrgdg_builder_session.json")).unwrap()).unwrap()
    }
}

async fn comfy_stand_in(root: &Path) -> String {
    let folders = json!({"custom_nodes": [root.join("ComfyUI").join("custom_nodes").display().to_string()]});
    let app = Router::new()
        .route("/system_stats", get(|| async { Json(json!({"system": {"os": "nt", "comfyui_version": "0.9.9", "argv": ["main.py"]}, "devices": [{"name": "Test GPU", "vram_total": 25769803776u64, "vram_free": 1}]})) }))
        .route("/internal/folder_paths", get(move || async move { Json(folders) }))
        .route("/object_info", get(|| async { Json(json!({})) }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

#[tokio::test]
async fn the_catalog_lists_transitions_with_their_settings() {
    let s = start().await;
    let data = s.ok(Method::GET, "/transitions", None).await;
    assert!(data["count"].as_u64().unwrap() > 100);
    let warp = data["transitions"].as_array().unwrap().iter().find(|t| t["id"] == "DirectionalWarp").expect("DirectionalWarp is listed");
    assert_eq!(warp["license"], "MIT");
    let direction = warp["params"].as_array().unwrap().iter().find(|p| p["name"] == "direction").expect("it has a direction setting");
    assert_eq!(direction["type"], "vec2");
    assert!(direction["default"].is_array());
}

#[tokio::test]
async fn an_agent_adds_changes_and_removes_the_transitions_on_the_timeline() {
    let s = start().await;
    assert_eq!(s.ok(Method::GET, "/projects/Demo/transitions", None).await["count"], json!(0));

    // A block of the timeline, with an id of its own.
    let added = s.ok(Method::POST, "/projects/Demo/transitions/set", Some(json!({"transition": "WipeLeft", "start": 1.75, "seconds": 0.5}))).await;
    let block = &added["scene_transitions"][0];
    assert_eq!((block["id"].clone(), block["start"].clone(), block["end"].clone()), (json!("t1"), json!(1.75), json!(2.25)));
    assert_eq!(s.session()["scene_transitions"][0]["transition"], "WipeLeft");

    // Only the fields sent change.
    let longer = s.ok(Method::POST, "/projects/Demo/transitions/set", Some(json!({"id": "t1", "seconds": 1.0}))).await;
    assert_eq!((longer["count"].clone(), longer["scene_transitions"][0]["start"].clone(), longer["scene_transitions"][0]["seconds"].clone()), (json!(1), json!(1.75), json!(1.0)));

    // Put on a cut by the scene before it: ending at the cut at 2 s, a scene number is read as the scene's.
    let moved = s
        .ok(Method::POST, "/projects/Demo/transitions/set", Some(json!({"id": "t1", "after_scene": 1, "align": "end", "transition": "Swirl", "seconds": 0.5})))
        .await;
    let block = &moved["scene_transitions"][0];
    assert_eq!((moved["count"].clone(), block["transition"].clone(), block["start"].clone()), (json!(1), json!("Swirl"), json!(1.5)));

    // Clearing a cut removes the blocks that cover it.
    let cleared = s.ok(Method::POST, "/projects/Demo/transitions/set", Some(json!({"after_scene": "a", "transition": "none"}))).await;
    assert_eq!(cleared["count"], json!(0));
    let centered = s.ok(Method::POST, "/projects/Demo/transitions/set", Some(json!({"after_scene": 1, "transition": "Fade", "seconds": 2.0}))).await;
    assert_eq!((centered["scene_transitions"][0]["start"].clone(), centered["scene_transitions"][0]["seconds"].clone()), (json!(1.0), json!(2.0)), "centered on the cut");
    let removed = s.ok(Method::POST, "/projects/Demo/transitions/set", Some(json!({"id": "t1", "transition": null}))).await;
    assert_eq!(removed["count"], json!(0));
    let long = s.ok(Method::POST, "/projects/Demo/transitions/set", Some(json!({"start": 0.0, "transition": "Fade", "seconds": 99}))).await;
    assert_eq!(long["scene_transitions"][0]["seconds"], json!(10.0), "the length is clamped");
    s.ok(Method::POST, "/projects/Demo/transitions/set", Some(json!({"id": "t1", "transition": "none"}))).await;

    for (body, why) in [
        (json!({"start": 1.0, "transition": "NoSuchThing"}), "an unknown transition"),
        (json!({"after_scene": 9, "transition": "Swirl"}), "an unknown scene"),
        (json!({"start": 1.0, "transition": "Swirl", "params": {"nope": 1}}), "an unknown setting"),
        (json!({"transition": "Swirl"}), "no place"),
        (json!({"start": -1.0, "transition": "Swirl"}), "a place before the timeline"),
        (json!({"transition": "none"}), "a clear that names nothing"),
        (json!({"id": "t9", "transition": "none"}), "a clear of an id that is not there"),
    ] {
        let (code, reply) = s.call(Method::POST, "/projects/Demo/transitions/set", Some(body)).await;
        assert_eq!(code, 400, "{why}: {reply}");
    }

    // The whole list at once: both forms, kept in the order of the timeline.
    let put = s
        .ok(Method::PUT, "/projects/Demo/transitions", Some(json!({"scene_transitions": [{"start": 3.0, "transition": "Cube", "seconds": 0.4}, {"after_scene": 1, "transition": "Fade"}]})))
        .await;
    let list = put["scene_transitions"].as_array().unwrap();
    assert_eq!(list.iter().map(|t| t["transition"].clone()).collect::<Vec<_>>(), vec![json!("Fade"), json!("Cube")]);
    assert_eq!(list.iter().map(|t| t["id"].clone()).collect::<Vec<_>>(), vec![json!("t2"), json!("t1")]);
    let (code, _) = s.call(Method::PUT, "/projects/Demo/transitions", Some(json!({"scene_transitions": "no"}))).await;
    assert_eq!(code, 400);
}

#[tokio::test]
async fn a_project_saved_with_the_earlier_per_cut_entries_reads_as_blocks() {
    let s = start().await;
    let mut session = s.session();
    session["scene_transitions"] = json!([{"after_scene": "a", "transition": "WipeLeft", "seconds": 0.5, "align": "end", "params": {}}]);
    std::fs::write(s.project.join("vrgdg_builder_session.json"), serde_json::to_string_pretty(&session).unwrap()).unwrap();
    let data = s.ok(Method::GET, "/projects/Demo/transitions", None).await;
    let block = &data["scene_transitions"][0];
    assert_eq!((block["transition"].clone(), block["start"].clone(), block["seconds"].clone()), (json!("WipeLeft"), json!(1.5), json!(0.5)));
}

#[tokio::test]
async fn a_stitch_draws_the_saved_transitions_and_keeps_the_timeline_length() {
    let s = start().await;
    let plain = s.stitch(json!({"audio": "embedded"})).await;
    assert!(plain.get("transitions").is_none(), "a stitch without transitions says nothing about them");
    assert_eq!(plain["timeline_frame_count"], json!(96));

    s.ok(Method::POST, "/projects/Demo/transitions/set", Some(json!({"after_scene": 1, "transition": "WipeLeft", "seconds": 0.5}))).await;
    let drawn = s.stitch(json!({"audio": "embedded", "output_prefix": "WITH"})).await;
    assert_eq!(drawn["timeline_frame_count"], json!(96), "{drawn}");
    assert_eq!(drawn["transitions"][0]["transition"], "WipeLeft");
    assert_eq!(drawn["transitions"][0]["frames"], json!(12));
    assert_eq!(drawn["transitions"][0]["after_scene_number"], json!(1));
    let video = PathBuf::from(drawn["final_video_path"].as_str().unwrap());
    let duration = drawn["output_duration"].as_f64().unwrap();
    assert!((duration - 4.0).abs() < 0.1, "the video is as long as the timeline: {duration}");
    let (red_before, _) = red_and_blue(&video, s.dir.path(), 1.5);
    let (_, blue_after) = red_and_blue(&video, s.dir.path(), 2.6);
    let (red_mid, blue_mid) = red_and_blue(&video, s.dir.path(), 2.0);
    assert!(red_before > 0.95 && blue_after > 0.95, "outside the window each scene is itself");
    assert!(red_mid > 0.1 && blue_mid > 0.1, "at the cut both scenes show: {red_mid} {blue_mid}");

    // A block inside the first scene is an effect on it: frames 12..24, white in the middle.
    s.ok(Method::POST, "/projects/Demo/transitions/set", Some(json!({"start": 0.5, "transition": "FlashColor", "seconds": 0.5}))).await;
    let flashed = s.stitch(json!({"audio": "embedded", "output_prefix": "FLASH"})).await;
    assert_eq!(flashed["timeline_frame_count"], json!(96));
    assert_eq!(flashed["transitions"].as_array().unwrap().len(), 2);
    assert_eq!(flashed["transitions"][0]["after_scene_number"], json!(null), "the one inside the first scene comes first");
    let video = PathBuf::from(flashed["final_video_path"].as_str().unwrap());
    let (red_early, _) = red_and_blue(&video, s.dir.path(), 0.25);
    let (red_flash, _) = red_and_blue(&video, s.dir.path(), 0.75);
    assert!(red_early > 0.95 && red_flash < 0.5, "the scene is red before the flash and not in the middle of it: {red_early} {red_flash}");

    // Transitions sent with the request replace the saved ones for this stitch.
    let none = s.stitch(json!({"audio": "embedded", "transitions": [], "output_prefix": "NONE"})).await;
    assert!(none.get("transitions").is_none());
    let (code, reply) = s.call(Method::POST, "/projects/Demo/stitch", Some(json!({"audio": "embedded", "transitions": [{"start": 1.0, "transition": "NoSuchThing"}]}))).await;
    assert_eq!(code, 400, "{reply}");
}
