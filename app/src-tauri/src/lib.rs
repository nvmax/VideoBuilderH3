//! Tauri entry point. The app serves the same `/api/v1` the agents use and the UI talks to it, so there is one code
//! path for the window and for outside tools.

use std::sync::atomic::{AtomicU32, Ordering};

use h3_project::AppConfig;
use tauri::webview::{NewWindowFeatures, NewWindowResponse, WebviewWindowBuilder};
use tauri::{Manager, WebviewUrl, WindowEvent};
use tauri_plugin_opener::OpenerExt;

/// Start the app again so saved settings (ComfyUI address, folders, ffmpeg) take effect.
#[tauri::command]
fn restart_app(app: tauri::AppHandle) {
    app.restart();
}

static POPUPS: AtomicU32 = AtomicU32::new(0);

/// What `window.open` asks for. The Builder's pop-outs (the preview player, the Builder Agent) open a blank window and fill it, so they get a window
/// of their own that can go to another screen. A page on another site opens in the system browser.
fn open_popup(app: &tauri::AppHandle, url: tauri::Url, features: NewWindowFeatures) -> NewWindowResponse<tauri::Wry> {
    let own_page = matches!(url.host_str(), Some("tauri.localhost" | "127.0.0.1" | "localhost")) || url.scheme() == "about";
    if matches!(url.scheme(), "http" | "https") && !own_page {
        if let Err(error) = app.opener().open_url(url.as_str(), None::<&str>) {
            eprintln!("[VRGDG-RS Window] Could not open {url}: {error}");
        }
        return NewWindowResponse::Deny;
    }
    let label = format!("popup-{}", POPUPS.fetch_add(1, Ordering::Relaxed));
    let blank = WebviewUrl::External("about:blank".parse().expect("a valid address"));
    match WebviewWindowBuilder::new(app, label, blank).window_features(features).title("VRGDG H3 Studio").on_document_title_changed(|window, title| drop(window.set_title(&title))).build() {
        Ok(window) => NewWindowResponse::Create { window },
        Err(error) => {
            eprintln!("[VRGDG-RS Window] Could not open the pop-out window: {error}");
            NewWindowResponse::Deny
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let config_path = AppConfig::default_path();
            let cfg = AppConfig::load(&config_path).map_err(|e| format!("The config file could not be read: {e}"))?;
            let state = h3_api::build_state(cfg, &h3_api::jobs_db_path(&config_path))?;
            // The address is taken before the window opens, so the window finds the server there. The window shows the Builder from the server's own
            // address: the page, its media and the API share one origin, like the Builder inside ComfyUI.
            let listener = tauri::async_runtime::block_on(h3_api::bind(&state)).map_err(|e| format!("The address {} could not be used: {e}", state.cfg.api_bind))?;
            let address: tauri::Url = format!("http://127.0.0.1:{}/", listener.local_addr()?.port()).parse()?;
            tauri::async_runtime::spawn(async move {
                if let Err(e) = h3_api::serve_on(listener, state).await {
                    eprintln!("[VRGDG-RS API] The API stopped: {e}");
                }
            });
            let handle = app.handle().clone();
            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(address))
                .title("VRGDG H3 Studio")
                .inner_size(1400.0, 900.0)
                .on_new_window(move |url, features| open_popup(&handle, url, features))
                .build()?;
            Ok(())
        })
        // The pop-outs belong to the main window: closing it ends the app.
        .on_window_event(|window, event| {
            if window.label() == "main" && matches!(event, WindowEvent::Destroyed) {
                window.app_handle().exit(0);
            }
        })
        .invoke_handler(tauri::generate_handler![restart_app])
        .run(tauri::generate_context!())
        .expect("error while running the VRGDG H3 Studio app");
}
