//! Project routes of the Builder: list, open, save, create, copy, package and delete.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, Multipart, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::Response;
use axum::routing::post as axum_post;
use axum::Router;
use h3_builder::payload::path_text;
use h3_builder::{capcut, projects, render_log};
use h3_project::Stamp;
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio_stream::wrappers::ReceiverStream;

use super::{get_query, post, Fail, Out};
use crate::app::AppState;

fn fields(result: Result<Value, String>) -> Result<Out, Fail> {
    result.map(Out::from).map_err(Fail::from)
}

pub fn routes(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    let base = "/vrgdg/music_builder";
    router
        .route(&format!("{base}/list_projects"), get_query(|env, query| Ok(Out::from(projects::list_projects(&env, &path_text(&query, "project_root"))))))
        .route(&format!("{base}/load_session"), post(|env, payload| fields(projects::load_session(&env, &payload))))
        .route(&format!("{base}/save_session"), post(|env, payload| fields(projects::save_session(&env, &payload, &Stamp::now()))))
        .route(&format!("{base}/new_project"), post(|env, payload| fields(projects::new_project(&env, &payload, &Stamp::now()))))
        .route(&format!("{base}/save_project_as"), post(|env, payload| fields(projects::save_project_as(&env, &payload, &Stamp::now()))))
        .route(&format!("{base}/delete_project"), post(|env, payload| fields(projects::delete_project(&env, &payload))))
        .route(&format!("{base}/save_render_log"), post(|_, payload| fields(render_log::save_render_log(&payload, &Stamp::now()))))
        .route(&format!("{base}/import_capcut_beats"), post(|_, payload| fields(capcut::find_latest_capcut_beats(payload.get("audio_duration").unwrap_or(&Value::Null)))))
        .route(&format!("{base}/export_project"), axum::routing::get(export_project))
        .route(&format!("{base}/import_project"), axum_post(import_project).layer(DefaultBodyLimit::disable()))
}

/// A folder that is removed when this goes out of scope.
struct TempFolder(PathBuf);

impl Drop for TempFolder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn millis() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis())
}

/// `GET /vrgdg/music_builder/export_project`: the project as a zip download.
async fn export_project(Query(query): Query<HashMap<String, String>>) -> Response {
    let folder = PathBuf::from(query.get("project_folder").map(|f| f.trim().trim_matches('"')).unwrap_or(""));
    let work = TempFolder(std::env::temp_dir().join(format!("vrgdg_builder_export_{}_{}", std::process::id(), millis())));
    let out = work.0.clone();
    let packed = tokio::task::spawn_blocking(move || h3_project::export::export_project(&folder, &out, &Stamp::now())).await;
    let (zip, name) = match packed {
        Ok(Ok(done)) => done,
        Ok(Err(message)) => return super::failure(StatusCode::BAD_REQUEST, message),
        Err(e) => return super::failure(StatusCode::INTERNAL_SERVER_ERROR, format!("The request failed: {e}")),
    };
    let mut file = match tokio::fs::File::open(&zip).await {
        Ok(file) => file,
        Err(e) => return super::failure(StatusCode::BAD_REQUEST, e.to_string()),
    };
    let length = file.metadata().await.map_or(0, |m| m.len());
    let (sender, receiver) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(4);
    tokio::spawn(async move {
        // The folder with the zip goes when the last chunk is sent or the download is dropped.
        let _work = work;
        let mut buffer = vec![0u8; 1024 * 1024];
        loop {
            match tokio::io::AsyncReadExt::read(&mut file, &mut buffer).await {
                Ok(0) => break,
                Ok(n) => {
                    if sender.send(Ok(Bytes::copy_from_slice(&buffer[..n]))).await.is_err() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = sender.send(Err(e)).await;
                    break;
                }
            }
        }
    });
    let mut response = Response::new(Body::from_stream(ReceiverStream::new(receiver)));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/zip"));
    if let Ok(value) = HeaderValue::from_str(&format!("attachment; filename=\"{}\"", name.replace('"', "_"))) {
        headers.insert(header::CONTENT_DISPOSITION, value);
    }
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(length));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// `POST /vrgdg/music_builder/import_project`: a project package (multipart: `project_zip` and `project_name`).
async fn import_project(State(state): State<Arc<AppState>>, mut form: Multipart) -> Response {
    let upload = std::env::temp_dir().join(format!("vrgdg_builder_import_{}_{}.zip", std::process::id(), millis()));
    let result = read_and_import(&state, &mut form, &upload).await;
    let _ = tokio::fs::remove_file(&upload).await;
    super::into_response(result.map(Out::from).map_err(Fail::from))
}

async fn read_and_import(state: &Arc<AppState>, form: &mut Multipart, upload: &Path) -> Result<Value, String> {
    let mut name = String::new();
    let mut saved = false;
    while let Some(mut field) = form.next_field().await.map_err(|e| e.to_string())? {
        match field.name().unwrap_or("") {
            "project_name" => name = field.text().await.map_err(|e| e.to_string())?.trim().to_string(),
            "project_zip" => {
                let mut file = tokio::fs::File::create(upload).await.map_err(|e| e.to_string())?;
                while let Some(chunk) = field.chunk().await.map_err(|e| e.to_string())? {
                    file.write_all(&chunk).await.map_err(|e| e.to_string())?;
                }
                file.flush().await.map_err(|e| e.to_string())?;
                saved = true;
            }
            _ => {}
        }
    }
    if !saved {
        return Err("Choose a .vrgdg.zip project package to import.".into());
    }
    let env = state.builder_env();
    let path = upload.to_path_buf();
    tokio::task::spawn_blocking(move || projects::import_project(&env, &path, &name, &Stamp::now())).await.map_err(|e| format!("The request failed: {e}"))?
}
