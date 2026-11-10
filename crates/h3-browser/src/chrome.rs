//! Finding and starting the person's Chrome with a debug port. Port of `_chrome_exe`, `_is_debug_chrome_ready` and `_start_debug_chrome`
//! of the pack's `browser/nodes.py`.
//!
//! Each provider gets its own Chrome profile folder, so the sign-in to a site stays between runs. The window is the person's own
//! Chrome, visible, with a real profile: that is what lets them sign in to the site and pass its checks.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::{BrowserError, Result};

/// Where Chrome is usually installed on this computer, in the order to try.
fn candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if cfg!(windows) {
        for variable in ["ProgramFiles", "ProgramFiles(x86)", "LocalAppData"] {
            if let Ok(base) = std::env::var(variable) {
                out.push(PathBuf::from(base).join("Google").join("Chrome").join("Application").join("chrome.exe"));
            }
        }
    } else if cfg!(target_os = "macos") {
        out.push("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".into());
        out.push("/Applications/Chromium.app/Contents/MacOS/Chromium".into());
    } else {
        for path in ["/usr/bin/google-chrome-stable", "/usr/bin/google-chrome", "/usr/bin/chromium", "/usr/bin/chromium-browser", "/snap/bin/chromium"] {
            out.push(path.into());
        }
    }
    out
}

/// `VRGDG_CHROME_PATH` or `CHROME_PATH` names a Chrome to use instead of the one that is found.
fn configured() -> Option<String> {
    ["VRGDG_CHROME_PATH", "CHROME_PATH"].iter().filter_map(|name| std::env::var(name).ok()).map(|v| v.trim().to_string()).find(|v| !v.is_empty())
}

/// The Chrome to start: the configured one, or the first installed one. The error says what to do.
pub fn chrome_exe() -> Result<PathBuf> {
    pick(configured(), &candidates())
}

fn pick(configured: Option<String>, candidates: &[PathBuf]) -> Result<PathBuf> {
    if let Some(path) = configured {
        let path = PathBuf::from(&path);
        return if path.is_file() { Ok(path) } else { Err(BrowserError::Chrome(format!("The Chrome set in VRGDG_CHROME_PATH was not found: {}", path.display()))) };
    }
    candidates
        .iter()
        .find(|path| path.is_file())
        .cloned()
        .ok_or_else(|| BrowserError::Chrome("Could not find Google Chrome. Install Chrome, or set VRGDG_CHROME_PATH to the browser's executable.".into()))
}

/// Is a Chrome with a debug port answering on `port`?
pub async fn is_ready(port: u16) -> bool {
    let Ok(client) = reqwest::Client::builder().timeout(Duration::from_secs(1)).build() else { return false };
    matches!(client.get(format!("http://127.0.0.1:{port}/json/version")).send().await, Ok(response) if response.status().is_success())
}

/// What Chrome is started with. `--remote-debugging-port` needs its own `--user-data-dir`, which is also what keeps the sign-in.
fn arguments(port: u16, profile_dir: &Path, url: &str) -> Vec<String> {
    vec![format!("--remote-debugging-port={port}"), format!("--user-data-dir={}", profile_dir.display()), "--window-size=1600,950".into(), url.into()]
}

/// Start Chrome on `url` with the debug port and the profile folder, unless one already answers on the port. Waits until the port is open.
pub async fn start_debug_chrome(profile_dir: &Path, port: u16, url: &str) -> Result<()> {
    if is_ready(port).await {
        return Ok(());
    }
    let exe = chrome_exe()?;
    std::fs::create_dir_all(profile_dir).map_err(|e| BrowserError::File(format!("Could not create the Chrome profile folder {}: {e}", profile_dir.display())))?;
    let mut command = std::process::Command::new(exe);
    command.args(arguments(port, profile_dir, url)).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NEW_PROCESS_GROUP: closing the app does not close the person's Chrome window.
        command.creation_flags(0x0000_0200);
    }
    command.spawn().map_err(|e| BrowserError::Chrome(format!("Chrome could not be started: {e}")))?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(25);
    while tokio::time::Instant::now() < deadline {
        if is_ready(port).await {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Err(BrowserError::Chrome(format!("Chrome debug port {port} did not become ready. If this Chrome profile is already open without a debug port, close that window first.")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_configured_chrome_wins_and_a_wrong_one_is_an_error_not_a_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("chrome.exe");
        std::fs::write(&fake, b"").unwrap();
        let installed = dir.path().join("installed.exe");
        std::fs::write(&installed, b"").unwrap();
        assert_eq!(pick(Some(fake.display().to_string()), std::slice::from_ref(&installed)).unwrap(), fake);
        let error = pick(Some(dir.path().join("nope.exe").display().to_string()), std::slice::from_ref(&installed)).unwrap_err();
        assert!(error.to_string().starts_with("The Chrome set in VRGDG_CHROME_PATH was not found"), "{error}");
        // Without a configured one, the first installed candidate is used.
        assert_eq!(pick(None, &[dir.path().join("missing.exe"), installed.clone()]).unwrap(), installed);
        assert!(pick(None, &[dir.path().join("missing.exe")]).unwrap_err().to_string().starts_with("Could not find Google Chrome"));
    }

    #[test]
    fn chrome_is_started_with_its_own_profile_and_the_debug_port() {
        let args = arguments(9223, Path::new("C:/data/chrome-chatgpt-profile"), "https://chatgpt.com/images");
        assert_eq!(args, ["--remote-debugging-port=9223", "--user-data-dir=C:/data/chrome-chatgpt-profile", "--window-size=1600,950", "https://chatgpt.com/images"]);
    }

    #[tokio::test]
    async fn a_port_with_a_debug_chrome_is_ready_and_a_closed_port_is_not() {
        use axum::routing::get;
        let app = axum::Router::new().route("/json/version", get(|| async { axum::Json(serde_json::json!({"Browser": "Chrome/120"})) }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        assert!(is_ready(port).await);
        assert!(!is_ready(9).await);
        // An already-open debug Chrome is used as it is: nothing is started.
        start_debug_chrome(Path::new("unused"), port, "https://example.com").await.unwrap();
    }
}
