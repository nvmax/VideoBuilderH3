//! The app gzips the text it sends and unpacks a gzipped request body, which matters when it is reached over a slow link.

use std::io::{Read, Write};

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use h3_project::AppConfig;
use reqwest::Client;
use serde_json::{json, Value};

async fn start() -> (String, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let cfg = AppConfig {
        // Nothing listens on port 9, so ComfyUI counts as down.
        comfy_url: "http://127.0.0.1:9".into(),
        project_root: dir.path().join("projects"),
        ..AppConfig::default()
    };
    let state = h3_api::build_state(cfg, &dir.path().join("jobs.db")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, h3_api::router(state)).await.unwrap() });
    (format!("http://{addr}"), dir)
}

fn gunzip(bytes: &[u8]) -> String {
    let mut text = String::new();
    GzDecoder::new(bytes).read_to_string(&mut text).unwrap();
    text
}

#[tokio::test]
async fn a_screen_is_gzipped_for_a_browser_that_asks() {
    let (base, _dir) = start().await;
    let plain = Client::new().get(format!("{base}/host/boot.js")).send().await.unwrap();
    assert_eq!(plain.status().as_u16(), 200);
    assert!(plain.headers().get("content-encoding").is_none());
    let plain = plain.text().await.unwrap();
    let packed = Client::new().get(format!("{base}/host/boot.js")).header("Accept-Encoding", "gzip").send().await.unwrap();
    assert_eq!(packed.headers()["content-encoding"], "gzip");
    let bytes = packed.bytes().await.unwrap();
    assert!(bytes.len() < plain.len());
    assert_eq!(gunzip(&bytes), plain);
}

#[tokio::test]
async fn a_video_is_sent_as_it_is() {
    let (base, dir) = start().await;
    let video = dir.path().join("clip.mp4");
    std::fs::write(&video, vec![b'a'; 64 * 1024]).unwrap();
    let url = format!("{base}/vrgdg/video_editor/video?path={}", video.display());
    let reply = Client::new().get(&url).header("Accept-Encoding", "gzip").send().await.unwrap();
    assert_eq!(reply.status().as_u16(), 200);
    assert!(reply.headers().get("content-encoding").is_none());
    assert_eq!(reply.bytes().await.unwrap().len(), 64 * 1024);
    let part = Client::new().get(&url).header("Accept-Encoding", "gzip").header("Range", "bytes=0-99").send().await.unwrap();
    assert_eq!(part.status().as_u16(), 206);
    assert_eq!(part.bytes().await.unwrap().len(), 100);
}

#[tokio::test]
async fn a_gzipped_request_body_is_unpacked() {
    let (base, dir) = start().await;
    let path = dir.path().join("projects/Song/notes.txt");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let content = "line\n".repeat(100_000);
    let body = serde_json::to_vec(&json!({"path": path.display().to_string(), "content": content})).unwrap();
    let mut packer = GzEncoder::new(Vec::new(), flate2::Compression::default());
    packer.write_all(&body).unwrap();
    let packed = packer.finish().unwrap();
    let reply = Client::new()
        .post(format!("{base}/vrgdg/music_builder/save_text_file"))
        .header("Content-Type", "application/json")
        .header("Content-Encoding", "gzip")
        .body(packed)
        .send()
        .await
        .unwrap();
    let status = reply.status().as_u16();
    let reply: Value = reply.json().await.unwrap();
    assert_eq!(status, 200, "{reply}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
}
