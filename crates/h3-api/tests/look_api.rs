//! The effect and filter routes of the agent API: the catalogs, a project's effects and filters as blocks of the timeline, and a stitch that draws them.
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
async fn the_catalogs_list_effects_and_filters_with_their_settings() {
    let s = start().await;
    let effects = s.ok(Method::GET, "/effects", None).await;
    assert!(effects["count"].as_u64().unwrap() >= 70);
    let glow = effects["effects"].as_array().unwrap().iter().find(|t| t["id"] == "Glow").expect("Glow is listed");
    assert_eq!((glow["kind"].clone(), glow["category"].clone(), glow["origin"].clone()), (json!("effect"), json!("Light"), json!("original")));
    let threshold = glow["params"].as_array().unwrap().iter().find(|p| p["name"] == "threshold").expect("it has a threshold");
    assert_eq!((threshold["type"].clone(), threshold["min"].clone(), threshold["max"].clone()), (json!("float"), json!(0.0), json!(1.0)));

    let filters = s.ok(Method::GET, "/filters", None).await;
    assert!(filters["count"].as_u64().unwrap() >= 120);
    let teal = filters["filters"].as_array().unwrap().iter().find(|t| t["id"] == "TealAndOrange").expect("TealAndOrange is listed");
    assert_eq!((teal["kind"].clone(), teal["category"].clone()), (json!("filter"), json!("Cinematic")));
    assert!(teal["params"].as_array().unwrap().iter().any(|p| p["name"] == "shadowTone" && p["color"] == json!(true)));
}

#[tokio::test]
async fn an_agent_adds_changes_and_removes_effects_and_filters_on_the_timeline() {
    let s = start().await;
    assert_eq!(s.ok(Method::GET, "/projects/Demo/effects", None).await["count"], json!(0));

    let added = s.ok(Method::POST, "/projects/Demo/effects/set", Some(json!({"look": "Glow", "start": 1.0, "seconds": 0.5, "intensity": 0.6}))).await;
    let block = &added["video_effects"][0];
    assert_eq!((block["id"].clone(), block["look"].clone(), block["start"].clone(), block["end"].clone(), block["intensity"].clone()), (json!("e1"), json!("Glow"), json!(1.0), json!(1.5), json!(0.6)));
    assert_eq!(s.session()["video_effects"][0]["look"], "Glow");

    // Only the fields sent change.
    let longer = s.ok(Method::POST, "/projects/Demo/effects/set", Some(json!({"id": "e1", "seconds": 2.0, "params": {"radius": 0.05}}))).await;
    let block = &longer["video_effects"][0];
    assert_eq!((longer["count"].clone(), block["start"].clone(), block["seconds"].clone(), block["intensity"].clone(), block["params"]["radius"].clone()), (json!(1), json!(1.0), json!(2.0), json!(0.6), json!(0.05)));

    // Overlapping blocks are allowed and come back in the order of the timeline.
    s.ok(Method::POST, "/projects/Demo/effects/set", Some(json!({"look": "Flash", "start": 0.25}))).await;
    let both = s.ok(Method::GET, "/projects/Demo/effects", None).await;
    assert_eq!(both["video_effects"].as_array().unwrap().iter().map(|e| e["look"].clone()).collect::<Vec<_>>(), vec![json!("Flash"), json!("Glow")]);

    let removed = s.ok(Method::POST, "/projects/Demo/effects/set", Some(json!({"id": "e1", "look": null}))).await;
    assert_eq!(removed["count"], json!(1));

    // Filters are a list of their own.
    let filter = s.ok(Method::POST, "/projects/Demo/filters/set", Some(json!({"look": "TealAndOrange", "start": 0.0, "seconds": 4.0}))).await;
    assert_eq!(filter["video_filters"][0]["id"], "f1");
    assert_eq!(s.ok(Method::GET, "/projects/Demo/effects", None).await["count"], json!(1), "the effects are untouched");

    for (path, body, why) in [
        ("/projects/Demo/effects/set", json!({"look": "NoSuchThing", "start": 1.0}), "an unknown effect"),
        ("/projects/Demo/effects/set", json!({"look": "Sepia", "start": 1.0}), "a filter is not an effect"),
        ("/projects/Demo/filters/set", json!({"look": "Glow", "start": 1.0}), "an effect is not a filter"),
        ("/projects/Demo/effects/set", json!({"look": "Glow"}), "no place"),
        ("/projects/Demo/effects/set", json!({"look": "Glow", "start": -1.0}), "a place before the timeline"),
        ("/projects/Demo/effects/set", json!({"look": "Glow", "start": 1.0, "params": {"nope": 1}}), "an unknown setting"),
        ("/projects/Demo/effects/set", json!({"look": "none"}), "a removal that names nothing"),
        ("/projects/Demo/effects/set", json!({"id": "e9", "look": null}), "a removal of an id that is not there"),
    ] {
        let (code, reply) = s.call(Method::POST, path, Some(body)).await;
        assert_eq!(code, 400, "{why}: {reply}");
    }

    // The whole list at once, kept in the order of the timeline, with ids made for the entries that have none.
    let put = s
        .ok(Method::PUT, "/projects/Demo/filters", Some(json!({"video_filters": [{"look": "Sepia", "start": 2.0, "seconds": 1.0}, {"look": "NoirClassic", "start": 0.5, "intensity": 0.5}]})))
        .await;
    let list = put["video_filters"].as_array().unwrap();
    assert_eq!(list.iter().map(|t| t["look"].clone()).collect::<Vec<_>>(), vec![json!("NoirClassic"), json!("Sepia")]);
    assert_eq!(list.iter().map(|t| t["id"].clone()).collect::<Vec<_>>(), vec![json!("f2"), json!("f1")]);
    let (code, _) = s.call(Method::PUT, "/projects/Demo/filters", Some(json!({"video_filters": "no"}))).await;
    assert_eq!(code, 400);
    let (code, _) = s.call(Method::PUT, "/projects/Demo/filters", Some(json!({"video_effects": []}))).await;
    assert_eq!(code, 400, "the other list's key");
}

#[tokio::test]
async fn a_stitch_draws_the_saved_filters_and_effects_and_keeps_the_timeline_length() {
    let s = start().await;
    let plain = s.stitch(json!({"audio": "embedded"})).await;
    assert!(plain.get("looks").is_none(), "a stitch without effects or filters says nothing about them");

    // The first scene is red until 0.5 s and again from 1.5 s, and gray in between. The second is blue, and its middle half second is inverted.
    s.ok(Method::POST, "/projects/Demo/filters/set", Some(json!({"look": "NoirClassic", "start": 0.5, "seconds": 1.0}))).await;
    s.ok(Method::POST, "/projects/Demo/effects/set", Some(json!({"look": "Negative", "start": 3.0, "seconds": 0.5}))).await;
    let drawn = s.stitch(json!({"audio": "embedded", "output_prefix": "LOOKS"})).await;
    assert_eq!(drawn["timeline_frame_count"], json!(96), "{drawn}");
    let looks = drawn["looks"].as_array().unwrap();
    assert_eq!(looks.iter().map(|l| (l["kind"].clone(), l["look"].clone(), l["frames"].clone())).collect::<Vec<_>>(), vec![(json!("filter"), json!("NoirClassic"), json!(24)), (json!("effect"), json!("Negative"), json!(12))]);
    let duration = drawn["output_duration"].as_f64().unwrap();
    assert!((duration - 4.0).abs() < 0.1, "the video is as long as the timeline: {duration}");
    let video = PathBuf::from(drawn["final_video_path"].as_str().unwrap());
    let (red_before, _) = red_and_blue(&video, s.dir.path(), 0.25);
    let (red_inside, _) = red_and_blue(&video, s.dir.path(), 1.0);
    let (red_after, _) = red_and_blue(&video, s.dir.path(), 1.75);
    assert!(red_before > 0.95 && red_after > 0.95, "outside the filter the scene is red");
    assert!(red_inside < 0.01, "inside it the scene is gray: {red_inside}");
    let (_, blue_before) = red_and_blue(&video, s.dir.path(), 2.6);
    let (_, blue_inside) = red_and_blue(&video, s.dir.path(), 3.25);
    let (_, blue_after) = red_and_blue(&video, s.dir.path(), 3.75);
    assert!(blue_before > 0.95 && blue_after > 0.95, "outside the effect the scene is blue");
    assert!(blue_inside < 0.01, "inside it the colours are inverted: {blue_inside}");

    // Blocks sent with the request replace the saved ones for this stitch.
    let none = s.stitch(json!({"audio": "embedded", "filters": [], "effects": [], "output_prefix": "NONE"})).await;
    assert!(none.get("looks").is_none());
    let (code, reply) = s.call(Method::POST, "/projects/Demo/stitch", Some(json!({"audio": "embedded", "effects": [{"look": "NoSuchThing", "start": 1.0}]}))).await;
    assert_eq!(code, 400, "{reply}");
    let (code, reply) = s.call(Method::POST, "/projects/Demo/stitch", Some(json!({"audio": "embedded", "effects": [{"look": "Sepia", "start": 1.0}]}))).await;
    assert_eq!(code, 400, "a filter sent as an effect: {reply}");
}
