// The window shows a page from the app's own server, which counts as remote content for Tauri. Remote content may only call the commands
// that capabilities/default.json lists (as `allow-<command>`), so every command the page uses is named here.
const COMMANDS: &[&str] = &["restart_app", "close_popups", "app_version", "update_check", "update_progress", "update_install"];

fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(COMMANDS))).expect("failed to run tauri-build");
}
