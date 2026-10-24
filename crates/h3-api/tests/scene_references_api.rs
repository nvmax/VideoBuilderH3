//! A scene's MiniMax references through the API: the picker data, choosing keys and handing the scene back to its mappings.

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
    std::fs::create_dir_all(&root).unwrap();
    let cfg = AppConfig { comfy_url: "http://127.0.0.1:9".into(), project_root: root, ..AppConfig::default() };
    let state = h3_api::build_state(cfg, &dir.path().join("jobs.db")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, h3_api::router(state)).await.unwrap() });
    Server { base: format!("http://{addr}/api/v1"), client: Client::builder().timeout(Duration::from_secs(60)).build().unwrap(), dir }
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
}

#[tokio::test]
async fn references_are_chosen_per_scene() {
    let s = start().await;
    s.call(Method::POST, "/projects", Some(json!({"name": "Refs"}))).await;
    let p = "/projects/Refs";
    s.call(Method::POST, &format!("{p}/timeline/bulk"), Some(json!({"text": "4 a\n4 b", "mode": "durations"}))).await;
    s.call(Method::PUT, &format!("{p}/references/subjects/subj1"), Some(json!({"name": "Ava", "description": "a singer"}))).await;

    let (code, got) = s.call(Method::GET, &format!("{p}/scenes/1/minimax-references"), None).await;
    assert_eq!(code, 200, "{got}");
    assert_eq!(got["data"]["pipeline"], "standard");
    assert_eq!(got["data"]["scene_number"], 1);
    assert_eq!(got["data"]["custom"], false);
    assert!(got["data"]["limits"]["max_images"].as_i64().unwrap() >= 1);

    // Keys the scene cannot use are refused and say what is allowed.
    let (code, bad) = s.call(Method::PUT, &format!("{p}/scenes/1/minimax-references"), Some(json!({"keys": ["location:nope"]}))).await;
    assert_eq!((code, bad["error"]["code"].as_str()), (400, Some("VALIDATION_ERROR")), "{bad}");
    assert!(bad["error"]["message"].as_str().unwrap().contains("Not available for this scene: location:nope"));
    let (code, _) = s.call(Method::PUT, &format!("{p}/scenes/1/minimax-references"), Some(json!({"keys": ["a", "a"]}))).await;
    assert_eq!(code, 400);
    let (code, _) = s.call(Method::PUT, &format!("{p}/scenes/1/minimax-references"), Some(json!({"keys": "x"}))).await;
    assert_eq!(code, 400);
    let (code, _) = s.call(Method::PUT, &format!("{p}/scenes/1/minimax-references"), Some(json!({}))).await;
    assert_eq!(code, 400);
    let (code, _) = s.call(Method::PUT, &format!("{p}/scenes/1/minimax-references"), Some(json!({"keys": [], "automatic": true}))).await;
    assert_eq!(code, 400);

    // An empty chosen list is a valid custom order. Automatic hands the scene back.
    let (code, empty) = s.call(Method::PUT, &format!("{p}/scenes/1/minimax-references"), Some(json!({"keys": []}))).await;
    assert_eq!(code, 200, "{empty}");
    assert_eq!(empty["data"]["custom"], true);
    let (_, again) = s.call(Method::GET, &format!("{p}/scenes/1/minimax-references"), None).await;
    assert_eq!(again["data"]["custom"], true);
    let (code, auto) = s.call(Method::PUT, &format!("{p}/scenes/1/minimax-references"), Some(json!({"automatic": true}))).await;
    assert_eq!(code, 200, "{auto}");
    assert_eq!(auto["data"]["custom"], false);

    let (code, missing) = s.call(Method::GET, &format!("{p}/scenes/99/minimax-references"), None).await;
    assert_eq!((code, missing["error"]["code"].as_str()), (404, Some("SCENE_NOT_FOUND")));
}

#[tokio::test]
async fn assets_are_listed_and_a_project_is_exported_as_a_portable_zip() {
    let s = start().await;
    s.call(Method::POST, "/projects", Some(json!({"name": "Exp"}))).await;
    let p = "/projects/Exp";
    s.call(Method::POST, &format!("{p}/timeline/bulk"), Some(json!({"text": "4 a", "mode": "durations"}))).await;
    let project = s.dir.path().join("projects").join("Exp");
    std::fs::create_dir_all(project.join("images")).unwrap();
    std::fs::write(project.join("images").join("image_0001.png"), b"png").unwrap();
    let (code, assets) = s.call(Method::GET, &format!("{p}/assets"), None).await;
    assert_eq!(code, 200, "{assets}");
    assert_eq!(assets["data"]["count"], 1);
    assert_eq!(assets["data"]["items"][0]["kind"], "image");
    assert_eq!(assets["data"]["items"][0]["id"], "image_0001");

    // A file outside the project and an API key in the session.
    let outside = s.dir.path().join("elsewhere.png");
    std::fs::write(&outside, b"outside").unwrap();
    let session_file = project.join("vrgdg_builder_session.json");
    let mut session: Value = serde_json::from_str(&std::fs::read_to_string(&session_file).unwrap()).unwrap();
    session["segments"][0]["custom_image_path"] = json!(outside.display().to_string());
    session["own_server_api_key"] = json!("sk-secret");
    std::fs::write(&session_file, serde_json::to_string(&session).unwrap()).unwrap();
    let before = std::fs::read_to_string(&session_file).unwrap();

    let (code, exported) = s.call(Method::POST, &format!("{p}/export"), Some(json!({}))).await;
    assert_eq!(code, 200, "{exported}");
    assert_eq!(exported["data"]["filename"], "Exp.vrgdg.zip");
    let mut archive = zip::ZipArchive::new(std::fs::File::open(exported["data"]["zip_path"].as_str().unwrap()).unwrap()).unwrap();
    let names: Vec<String> = (0..archive.len()).map(|i| archive.by_index(i).unwrap().name().to_string()).collect();
    assert!(names.contains(&"vrgdg_project_package.json".to_string()) && names.contains(&"images/image_0001.png".to_string()), "{names:?}");
    let portable: Vec<&String> = names.iter().filter(|n| n.starts_with("portable_assets/")).collect();
    assert_eq!(portable.len(), 1, "{names:?}");
    let mut text = String::new();
    std::io::Read::read_to_string(&mut archive.by_name("vrgdg_builder_session.json").unwrap(), &mut text).unwrap();
    let packed: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(packed["own_server_api_key"], "");
    assert!(packed["segments"][0]["custom_image_path"].as_str().unwrap().replace('\\', "/").contains("/portable_assets/0001_"), "{}", packed["segments"][0]["custom_image_path"]);
    // The session on disk is not touched.
    assert_eq!(std::fs::read_to_string(&session_file).unwrap(), before);
}

#[tokio::test]
async fn bulk_edits_save_once_and_modes_lists_the_h3_engine() {
    let s = start().await;
    let (code, modes) = s.call(Method::GET, "/modes", None).await;
    assert_eq!(code, 200);
    assert_eq!(modes["data"]["enums"]["video_engines"], json!(["minimax_h3"]));
    assert!(modes["data"]["video_engines"]["minimax_h3"]["modes"]["image_to_video"]["settings_keys"].as_array().unwrap().len() > 10);

    s.call(Method::POST, "/projects", Some(json!({"name": "Bulk"}))).await;
    let p = "/projects/Bulk";
    s.call(Method::POST, &format!("{p}/timeline/bulk"), Some(json!({"text": "4 a
4 b", "mode": "durations"}))).await;
    let (_, scenes) = s.call(Method::GET, &format!("{p}/scenes"), None).await;
    let first = scenes["data"][0]["id"].as_str().unwrap().to_string();
    let second = scenes["data"][1]["id"].as_str().unwrap().to_string();
    let ops = json!({"operations": [
        {"op": "resize", "scene_id": first, "duration": 3.0, "ripple": true},
        {"op": "patch", "scene_id": second, "fields": {"label": "Chorus", "lyric_text": "[instrumental]"}},
        {"op": "nothing"},
    ]});
    let (code, done) = s.call(Method::POST, &format!("{p}/scenes/bulk"), Some(ops)).await;
    assert_eq!(code, 200, "{done}");
    assert_eq!(done["data"]["applied_operations"], 2);
    let (_, after) = s.call(Method::GET, &format!("{p}/scenes"), None).await;
    assert_eq!((after["data"][0]["end"].as_f64(), after["data"][1]["start"].as_f64()), (Some(3.0), Some(3.0)));
    assert_eq!(after["data"][1]["label"], "Chorus");

    // One bad operation saves nothing.
    let bad = json!({"operations": [{"op": "patch", "scene_id": first, "fields": {"label": "X"}}, {"op": "move", "scene_id": "seg_missing", "start": 1.0}]});
    let (code, err) = s.call(Method::POST, &format!("{p}/scenes/bulk"), Some(bad)).await;
    assert_eq!((code, err["error"]["code"].as_str()), (404, Some("SCENE_NOT_FOUND")));
    let (_, unchanged) = s.call(Method::GET, &format!("{p}/scenes"), None).await;
    assert_ne!(unchanged["data"][0]["label"], "X");
}

#[tokio::test]
async fn agents_get_a_brief_assemble_a_prompt_and_validate_it() {
    let s = start().await;
    s.call(Method::POST, "/projects", Some(json!({"name": "Agent"}))).await;
    let p = "/projects/Agent";
    s.call(Method::POST, &format!("{p}/timeline/bulk"), Some(json!({"text": "4 a", "mode": "durations"}))).await;
    let (code, brief) = s.call(Method::GET, &format!("{p}/scenes/1/prompts/context"), None).await;
    assert_eq!(code, 200, "{brief}");
    assert_eq!(brief["data"]["shot_plan"]["shot_count"], 1);
    assert!(brief["data"]["instruction_text"].as_str().unwrap().starts_with("Generate 1 cinematic shot descriptions for a 4.0s "));

    let (code, preview) = s.call(Method::POST, &format!("{p}/scenes/1/prompts/minimax/assemble"), Some(json!({"shots": ["She sings on a roof"], "mode": "text_to_video"}))).await;
    assert_eq!(code, 200, "{preview}");
    assert_eq!((preview["data"]["saved"].clone(), preview["data"]["shots_used"].clone()), (json!(false), json!(1)));
    let prompt = preview["data"]["prompt"].as_str().unwrap().to_string();
    assert!(prompt.contains("[Shot 1] She sings on a roof."));

    let (_, saved) = s.call(Method::POST, &format!("{p}/scenes/1/prompts/minimax/assemble"), Some(json!({"shots": ["She sings on a roof"], "mode": "text_to_video", "save": true}))).await;
    assert_eq!(saved["data"]["saved"], true);
    let (_, scene) = s.call(Method::GET, &format!("{p}/scenes/1"), None).await;
    assert_eq!(scene["data"]["minimax_h3_prompt"], prompt);

    let (_, ok) = s.call(Method::POST, &format!("{p}/scenes/1/prompts/minimax/validate"), Some(json!({"prompt": prompt, "mode": "text_to_video"}))).await;
    assert_eq!(ok["data"]["valid"], true, "{ok}");
    let (_, bad) = s.call(Method::POST, &format!("{p}/scenes/1/prompts/minimax/validate"), Some(json!({"prompt": "no structure", "mode": "text_to_video"}))).await;
    assert_eq!(bad["data"]["errors"][0]["code"], "MISSING_HEADER");
    let (code, _) = s.call(Method::GET, &format!("{p}/scenes/99/prompts/context"), None).await;
    assert_eq!(code, 404);
}

#[tokio::test]
async fn scan_recovers_clips_from_old_scratch_folders_and_index_is_written() {
    let s = start().await;
    s.call(Method::POST, "/projects", Some(json!({"name": "Scan"}))).await;
    let p = "/projects/Scan";
    let project = s.dir.path().join("projects").join("Scan");
    let old = project.join("image_to_video_clips").join("scene_0002");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join("video_0001-audio.mp4"), b"not really a video").unwrap();
    let backups = project.join("rendered_scene_videos_backup").join("scene_0003");
    std::fs::create_dir_all(&backups).unwrap();
    std::fs::write(backups.join("video_0003-audio_20250101_000000.mp4"), b"x").unwrap();

    let (code, scan) = s.call(Method::POST, &format!("{p}/video/scan"), None).await;
    assert_eq!(code, 200, "{scan}");
    assert!(scan["data"]["videos"]["2"].as_str().unwrap().ends_with("video_0002-audio.mp4"), "{scan}");
    assert!(project.join("rendered_scene_videos").join("video_0002-audio.mp4").is_file());
    assert!(scan["data"]["recovered_from_scratch"]["2"].as_str().unwrap().contains("scene_0002"));
    assert_eq!(scan["data"]["video_backups"]["3"].as_array().unwrap().len(), 1);

    let (code, index) = s.call(Method::GET, &format!("{p}/minimax/index"), None).await;
    assert_eq!(code, 200);
    assert!(index["data"]["content"].as_str().unwrap().starts_with("# MiniMax H3 project files"));
    assert!(project.join("MINIMAX_PROJECT_FILES.md").is_file());
}

#[tokio::test]
async fn a_scenes_start_image_can_come_from_the_previous_clips_last_frame() {
    let s = start().await;
    s.call(Method::POST, "/projects", Some(json!({"name": "Frame"}))).await;
    let p = "/projects/Frame";
    s.call(Method::POST, &format!("{p}/timeline/bulk"), Some(json!({"text": "4 a
4 b", "mode": "durations"}))).await;
    let (code, none) = s.call(Method::POST, &format!("{p}/scenes/2/image/from-video-frame"), Some(json!({}))).await;
    assert_eq!((code, none["error"]["code"].as_str()), (400, Some("VALIDATION_ERROR")));

    let project = s.dir.path().join("projects").join("Frame");
    let clip = project.join("rendered_scene_videos").join("video_0001-audio.mp4");
    std::fs::create_dir_all(clip.parent().unwrap()).unwrap();
    let made = std::process::Command::new("ffmpeg").args(["-y", "-f", "lavfi", "-i", "color=c=blue:s=64x64:d=1:r=24", "-pix_fmt", "yuv420p"]).arg(&clip).output().expect("ffmpeg is on the PATH");
    assert!(made.status.success());
    let (_, scenes) = s.call(Method::GET, &format!("{p}/scenes"), None).await;
    let first = scenes["data"][0]["id"].as_str().unwrap().to_string();
    let session_file = project.join("vrgdg_builder_session.json");
    let mut session: Value = serde_json::from_str(&std::fs::read_to_string(&session_file).unwrap()).unwrap();
    session["segments"][0]["video_path"] = json!(clip.display().to_string());
    std::fs::write(&session_file, serde_json::to_string(&session).unwrap()).unwrap();
    let _ = first;

    let (code, done) = s.call(Method::POST, &format!("{p}/scenes/2/image/from-video-frame"), Some(json!({}))).await;
    assert_eq!(code, 200, "{done}");
    let saved = done["data"]["saved_path"].as_str().unwrap();
    assert!(std::path::Path::new(saved).is_file() && saved.contains("scene_image_previews"), "{saved}");
    let (_, scene) = s.call(Method::GET, &format!("{p}/scenes/2"), None).await;
    assert_eq!(scene["data"]["approved_image"]["path"].as_str().map(|p| p.replace('\\', "/")), Some(saved.replace('\\', "/")), "{scene}");
}

#[tokio::test]
async fn a_slideshow_preview_is_rendered_from_stills_and_the_song() {
    let s = start().await;
    s.call(Method::POST, "/projects", Some(json!({"name": "Slides"}))).await;
    let p = "/projects/Slides";
    let dir = s.dir.path().to_path_buf();
    let image = |name: &str, color: &str, size: &str| {
        let path = dir.join(name);
        let source = format!("color=c={color}:s={size}:d=1");
        let made = std::process::Command::new("ffmpeg").args(["-y", "-f", "lavfi", "-i", &source, "-frames:v", "1"]).arg(&path).output().expect("ffmpeg is on the PATH");
        assert!(made.status.success());
        path.display().to_string()
    };
    let (red, green) = (image("red.png", "red", "64x48"), image("green.png", "green", "32x32"));
    let song = dir.join("song.wav");
    let made = std::process::Command::new("ffmpeg").args(["-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=6"]).arg(&song).output().expect("ffmpeg is on the PATH");
    assert!(made.status.success());

    let (code, bad) = s.call(Method::POST, &format!("{p}/slideshow"), Some(json!({"image_items": []}))).await;
    assert_eq!((code, bad["error"]["code"].as_str()), (400, Some("VALIDATION_ERROR")));
    let (code, bad) = s.call(Method::POST, &format!("{p}/slideshow"), Some(json!({"image_items": [{"path": red, "duration": 1}], "audio_path": dir.join("none.wav").display().to_string()}))).await;
    assert_eq!(code, 400, "{bad}");

    let body = json!({"image_items": [{"path": red, "duration": 1.5}, {"path": green, "duration": 2.0}], "audio_path": song.display().to_string(), "width": 128, "height": 72, "fps": 12});
    let (code, queued) = s.call(Method::POST, &format!("{p}/slideshow"), Some(body)).await;
    assert_eq!(code, 202, "{queued}");
    let id = queued["data"]["job_id"].as_str().unwrap().to_string();
    let mut done = Value::Null;
    for _ in 0..300 {
        let (_, job) = s.call(Method::GET, &format!("/jobs/{id}"), None).await;
        if ["succeeded", "failed", "cancelled"].contains(&job["data"]["status"].as_str().unwrap_or("")) {
            done = job["data"].clone();
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(done["status"], "succeeded", "{done}");
    let result = &done["result"];
    assert_eq!((result["scene_count"].as_i64(), result["output_width"].as_i64(), result["fps"].as_i64()), (Some(2), Some(128), Some(12)));
    let video = result["final_video_path"].as_str().unwrap();
    assert!(std::path::Path::new(video).is_file() && video.contains("IMAGE_SLIDESHOW_PREVIEW"), "{video}");
    let probe = std::process::Command::new("ffprobe").args(["-v", "error", "-show_entries", "format=duration", "-of", "default=noprint_wrappers=1:nokey=1", video]).output().unwrap();
    let seconds: f64 = String::from_utf8_lossy(&probe.stdout).trim().parse().unwrap();
    assert!((seconds - 3.5).abs() < 0.2, "{seconds}");
}

#[tokio::test]
async fn song_to_video_prepares_the_project_before_it_needs_comfyui() {
    let s = start().await;
    let (code, bad) = s.call(Method::POST, "/pipelines/from-song", Some(json!({}))).await;
    assert_eq!((code, bad["error"]["code"].as_str()), (400, Some("VALIDATION_ERROR")));

    let song = s.dir.path().join("song.wav");
    let made = std::process::Command::new("ffmpeg").args(["-y", "-f", "lavfi", "-i", "sine=frequency=330:duration=8.5"]).arg(&song).output().expect("ffmpeg is on the PATH");
    assert!(made.status.success());
    let (code, queued) = s.call(Method::POST, "/pipelines/from-song", Some(json!({"project_name": "Song", "audio_path": song.display().to_string(), "lyrics_text": "one
two"}))).await;
    assert_eq!(code, 202, "{queued}");
    assert_eq!(queued["data"]["project_id"], "Song");
    let id = queued["data"]["job_id"].as_str().unwrap().to_string();
    let mut done = Value::Null;
    for _ in 0..300 {
        let (_, job) = s.call(Method::GET, &format!("/jobs/{id}"), None).await;
        if ["succeeded", "failed", "cancelled"].contains(&job["data"]["status"].as_str().unwrap_or("")) {
            done = job["data"].clone();
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // ComfyUI is not running, so the build stops at its first picture, after the scenes were made.
    assert_eq!(done["status"], "failed", "{done}");
    assert_eq!(done["error"]["code"], "COMFY_UNAVAILABLE", "{done}");
    let (_, scenes) = s.call(Method::GET, "/projects/Song/scenes", None).await;
    assert_eq!(scenes["data"].as_array().unwrap().len(), 2);
    assert_eq!((scenes["data"][0]["start"].as_f64(), scenes["data"][1]["end"].as_f64()), (Some(0.0), Some(8.5)));

    let (code, plan) = s.call(Method::GET, "/projects/Song/pipelines/plan?scope=all", None).await;
    assert_eq!(code, 200, "{plan}");
    assert_eq!(plan["data"]["summary"]["total_target_scenes"], 2);
    assert_eq!(plan["data"]["can_run"], true);
    assert_eq!(plan["data"]["summary"]["videos_to_render"], 2);
}

