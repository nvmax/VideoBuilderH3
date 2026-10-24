//! Tauri entry point. The app serves the same `/api/v1` the agents use and the UI talks to it, so there is one code
//! path for the window and for outside tools.

use h3_project::AppConfig;

/// The address the UI uses for the API.
#[tauri::command]
fn api_base() -> String {
    let cfg = AppConfig::load(&AppConfig::default_path()).unwrap_or_default();
    format!("http://{}/api/v1", cfg.api_bind)
}

/// Start the app again so saved settings (ComfyUI address, folders, ffmpeg) take effect.
#[tauri::command]
fn restart_app(app: tauri::AppHandle) {
    app.restart();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|_app| {
            let config_path = AppConfig::default_path();
            let cfg = AppConfig::load(&config_path).map_err(|e| format!("The config file could not be read: {e}"))?;
            let state = h3_api::build_state(cfg, &h3_api::jobs_db_path(&config_path))?;
            tauri::async_runtime::spawn(async move {
                if let Err(e) = h3_api::serve(state).await {
                    eprintln!("[VRGDG-RS API] The API stopped: {e}");
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![api_base, restart_app])
        .run(tauri::generate_context!())
        .expect("error while running the VRGDG H3 Studio app");
}
