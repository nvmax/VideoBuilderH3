//! Only this computer may use the server. The API has no password, so a web page open in the user's browser must not be able to call it: such a page could
//! change the settings (the ffmpeg path, for one), delete projects or start renders. Two checks run before every route, the REST API, the Builder's
//! `/vrgdg` routes, the ComfyUI pass-through and `/mcp`:
//!
//! - A request that carries an `Origin` (a browser sends one with every request a page makes to another address, and with every POST) must name this computer:
//!   `localhost`, a loopback address or a `*.localhost` name (the app's own window). `Origin: null`, which a sandboxed page sends, is refused too.
//! - The `Host` must be `localhost`, a `*.localhost` name or an IP address. A name that points at this computer only because an attacker's DNS says so (rebinding)
//!   is refused.
//!
//! A request with no `Origin` (curl, an agent, the MCP bridge, the app's own page reading its own files) passes. CORS answers only the origins that pass.

use axum::extract::Request;
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// The name in `host:port`, `[ipv6]:port` or a bare host.
fn host_name(authority: &str) -> &str {
    match authority.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or(""),
        None => authority.rsplit_once(':').map_or(authority, |(name, _)| name),
    }
}

/// Whether a `Host` header names this computer: `localhost`, a `*.localhost` name or an IP address.
pub fn host_allowed(host: &str) -> bool {
    let name = host_name(host.trim());
    name.eq_ignore_ascii_case("localhost") || name.to_ascii_lowercase().ends_with(".localhost") || name.parse::<std::net::IpAddr>().is_ok()
}

/// Whether an `Origin` header is a page of this computer: `localhost`, a `*.localhost` name (the app window) or a loopback address.
pub fn origin_allowed(origin: &str) -> bool {
    let authority = origin.trim().split("://").nth(1).unwrap_or("").split('/').next().unwrap_or("");
    let name = host_name(authority);
    name.eq_ignore_ascii_case("localhost") || name.to_ascii_lowercase().ends_with(".localhost") || name.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

fn refuse(code: &'static str, message: &str) -> Response {
    (StatusCode::FORBIDDEN, Json(json!({"ok": false, "error": {"code": code, "message": message, "details": {}, "retryable": false}}))).into_response()
}

/// What is wrong with the request's `Origin` or `Host`, if anything: the error code and message.
pub fn violation(headers: &HeaderMap) -> Option<(&'static str, &'static str)> {
    let text = |name| headers.get(name).map(|v| v.to_str().unwrap_or("\u{0}invalid"));
    if text(header::ORIGIN).is_some_and(|origin| !origin_allowed(origin)) {
        return Some(("FOREIGN_ORIGIN", "This server only answers pages and programs on this computer."));
    }
    if text(header::HOST).is_some_and(|host| !host_allowed(host)) {
        return Some(("FOREIGN_HOST", "This server only answers requests addressed to localhost or an IP address."));
    }
    None
}

/// The layer that runs before every route.
pub async fn only_this_computer(request: Request, next: Next) -> Response {
    match violation(request.headers()) {
        Some((code, message)) => refuse(code, message),
        None => next.run(request).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_are_this_computer_or_an_address() {
        for host in ["127.0.0.1:8190", "localhost:8190", "LOCALHOST", "[::1]:8190", "192.168.1.20:8190", "app.localhost:1420"] {
            assert!(host_allowed(host), "{host}");
        }
        for host in ["evil.example:8190", "rebind.attacker.com", "localhost.evil.example", "127.0.0.1.evil.example:8190"] {
            assert!(!host_allowed(host), "{host}");
        }
    }

    #[test]
    fn origins_are_pages_of_this_computer() {
        for origin in ["http://127.0.0.1:8190", "http://localhost:3000", "http://tauri.localhost", "http://[::1]:8190", "https://localhost"] {
            assert!(origin_allowed(origin), "{origin}");
        }
        for origin in ["https://evil.example", "null", "", "http://192.168.1.20:8190", "http://localhost.evil.example", "file://"] {
            assert!(!origin_allowed(origin), "{origin}");
        }
    }
}
