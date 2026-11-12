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

    /// The app is only the Builder, so it fills the window and never shows the floating panel it has inside ComfyUI. That takes both halves: the
    /// Builder opens filling the window, and the toolbar's fullscreen button, the only thing that returns it to the panel, is hidden. A sync that
    /// brings in a Builder that opens as a panel, or a label that changes, must not leave the window without a way back.
    #[test]
    fn the_builder_always_fills_the_window() {
        let text = |name: &str| WEB.get_file(name).and_then(|f| f.contents_utf8()).unwrap_or_else(|| panic!("{name} is not embedded")).to_string();
        let css = text("host/host.css");
        for label in ["Enter fullscreen", "Exit fullscreen"] {
            assert!(css.contains(&format!("button[aria-label=\"{label}\"]")), "host.css does not hide the {label} button");
        }
        let builder = text("extensions/comfyui-vrgamedevgirl/music_video_builder/builder.mjs");
        assert!(builder.contains("applyBuilderFullscreen(true);"), "the Builder no longer opens filling the window");
        // Two calls: the one that opens it filling the window, and the one of the button. A third is something else that could change it.
        let calls: usize = WEB
            .get_dir("extensions/comfyui-vrgamedevgirl/music_video_builder")
            .expect("the Builder's folder")
            .files()
            .filter_map(|f| f.contents_utf8())
            .map(|t| t.matches("applyBuilderFullscreen(").count() - t.matches("function applyBuilderFullscreen(").count())
            .sum();
        assert_eq!(calls, 2, "something else now switches the Builder out of fullscreen, or the button's call moved");
    }
}
