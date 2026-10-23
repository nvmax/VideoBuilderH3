//! The Video Builder's "create each next scene prompt from the previous rendered final frame". A scene continued with
//! `latent_continuation_masked` and `continuity_prompt_from_last_frame` gets its prompt written from the real last frame of
//! the scene before it, right before it renders. Port of the step in `agent_api/orchestrator/video_orchestrator.py` and of
//! `write_continued_scene_prompt` in `minimax_prompt_orchestrator.py`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use h3_core::pyval::{text_or_empty, truthy, Obj};
use h3_core::timeline::sort_segments;
use h3_media::video::extract_final_frame;
use h3_media::Tools;
use h3_pipeline::minimax_prompts::{generate_scene_prompt, PromptParams};
use h3_pipeline::SceneHook;
use h3_project::{load_session, session_ops, Stamp};
use serde_json::{json, Value};

use crate::app::AppState;
use crate::llm_routes::current;

/// The hook a render context runs before each scene.
pub fn hook(state: &Arc<AppState>) -> SceneHook {
    let state = Arc::clone(state);
    Arc::new(move |project: PathBuf, scene: String, has_prompt: bool| {
        let state = Arc::clone(&state);
        Box::pin(async move { write_before_render(&state, &project, &scene, has_prompt).await })
    })
}

fn video_of(segment: &Value) -> String {
    ["video_path", "rendered_video_path"].iter().map(|k| text_or_empty(segment.get(*k)).trim().to_string()).find(|p| !p.is_empty()).unwrap_or_default()
}

/// Write the scene's prompt from the previous scene's final frame when the scene is set up for it. Does nothing for a
/// scene that is not.
pub async fn write_before_render(state: &AppState, project: &Path, scene_ref: &str, has_prompt: bool) -> Result<(), String> {
    let folder = project.to_path_buf();
    let loaded = tokio::task::spawn_blocking({
        let folder = folder.clone();
        move || load_session(&folder)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    let session: Obj = loaded.session;
    let mut segments: Vec<Value> = session.get("segments").and_then(Value::as_array).cloned().unwrap_or_default();
    sort_segments(&mut segments);
    let Some(index) = segments.iter().position(|s| text_or_empty(s.get("id")) == scene_ref).or_else(|| scene_ref.parse::<usize>().ok().filter(|n| *n >= 1 && *n <= segments.len()).map(|n| n - 1)) else {
        // The render itself reports a scene that is not there.
        return Ok(());
    };
    let number = index + 1;
    let settings = h3_core::settings::settings_for_scene(&Value::Object(session.clone()), Some(&segments[index]));
    let wanted = number > 1 && !has_prompt && settings.get("continuity_mode").and_then(Value::as_str) == Some("latent_continuation_masked") && settings.get("continuity_prompt_from_last_frame").is_some_and(truthy);
    if !wanted {
        return Ok(());
    }
    let previous = &segments[index - 1];
    let previous_video = video_of(previous);
    if previous_video.is_empty() || !Path::new(&previous_video).is_file() {
        return Err(format!("Scene {number} is set to write its prompt from scene {}'s final frame, but scene {} has no rendered video. Render it first.", number - 1, number - 1));
    }
    eprintln!("[VRGDG-RS Render] Writing the scene {number} prompt from scene {}'s final frame...", number - 1);
    let tools = Tools::find(&state.cfg.ffmpeg_path, &state.cfg.ffprobe_path).map_err(|e| e.to_string())?;
    let frame = tokio::task::spawn_blocking({
        let (folder, source) = (folder.clone(), PathBuf::from(&previous_video));
        move || extract_final_frame(&tools, &folder, &source, number as u32, &Stamp::now().compact)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;

    let params = PromptParams { previous_frame: Some(frame.clone()), ..PromptParams::default() };
    let written = generate_scene_prompt(&current(state), &session, index, &params).await.map_err(|e| e.to_string())?;
    for warning in &written.warnings {
        eprintln!("[VRGDG-RS Render] Scene {number}: {warning}");
    }
    let mut fields = Obj::new();
    fields.insert("minimax_h3_prompt".into(), json!(written.prompt));
    fields.insert("minimax_h3_prompt_origin".into(), json!(if written.used_previous_frame { "previous_final_frame" } else { "gemma" }));
    fields.insert("minimax_h3_mode".into(), json!("reference_to_video"));
    fields.insert("video_prompt_type".into(), json!("rtv"));
    fields.insert("minimax_h3_continuity_prompt_source_scene_id".into(), json!(text_or_empty(previous.get("id"))));
    fields.insert("minimax_h3_continuity_prompt_frame_path".into(), json!(if written.used_previous_frame { frame.display().to_string() } else { String::new() }));
    fields.insert("minimax_h3_continuity_prompt_created_at".into(), json!(Stamp::now().iso));
    let id = text_or_empty(segments[index].get("id"));
    let _guard = state.edit_lock.lock().await;
    tokio::task::spawn_blocking(move || session_ops::set_scene_fields(&folder, &id, &fields, &Stamp::now()))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    Ok(())
}
