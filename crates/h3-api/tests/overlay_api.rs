//! The overlay routes of the agent API with real clips: read, replace and add clips, lyric captions, a library clip, and the overlay pass that follows a stitch
//! (a vignette must darken the corner of the stitched video, and `overlay_layers: false` must not).

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

/// The red value of the pixel at (`fx`, `fy`) as a share of the frame, in the frame at `at` seconds.
fn red_at(video: &Path, scratch: &Path, at: f64, fx: f64, fy: f64) -> u8 {
    let frame = scratch.join(format!("frame_{at}.png"));
    let status = Command::new("ffmpeg").args(["-y", "-ss", &at.to_string(), "-i"]).arg(video).args(["-frames:v", "1"]).arg(&frame).output().unwrap().status;
    assert!(status.success());
    let image = image::open(&frame).unwrap().to_rgb8();
    let (w, h) = image.dimensions();
    image.get_pixel(((w - 1) as f64 * fx) as u32, ((h - 1) as f64 * fy) as u32).0[0]
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
            {"id": "a", "start": 0.0, "end": 2.0, "label": "Scene 1", "lyric_text": "first line", "video_path": clip1.display().to_string()},
            {"id": "b", "start": 2.0, "end": 4.0, "label": "Scene 2", "lyric_text": "second line", "video_path": clip2.display().to_string()}
        ]
    });
    std::fs::write(project.join("vrgdg_builder_session.json"), serde_json::to_string_pretty(&session).unwrap()).unwrap();
    // The library is the `overlay` folder of the test directory, set in the config. ComfyUI is a stand-in: a stitch job connects to it first, and the ComfyUI on this computer must not be used.
    let comfy = comfy_stand_in(dir.path()).await;
    let cfg = AppConfig { comfy_url: comfy, project_root: root, overlay_library_dir: dir.path().join("overlay").display().to_string(), ..AppConfig::default() };
    let state = h3_api::build_state(cfg, &dir.path().join("jobs.db")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, h3_api::router(state)).await.unwrap() });
    Server { base: format!("http://{addr}/api/v1"), client: Client::builder().timeout(Duration::from_secs(120)).build().unwrap(), project, dir }
}

impl Server {
    async fn call(&self, method: Method, path: &str, body: Option<Value>, if_match: Option<i64>) -> (u16, Value) {
        let mut request = self.client.request(method, format!("{}{}", self.base, path));
        if let Some(body) = body {
            request = request.json(&body);
        }
        if let Some(revision) = if_match {
            request = request.header("If-Match", revision.to_string());
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Value::Null))
    }

    async fn ok(&self, method: Method, path: &str, body: Option<Value>) -> Value {
        let (code, response) = self.call(method, path, body, None).await;
        assert_eq!(code, 200, "{path}: {response}");
        response["data"].clone()
    }

    async fn stitch(&self, body: Value) -> Value {
        let (code, response) = self.call(Method::POST, "/projects/Demo/stitch", Some(body), None).await;
        assert_eq!(code, 202, "{response}");
        let id = response["data"]["job_id"].as_str().unwrap().to_string();
        for _ in 0..600 {
            let (_, body) = self.call(Method::GET, &format!("/jobs/{id}"), None, None).await;
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

#[tokio::test]
async fn an_agent_reads_replaces_and_adds_clips_and_sees_what_was_saved() {
    let s = start().await;
    let view = s.ok(Method::GET, "/projects/Demo/overlay-layers", None).await;
    assert_eq!((view["overlay_layer_track"]["enabled"].clone(), view["count"].clone(), view["warnings"].clone()), (json!(true), json!(0), json!([])));

    let (code, reply) = s
        .call(
            Method::PUT,
            "/projects/Demo/overlay-layers",
            Some(json!({"overlay_layers": [
                {"kind": "grain", "id": "g", "start": 0, "end": 4, "strength": 99},
                {"kind": "media", "id": "m", "start": 0, "end": 2, "path": "overlay/not_there.mov"},
                {"kind": "text", "id": "t", "start": 1, "end": 3, "text": "Hello", "font": "Papyrus"},
                {"kind": "sparkle", "id": "x", "start": 0, "end": 1}
            ]})),
            None,
        )
        .await;
    assert_eq!(code, 200, "{reply}");
    let data = &reply["data"];
    assert_eq!(data["overlay_layers"][0]["strength"], json!(60.0), "99 is clamped and the reply shows the saved value");
    let reasons: Vec<(String, String)> = data["warnings"].as_array().unwrap().iter().map(|w| (w["id"].as_str().unwrap().to_string(), w["reason"].as_str().unwrap().to_string())).collect();
    assert_eq!(reasons.len(), 3, "{reasons:?}");
    assert!(reasons[0].1.contains("media file was not found") && reasons[0].0 == "m");
    assert!(reasons[1].1.contains("'Papyrus' is not installed") && reasons[1].0 == "t");
    assert!(reasons[2].1.contains("'sparkle' is not a clip kind") && reasons[2].0 == "x");
    assert_eq!(s.session()["overlay_layers"][0]["strength"], json!(60.0), "what the reply showed is what is on disk");

    // The revision check, the field check and the shape check.
    let revision = reply["revision"].as_i64().unwrap();
    let (code, body) = s.call(Method::PUT, "/projects/Demo/overlay-layers", Some(json!({"overlay_layer_track": {"enabled": false}})), Some(revision - 1)).await;
    assert_eq!((code, body["error"]["code"].as_str()), (409, Some("REVISION_CONFLICT")), "{body}");
    let (code, _) = s.call(Method::PUT, "/projects/Demo/overlay-layers", Some(json!({"clips": []})), None).await;
    assert_eq!(code, 400);
    let (code, _) = s.call(Method::PUT, "/projects/Demo/overlay-layers", Some(json!({"overlay_layers": "grain"})), None).await;
    assert_eq!(code, 400);
    let (code, _) = s.call(Method::PUT, "/projects/Nope/overlay-layers", Some(json!({"overlay_layers": []})), None).await;
    assert_eq!(code, 404);

    // Adding keeps the others, a switch alone keeps the clips.
    let added = s.ok(Method::POST, "/projects/Demo/overlay-layers/add", Some(json!({"layers": [{"kind": "vignette", "start": 0, "end": 4}]}))).await;
    assert_eq!((added["count"].clone(), added["added"].clone()), (json!(5), json!(["overlay_5"])));
    let off = s.ok(Method::PUT, "/projects/Demo/overlay-layers", Some(json!({"overlay_layer_track": {"enabled": false}}))).await;
    assert_eq!((off["overlay_layer_track"]["enabled"].clone(), off["count"].clone()), (json!(false), json!(5)));
    let (code, _) = s.call(Method::POST, "/projects/Demo/overlay-layers/add", Some(json!({"layers": []})), None).await;
    assert_eq!(code, 400);
    let (code, _) = s.call(Method::POST, "/projects/Demo/overlay-layers/add", Some(json!({"layer": {}})), None).await;
    assert_eq!(code, 400);
}

#[tokio::test]
async fn lyric_captions_take_any_font_of_the_fonts_list_and_refuse_one_that_is_not_there() {
    let s = start().await;
    let chosen = s.ok(Method::POST, "/projects/Demo/overlay-layers/lyric_captions", Some(json!({"style": "bold", "font": " Caveat "}))).await;
    let fonts: Vec<&str> = chosen["overlay_layers"].as_array().unwrap().iter().map(|c| c["font"].as_str().unwrap()).collect();
    assert_eq!(fonts, ["Caveat", "Caveat"], "the font replaces the font of the style");
    // A font installed on this computer works the same way (the first one, when the computer has any).
    if let Some(installed) = h3_media::fonts::system_fonts(false).first() {
        let reply = s.ok(Method::POST, "/projects/Demo/overlay-layers/lyric_captions", Some(json!({"font": installed.name}))).await;
        assert_eq!(reply["overlay_layers"][0]["font"], json!(installed.name));
    }
    let (code, body) = s.call(Method::POST, "/projects/Demo/overlay-layers/lyric_captions", Some(json!({"font": "No Such Font"})), None).await;
    assert_eq!(code, 400, "{body}");
    assert!(body.to_string().contains("There is no font called No Such Font."), "{body}");
}

#[tokio::test]
async fn lyric_captions_come_from_the_scenes_replace_each_other_and_take_a_style() {
    let s = start().await;
    let first = s.ok(Method::POST, "/projects/Demo/overlay-layers/lyric_captions", Some(json!({"style": "bold"}))).await;
    assert_eq!((first["captions"].clone(), first["source"].clone(), first["style"].clone()), (json!(2), json!("scenes"), json!("bold")));
    let texts: Vec<(String, f64, f64)> = first["overlay_layers"].as_array().unwrap().iter().map(|c| (c["text"].as_str().unwrap().to_string(), c["start"].as_f64().unwrap(), c["end"].as_f64().unwrap())).collect();
    assert_eq!(texts, [("first line".to_string(), 0.0, 2.0), ("second line".to_string(), 2.0, 4.0)]);
    assert_eq!(first["overlay_layers"][0]["font"], json!("Anton"), "bold is the Anton style");

    // A second run replaces them, and another style changes the look, and an unknown style is refused.
    let second = s.ok(Method::POST, "/projects/Demo/overlay-layers/lyric_captions", Some(json!({"style": "lower"}))).await;
    assert_eq!((second["count"].clone(), second["overlay_layers"][0]["font"].clone()), (json!(2), json!("Oswald")));
    let (code, _) = s.call(Method::POST, "/projects/Demo/overlay-layers/lyric_captions", Some(json!({"style": "wild"})), None).await;
    assert_eq!(code, 400);
    let (code, _) = s.call(Method::POST, "/projects/Demo/overlay-layers/lyric_captions", Some(json!({"colour": "red"})), None).await;
    assert_eq!(code, 400);

    // A range replaces only the captions inside it. `replace: false` keeps them all.
    let ranged = s.ok(Method::POST, "/projects/Demo/overlay-layers/lyric_captions", Some(json!({"style": "clean", "from": 2.0, "to": 4.0}))).await;
    let fonts: Vec<&str> = ranged["overlay_layers"].as_array().unwrap().iter().map(|c| c["font"].as_str().unwrap()).collect();
    assert_eq!(fonts, ["Oswald", "Montserrat"]);
    let kept = s.ok(Method::POST, "/projects/Demo/overlay-layers/lyric_captions", Some(json!({"replace": false}))).await;
    assert_eq!(kept["count"], json!(4));
    let (code, body) = s.call(Method::POST, "/projects/Demo/overlay-layers/lyric_captions", Some(json!({"from": 50.0, "to": 60.0})), None).await;
    assert_eq!(code, 400, "{body}");

    // Word timing is used when the project has it.
    let context = s.project.join("project_context");
    std::fs::create_dir_all(&context).unwrap();
    std::fs::write(context.join("timestamped_lyrics.json"), json!({"segments": [{"type": "vocal", "start": 0.5, "end": 1.5, "text": "timed  words"}]}).to_string()).unwrap();
    let timed = s.ok(Method::POST, "/projects/Demo/overlay-layers/lyric_captions", Some(json!({}))).await;
    assert_eq!((timed["source"].clone(), timed["captions"].clone()), (json!("timestamped_lyrics"), json!(1)));
    assert_eq!(timed["overlay_layers"].as_array().unwrap().last().unwrap()["text"], json!("timed words"), "extra spaces are collapsed");
}

#[tokio::test]
async fn a_library_clip_is_copied_into_the_project_and_added_over_the_whole_timeline() {
    let s = start().await;
    let library = s.dir.path().join("overlay").join("dust");
    make_video(&library.join("sparks.mp4"), "black");

    let listed = s.ok(Method::GET, "/overlay-library", None).await;
    let ids: Vec<&str> = listed["items"].as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["dust/sparks.mp4"]);
    assert_eq!(listed["kinds"], json!(["media", "grain", "vignette", "text", "lut"]));
    assert_eq!(listed["caption_styles"], json!(["clean", "bold", "lower"]));
    assert!(listed["fonts"].as_array().unwrap().contains(&json!("Bebas Neue")));

    let added = s.ok(Method::POST, "/projects/Demo/overlay-layers/media", Some(json!({"library_id": "dust/sparks.mp4", "opacity": 0.5}))).await;
    assert_eq!(added["path"], json!("overlay/sparks.mp4"));
    assert!(s.project.join("overlay").join("sparks.mp4").is_file(), "the clip is copied into the project");
    let clip = &added["overlay_layers"][0];
    assert_eq!((clip["kind"].clone(), clip["start"].clone(), clip["end"].clone(), clip["loop"].clone(), clip["opacity"].clone()), (json!("media"), json!(0.0), json!(4.0), json!(true), json!(0.5)));
    assert_eq!(added["warnings"], json!([]), "the copied file is found");

    let (code, body) = s.call(Method::POST, "/projects/Demo/overlay-layers/media", Some(json!({"library_id": "dust/nothing.mp4"})), None).await;
    assert_eq!(code, 400, "{body}");
    let (code, _) = s.call(Method::POST, "/projects/Demo/overlay-layers/media", Some(json!({})), None).await;
    assert_eq!(code, 400);
}

#[tokio::test]
async fn a_stitch_draws_the_overlay_and_overlay_false_leaves_it_out() {
    let s = start().await;
    let scratch = s.dir.path().join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();
    // No overlays in the project: nothing is reported.
    let plain = s.stitch(json!({"audio": "embedded"})).await;
    assert_eq!(plain["overlay_layers"], Value::Null);
    let plain_video = PathBuf::from(plain["final_video_path"].as_str().unwrap());
    assert!(red_at(&plain_video, &scratch, 0.5, 0.0, 0.0) > 200, "the plain video is red in the corner");

    s.ok(Method::PUT, "/projects/Demo/overlay-layers", Some(json!({"overlay_layers": [{"kind": "vignette", "start": 0, "end": 4, "strength": 1, "fade_in": 0, "fade_out": 0}]}))).await;
    let skipped = s.stitch(json!({"audio": "embedded", "overlay_layers": false})).await;
    assert_eq!(skipped["overlay_layers"], Value::Null, "overlay_layers: false runs no overlay pass");
    assert!(red_at(Path::new(skipped["final_video_path"].as_str().unwrap()), &scratch, 0.5, 0.0, 0.0) > 200);

    let drawn = s.stitch(json!({"audio": "embedded"})).await;
    assert_eq!((drawn["overlay_layers"]["applied"].clone(), drawn["overlay_layers"]["clips"].as_array().map(Vec::len)), (json!(true), Some(1)), "{}", drawn["overlay_layers"]);
    let video = PathBuf::from(drawn["final_video_path"].as_str().unwrap());
    let (corner, middle) = (red_at(&video, &scratch, 0.5, 0.0, 0.0), red_at(&video, &scratch, 0.5, 0.5, 0.5));
    assert!(corner < 90 && middle > 180, "the vignette darkens the corner (red {corner}) and not the middle (red {middle})");
    let dir_files: Vec<String> = std::fs::read_dir(video.parent().unwrap()).unwrap().filter_map(Result::ok).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.contains("vrgdg_overlay")).collect();
    assert!(dir_files.is_empty(), "no temporary overlay file is left: {dir_files:?}");

    // A range stitch starts part way into the timeline: a clip on the second scene must show from the first frame of that video.
    s.ok(Method::PUT, "/projects/Demo/overlay-layers", Some(json!({"overlay_layers": [{"kind": "vignette", "start": 2, "end": 4, "strength": 1, "fade_in": 0, "fade_out": 0}]}))).await;
    let range = s.stitch(json!({"audio": "embedded", "scene_ids": ["b"], "output_prefix": "RANGE"})).await;
    assert_eq!(range["overlay_layers"]["applied"], json!(true), "{}", range["overlay_layers"]);
    let ranged = PathBuf::from(range["final_video_path"].as_str().unwrap());
    assert!(red_at(&ranged, &scratch, 0.2, 0.0, 0.0) < 90, "the clip shows at the start of the second scene's video");

    // The track switch turns the pass off and says so.
    s.ok(Method::PUT, "/projects/Demo/overlay-layers", Some(json!({"overlay_layer_track": {"enabled": false}}))).await;
    let off = s.stitch(json!({"audio": "embedded"})).await;
    assert_eq!((off["overlay_layers"]["applied"].clone(), off["overlay_layers"]["reason"].clone()), (json!(false), json!("The overlays are off.")));
    assert!(red_at(Path::new(off["final_video_path"].as_str().unwrap()), &scratch, 0.5, 0.0, 0.0) > 200);
}

impl Server {
    /// A tool call through the app's MCP endpoint, as an agent makes it: whether it failed and what it answered.
    async fn mcp(&self, name: &str, arguments: Value) -> (bool, Value) {
        let root = self.base.trim_end_matches("/api/v1");
        let response = self.client.post(format!("{root}/mcp")).json(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": arguments}})).send().await.unwrap();
        assert_eq!(response.status(), 200, "{name}");
        let answer: Value = response.json().await.unwrap();
        let result = &answer["result"];
        let text = result["content"].as_array().and_then(|c| c.iter().find(|i| i["type"] == "text")).and_then(|i| i["text"].as_str()).unwrap_or("").to_string();
        (result["isError"].as_bool().unwrap_or(true), serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }
}

#[tokio::test]
async fn an_agent_lists_adds_changes_and_renders_overlay_through_the_mcp_tools() {
    let s = start().await;
    let (failed, library) = s.mcp("api_get_overlay_library", json!({})).await;
    assert!(!failed, "{library}");
    assert_eq!(library["caption_styles"], json!(["clean", "bold", "lower"]));

    let (failed, added) = s.mcp("api_post_overlay_layers_add", json!({"project_id": "Demo", "body": {"layers": [{"kind": "vignette", "start": 0, "end": 4, "strength": 1, "fade_in": 0, "fade_out": 0}]}})).await;
    assert!(!failed, "{added}");
    let (failed, captions) = s.mcp("api_post_overlay_layers_lyric_captions", json!({"project_id": "Demo", "body": {"style": "clean"}})).await;
    assert!(!failed, "{captions}");
    assert_eq!(captions["captions"], json!(2));

    let (failed, read) = s.mcp("api_get_overlay_layers", json!({"project_id": "Demo"})).await;
    assert!(!failed, "{read}");
    assert_eq!(read["count"], json!(3));
    let revision = read["revision"].as_i64().unwrap();

    // A change made from a stale read is refused with a message the agent can act on, then made from a fresh one.
    let keep: Vec<Value> = read["overlay_layers"].as_array().unwrap().iter().filter(|c| c["kind"] == "vignette").cloned().collect();
    let (failed, refused) = s.mcp("api_put_overlay_layers", json!({"project_id": "Demo", "body": {"overlay_layers": keep}, "if_match_revision": revision - 1})).await;
    assert!(failed && refused.to_string().contains("REVISION_CONFLICT"), "{refused}");
    let (failed, changed) = s.mcp("api_put_overlay_layers", json!({"project_id": "Demo", "body": {"overlay_layers": keep}, "if_match_revision": revision})).await;
    assert!(!failed, "{changed}");
    assert_eq!(changed["count"], json!(1));

    // The stitch tool draws what is on the track and says so.
    let (failed, started) = s.mcp("stitch_final", json!({"project_id": "Demo", "params": {"audio": "embedded"}})).await;
    assert!(!failed, "{started}");
    let job_id = started["job_id"].as_str().unwrap_or_else(|| panic!("no job id in {started}")).to_string();
    let (failed, done) = s.mcp("job_wait", json!({"job_id": job_id, "timeout_seconds": 120})).await;
    assert!(!failed, "{done}");
    let result = if done["result"].is_object() { &done["result"] } else { &done["job"]["result"] };
    assert_eq!((result["overlay_layers"]["applied"].clone(), result["overlay_layers"]["clips"].as_array().map(Vec::len)), (json!(true), Some(1)), "{done}");
}

/// A ComfyUI stand-in that answers what a stitch job asks when it connects: its stats, its folders (under `root`) and an empty node list.
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

/// A ComfyUI stand-in that says where its `models` folder is.
async fn comfy_with_models(models: &Path) -> String {
    let reported = json!({"checkpoints": [models.join("checkpoints").display().to_string()]});
    let app = Router::new().route("/internal/folder_paths", get(move || async move { Json(reported) }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

/// The API on a config file in `dir` (`jobs` names the job database, so two copies can run on one folder). Returns the base URL.
async fn serve_in(dir: &Path, jobs: &str, cfg: AppConfig) -> String {
    let state = h3_api::build_state(cfg, &dir.join(jobs)).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, h3_api::router(state)).await.unwrap() });
    format!("http://{addr}/api/v1")
}

async fn library_at(base: &str) -> (Vec<String>, String) {
    let reply: Value = Client::new().get(format!("{base}/overlay-library")).send().await.unwrap().json().await.unwrap();
    let ids = reply["data"]["items"].as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap().to_string()).collect();
    (ids, reply["data"]["folder"].as_str().unwrap().to_string())
}

#[tokio::test]
async fn the_library_is_models_overlay_of_comfyui_and_is_remembered_while_comfyui_is_not_running() {
    let dir = tempfile::tempdir().unwrap();
    let models = dir.path().join("ComfyUI").join("models");
    make_video(&models.join("overlay").join("dust").join("sparks.mp4"), "black");
    let projects = dir.path().join("projects");
    let overlay_folder = models.join("overlay").display().to_string();

    // ComfyUI says where `models` is, so the clip in models/overlay is the library.
    let comfy = comfy_with_models(&models).await;
    let running = serve_in(dir.path(), "jobs.db", AppConfig { comfy_url: comfy, project_root: projects.clone(), ..AppConfig::default() }).await;
    assert_eq!(library_at(&running).await, (vec!["dust/sparks.mp4".to_string()], overlay_folder.clone()));

    // The next start finds ComfyUI stopped and still uses the folder it saw.
    let stopped = serve_in(dir.path(), "jobs_again.db", AppConfig { comfy_url: "http://127.0.0.1:9".into(), project_root: projects.clone(), ..AppConfig::default() }).await;
    assert_eq!(library_at(&stopped).await, (vec!["dust/sparks.mp4".to_string()], overlay_folder));

    // A folder named in the config wins over ComfyUI's.
    let own = dir.path().join("own_overlay");
    let comfy = comfy_with_models(&models).await;
    let named = serve_in(dir.path(), "jobs_named.db", AppConfig { comfy_url: comfy, project_root: projects, overlay_library_dir: own.display().to_string(), ..AppConfig::default() }).await;
    assert_eq!(library_at(&named).await, (Vec::<String>::new(), own.display().to_string()));
}

#[tokio::test]
async fn without_comfyui_and_without_a_setting_the_library_is_beside_the_config_file() {
    let dir = tempfile::tempdir().unwrap();
    let base = serve_in(dir.path(), "jobs.db", AppConfig { comfy_url: "http://127.0.0.1:9".into(), project_root: dir.path().join("projects"), ..AppConfig::default() }).await;
    assert_eq!(library_at(&base).await, (Vec::<String>::new(), dir.path().join("overlay").display().to_string()));
}
