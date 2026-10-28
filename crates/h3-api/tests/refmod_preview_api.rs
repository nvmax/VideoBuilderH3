//! A RefMod's preview can be checked with If-Modified-Since, so a page that draws it again gets a 304 instead of the picture.

use std::path::Path;

use h3_project::AppConfig;
use reqwest::header::{CACHE_CONTROL, IF_MODIFIED_SINCE, LAST_MODIFIED};
use reqwest::Client;
use serde_json::json;

/// A ComfyUI that only tells where its folders are, with one saved RefMod and its preview.
async fn fake_comfy(refmods: &Path) -> String {
    let path = refmods.join("background/Street.safetensors");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let meta = json!({"kind": "image", "latent_t": 1, "latent_h": 8, "latent_w": 8, "concept_type": "background"});
    let header = serde_json::to_vec(&json!({"__metadata__": {"refmod_meta": meta.to_string()}})).unwrap();
    let mut bytes = (header.len() as u64).to_le_bytes().to_vec();
    bytes.extend(header);
    std::fs::write(&path, bytes).unwrap();
    image::RgbImage::from_pixel(8, 8, image::Rgb([20, 120, 220])).save(refmods.join("background/Street.preview.png")).unwrap();
    let folders = json!({"refmods": [refmods.display().to_string()]});
    let app = axum::Router::new().route("/internal/folder_paths", axum::routing::get(move || async move { axum::Json(folders) }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

#[tokio::test]
async fn an_unchanged_preview_is_a_304() {
    let dir = tempfile::tempdir().unwrap();
    let comfy = fake_comfy(&dir.path().join("refmods")).await;
    let cfg = AppConfig { comfy_url: comfy, project_root: dir.path().join("projects"), ..AppConfig::default() };
    let state = h3_api::build_state(cfg, &dir.path().join("jobs.db")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, h3_api::router(state)).await.unwrap() });
    let url = format!("http://{addr}/vrgdg/refmod/preview?name=background%2FStreet");
    let client = Client::new();

    let first = client.get(&url).send().await.unwrap();
    assert_eq!(first.status().as_u16(), 200);
    assert_eq!(first.headers()[CACHE_CONTROL], "no-cache");
    assert_eq!(first.headers()["content-type"], "image/png");
    let modified = first.headers()[LAST_MODIFIED].clone();
    assert!(!first.bytes().await.unwrap().is_empty());

    let again = client.get(&url).header(IF_MODIFIED_SINCE, modified).send().await.unwrap();
    assert_eq!(again.status().as_u16(), 304);
    assert!(again.bytes().await.unwrap().is_empty());

    let missing = client.get(format!("http://{addr}/vrgdg/refmod/preview?name=background%2FNope")).send().await.unwrap();
    assert_eq!(missing.status().as_u16(), 404);
}
