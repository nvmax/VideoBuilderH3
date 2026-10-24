//! The app's screens (`app/web`), embedded in the program. The desktop app opens its window on the API server's own address, so the Builder, its media
//! and the API share one origin, like the Builder does inside ComfyUI. `h3 serve --web <folder>` serves a folder from disk instead, for working on the
//! screens.

use axum::body::Body;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::Response;
use include_dir::{include_dir, Dir};

static WEB: Dir = include_dir!("$CARGO_MANIFEST_DIR/../../app/web");

/// The embedded file for a request path (`/` is the start page), or None.
pub fn file(path: &str) -> Option<Response> {
    let clean = path.trim_start_matches('/');
    let name = if clean.is_empty() || clean.ends_with('/') { format!("{clean}index.html") } else { clean.to_string() };
    let found = WEB.get_file(&name)?;
    let kind = mime_guess::from_path(&name).first_or_octet_stream();
    let mut reply = Response::new(Body::from(found.contents()));
    *reply.status_mut() = StatusCode::OK;
    reply.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_str(kind.essence_str()).unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")));
    // The screens change with the app, so the window checks them each time.
    reply.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    Some(reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_start_page_and_the_builder_are_embedded_with_script_types() {
        let page = file("/").expect("start page");
        assert!(page.headers()[header::CONTENT_TYPE].to_str().unwrap().starts_with("text/html"));
        let module = file("/extensions/comfyui-vrgamedevgirl/music_video_builder/builder.mjs").expect("the Builder");
        let kind = module.headers()[header::CONTENT_TYPE].to_str().unwrap().to_string();
        assert!(kind.contains("javascript"), "{kind}");
        assert!(file("/host/host.css").is_some());
        assert!(file("/nothing/here.js").is_none());
    }
}
