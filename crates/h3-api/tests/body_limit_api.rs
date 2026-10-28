//! Routes that take a picture, a song or a file in the request body accept more than axum's 2 MB default.

use base64::Engine;
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

/// A PNG of about 3.6 MB: noise, stored without compression.
fn big_png() -> Vec<u8> {
    let mut seed: u32 = 1;
    let pixels = (0..1100 * 1100 * 3).map(|_| {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed >> 24) as u8
    });
    let picture = image::RgbImage::from_raw(1100, 1100, pixels.collect()).unwrap();
    let mut bytes = std::io::Cursor::new(Vec::new());
    picture.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    bytes.into_inner()
}

fn data_url(png: &[u8]) -> String {
    format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png))
}

#[tokio::test]
async fn a_scene_picture_over_two_megabytes_is_saved() {
    let (base, dir) = start().await;
    let project = dir.path().join("projects/Song");
    std::fs::create_dir_all(&project).unwrap();
    let body = json!({"project_folder": project.display().to_string(), "scene_number": 1, "image_data": data_url(&big_png())});
    let response = Client::new().post(format!("{base}/vrgdg/music_builder/save_scene_image")).json(&body).send().await.unwrap();
    let status = response.status().as_u16();
    let reply: Value = response.json().await.unwrap();
    assert_eq!(status, 200, "{reply}");
    let saved = reply["saved_path"].as_str().unwrap();
    assert!(std::fs::metadata(saved).unwrap().len() > 2 * 1024 * 1024);
}

#[tokio::test]
async fn a_reference_picture_over_two_megabytes_is_saved() {
    let (base, dir) = start().await;
    let project = dir.path().join("projects/Song");
    std::fs::create_dir_all(&project).unwrap();
    let body = json!({"project_folder": project.display().to_string(), "reference_type": "subject", "name": "Anna", "image_data": data_url(&big_png())});
    let response = Client::new().post(format!("{base}/vrgdg/music_builder/save_flux_reference_image")).json(&body).send().await.unwrap();
    let status = response.status().as_u16();
    let reply: Value = response.json().await.unwrap();
    assert_eq!(status, 200, "{reply}");
    let saved = reply["saved_path"].as_str().unwrap();
    assert!(std::fs::metadata(saved).unwrap().len() > 2 * 1024 * 1024);
}

#[tokio::test]
async fn a_song_over_two_megabytes_reaches_the_agent_api() {
    let (base, _dir) = start().await;
    // About 3 MB of base64. The project does not exist, so the request is answered 404 once the body has been read.
    let body = json!({"audio_name": "song.wav", "audio_data": format!("data:audio/wav;base64,{}", "A".repeat(3 * 1024 * 1024))});
    let response = Client::new().put(format!("{base}/api/v1/projects/nothing/audio")).json(&body).send().await.unwrap();
    assert_eq!(response.status().as_u16(), 404);
}

#[tokio::test]
async fn a_lut_over_two_megabytes_reaches_the_route() {
    let (base, _dir) = start().await;
    let mut cube = String::from("LUT_3D_SIZE 55\n");
    for index in 0..55 * 55 * 55 {
        cube.push_str(&format!("{:.6} {:.6} {:.6}\n", (index % 55) as f64 / 54.0, (index / 55 % 55) as f64 / 54.0, (index / 3025) as f64 / 54.0));
    }
    assert!(cube.len() > 2 * 1024 * 1024, "{}", cube.len());
    let response = Client::new().post(format!("{base}/api/v1/post/luts/upload?filename=big.cube")).body(cube).send().await.unwrap();
    let status = response.status().as_u16();
    let reply: Value = response.json().await.unwrap();
    assert_eq!(status, 200, "{reply}");
}
