//! Edits that arrive at the same time. Each one reads the project, changes it and saves it, so none may start from a revision another has just replaced.

use h3_project::AppConfig;
use reqwest::Client;
use serde_json::{json, Value};

#[tokio::test]
async fn settings_patched_at_the_same_time_all_land() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("projects");
    let project = root.join("Demo");
    std::fs::create_dir_all(&project).unwrap();
    let session = json!({"project_name": "Demo", "revision": 3, "builder_save_revision": 3, "segments": [{"id": "seg_0001", "start": 0.0, "end": 4.0}]});
    std::fs::write(project.join("vrgdg_builder_session.json"), serde_json::to_string_pretty(&session).unwrap()).unwrap();
    let cfg = AppConfig { comfy_url: "http://127.0.0.1:9".into(), project_root: root, ..AppConfig::default() };
    let state = h3_api::build_state(cfg, &dir.path().join("jobs.db")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/api/v1", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, h3_api::router(state)).await.unwrap() });

    // One patch per setting, all sent at once and none with an If-Match, so each wins or loses only by the order the server takes them in.
    let patches = [
        json!({"project": {"timing_frozen": true}}),
        json!({"project": {"auto_save_enabled": false}}),
        json!({"project": {"omit_lyrics_from_video_prompts": true}}),
        json!({"project": {"automatic_memory_cleanup": true}}),
        json!({"project": {"scene_render_wait_hours": 3}}),
        json!({"project": {"video_type": "story"}}),
    ];
    let client = Client::new();
    let mut calls = tokio::task::JoinSet::new();
    for patch in patches {
        let (client, url) = (client.clone(), format!("{base}/projects/Demo/settings"));
        calls.spawn(async move {
            let response = client.patch(url).json(&patch).send().await.unwrap();
            (response.status().as_u16(), response.json::<Value>().await.unwrap_or(Value::Null))
        });
    }
    while let Some(done) = calls.join_next().await {
        let (code, body) = done.unwrap();
        assert_eq!(code, 200, "no patch is refused or lost to another: {body}");
    }

    let saved = h3_project::load_session(&project).unwrap().session;
    assert_eq!(saved["timing_frozen"], true);
    assert_eq!(saved["auto_save_enabled"], false);
    assert_eq!(saved["omit_lyrics_from_video_prompts"], true);
    assert_eq!(saved["automatic_memory_cleanup"], true);
    assert_eq!(saved["scene_render_wait_hours"], 3);
    assert_eq!(saved["video_type"], "story");
}
