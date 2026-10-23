//! A continued scene gets its prompt written from the previous scene's real final frame right before it renders.
//! Needs ffmpeg, like the other media tests.

use std::process::Command;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use h3_api::continued_prompt::write_before_render;
use h3_project::AppConfig;
use serde_json::{json, Value};

const SHOTS: &str = "{\"shots\": [{\"description\": \"<Subject 1> (Ava) keeps walking along the roof edge as the camera glides on beside her in one steady move.\"}]}";

async fn llm() -> (String, Arc<Mutex<Vec<Value>>>) {
    let seen: Arc<Mutex<Vec<Value>>> = Arc::default();
    let app = Router::new()
        .route(
            "/v1/chat/completions",
            post(|State(seen): State<Arc<Mutex<Vec<Value>>>>, Json(body): Json<Value>| async move {
                seen.lock().unwrap().push(body);
                Json(json!({"choices": [{"message": {"content": SHOTS}}]}))
            }),
        )
        .with_state(Arc::clone(&seen));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, seen)
}

fn make_video(path: &std::path::Path) {
    let status = Command::new("ffmpeg")
        .args(["-y", "-f", "lavfi", "-i", "color=c=red:s=64x64:d=1:r=24", "-pix_fmt", "yuv420p"])
        .arg(path)
        .output()
        .expect("ffmpeg is on the PATH")
        .status;
    assert!(status.success());
}

fn session(video: &str) -> Value {
    json!({
        "project_name": "Demo", "revision": 1, "video_engine": "minimax_h3",
        "minimax_h3_settings": {"audio_mode": "input_audio", "aspect_ratio": "16:9", "continuity_mode": "latent_continuation_masked", "continuity_prompt_from_last_frame": true},
        "segments": [
            {"id": "a", "start": 0.0, "end": 4.0, "label": "Scene 1", "lyric_text": "hold me close", "video_path": video, "minimax_h3_prompt": "[Shot 1] At 0:00, Ava walks the roof edge."},
            {"id": "b", "start": 4.0, "end": 8.0, "label": "Scene 2", "lyric_text": "never let go"}
        ],
        "flux_reference_builder": {
            "subjects": [{"id": "s", "name": "Ava", "description": "dark hair", "image": {"path": "C:\\refs\\ava.png"}}],
            "locations": [],
            "subject_scene_map": {"a": ["s"], "b": ["s"]},
            "use_subject_reference": true
        }
    })
}

#[tokio::test]
async fn the_prompt_is_written_from_the_previous_final_frame() {
    let (url, seen) = llm().await;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("projects");
    let project = root.join("Demo");
    std::fs::create_dir_all(&project).unwrap();
    let video = project.join("video_0001.mp4");
    make_video(&video);
    std::fs::write(project.join("vrgdg_builder_session.json"), serde_json::to_string_pretty(&session(&video.display().to_string())).unwrap()).unwrap();
    let mut cfg = AppConfig { project_root: root, ..AppConfig::default() };
    cfg.llm.runner = "own_server".into();
    cfg.llm.own_server_url = url;
    cfg.llm.own_server_model = "m".into();
    let state = h3_api::build_state(cfg, &dir.path().join("jobs.db")).unwrap();
    let read = || serde_json::from_str::<Value>(&std::fs::read_to_string(project.join("vrgdg_builder_session.json")).unwrap()).unwrap();

    // The first scene, and a scene given a prompt of its own, are left alone.
    write_before_render(&state, &project, "1", false).await.unwrap();
    write_before_render(&state, &project, "2", true).await.unwrap();
    assert!(seen.lock().unwrap().is_empty());
    assert!(read()["segments"][1].get("minimax_h3_prompt").is_none());

    write_before_render(&state, &project, "b", false).await.unwrap();
    let saved = read();
    let scene = &saved["segments"][1];
    let prompt = scene["minimax_h3_prompt"].as_str().unwrap();
    assert!(prompt.contains("[Shot 1]") && prompt.contains("<Subject 1>"), "{prompt}");
    assert_eq!(scene["minimax_h3_prompt_origin"], "previous_final_frame");
    assert_eq!(scene["minimax_h3_continuity_prompt_source_scene_id"], "a");
    let frame = scene["minimax_h3_continuity_prompt_frame_path"].as_str().unwrap();
    assert!(std::path::Path::new(frame).is_file(), "the extracted frame is kept: {frame}");
    // The LLM was shown the frame: a picture block next to the text.
    let sent = seen.lock().unwrap()[0].clone();
    let content = sent["messages"][0]["content"].as_array().expect("text and picture");
    assert!(content.iter().any(|part| part["type"] == "image_url"), "{content:?}");
    assert!(content[0]["text"].as_str().unwrap().contains("FRAME-TO-FRAME CONTINUITY"));

    // A previous scene without a video is an error that says what to do.
    let mut broken = session("");
    broken["segments"][0].as_object_mut().unwrap().remove("video_path");
    std::fs::write(project.join("vrgdg_builder_session.json"), serde_json::to_string_pretty(&broken).unwrap()).unwrap();
    let error = write_before_render(&state, &project, "2", false).await.unwrap_err();
    assert!(error.contains("has no rendered video. Render it first."), "{error}");
}
