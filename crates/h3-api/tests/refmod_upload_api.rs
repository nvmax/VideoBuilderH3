//! RefMods Studio picture uploads larger than axum's 2 MB default body limit.

use h3_project::AppConfig;
use reqwest::multipart::{Form, Part};
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

#[tokio::test]
async fn a_picture_over_two_megabytes_uploads() {
    let (base, _dir) = start().await;
    let png = big_png();
    assert!(png.len() > 2 * 1024 * 1024, "{}", png.len());
    let form = Form::new().part("image", Part::bytes(png.clone()).file_name("big.png"));
    let response = Client::new().post(format!("{base}/vrgdg/refmod/upload_image")).multipart(form).send().await.unwrap();
    let status = response.status().as_u16();
    let body: Value = response.json().await.unwrap();
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["ok"], true, "{body}");
    let path = body["path"].as_str().unwrap();
    assert_eq!(std::fs::read(path).unwrap().len(), png.len());
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn a_base64_picture_over_two_megabytes_gets_past_the_body_limit() {
    use base64::Engine;
    let (base, _dir) = start().await;
    let data = base64::engine::general_purpose::STANDARD.encode(big_png());
    let body = json!({"image": format!("data:image/png;base64,{data}"), "name": "big.png"});
    let response = Client::new().post(format!("{base}/vrgdg/refmod/from_image")).json(&body).send().await.unwrap();
    // ComfyUI is down, so the request fails later on, but not with axum's 413 for a body over 2 MB.
    assert_ne!(response.status().as_u16(), 413);
}
