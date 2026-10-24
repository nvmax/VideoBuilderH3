//! Audio analysis, latent status and scene video routes of the Builder. Thin ports of the handlers in `builder/routes.py`
//! over the project, latent and media code the app already has.

use std::path::{Path, PathBuf};

use h3_core::pyval::{to_int, Obj};
use h3_media::peaks::{estimate_beats_from_peaks, read_audio_peaks, tempo_from_beat_times};
use h3_media::video::{restore_scene_video, Restored};
use h3_project::layout::Stamp;
use h3_project::{assets, latents, takes};
use serde_json::{json, Value};

use crate::files::resolve_existing_file;
use crate::payload::path_text;
use crate::Env;

type Reply = Result<Value, String>;

fn text(path: &Path) -> String {
    path.display().to_string()
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()).components().collect()
}

/// `int(payload.get(key, default))`.
fn int(payload: &Obj, key: &str, default: i64) -> Result<i64, String> {
    match payload.get(key) {
        None => Ok(default),
        Some(value) => to_int(value).ok_or_else(|| format!("invalid literal for int() with base 10: {value}")),
    }
}

/// `POST /vrgdg/music_builder/analyze_audio`: waveform peaks, beats and tempo of the song.
pub fn analyze_audio(env: &Env, payload: &Obj) -> Reply {
    let tools = env.tools()?;
    let mut audio = resolve_existing_file(&path_text(payload, "audio_path"), "Audio file")?;
    let project = path_text(payload, "project_folder");
    let is_m4a = audio.extension().is_some_and(|e| e.eq_ignore_ascii_case("m4a"));
    if is_m4a && !project.is_empty() {
        let target = absolute(Path::new(&project)).join("project_audio").join("project_audio.wav");
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let args: Vec<String> = ["-hide_banner", "-loglevel", "error", "-y", "-i", &text(&audio), "-vn", "-acodec", "pcm_s16le", "-ar", "44100", "-ac", "2", &text(&target)].iter().map(|s| s.to_string()).collect();
        tools.ffmpeg_checked(&args, "Could not convert audio to WAV").map_err(|e| format!("Could not convert audio to WAV: {e}"))?;
        audio = target;
    }
    let target_peaks = int(payload, "target_peaks", 1600)?.max(1) as usize;
    let info = read_audio_peaks(&tools, &audio, target_peaks).map_err(|e| format!("Could not read audio for waveform. Try a standard WAV, MP3, FLAC, or M4A file. {e}"))?;
    let beats = estimate_beats_from_peaks(&info.peaks, info.duration);
    let tempo = tempo_from_beat_times(&beats);
    Ok(json!({
        "audio_path": text(&audio),
        "duration": info.duration,
        "sample_rate": info.sample_rate,
        "channels": info.channels,
        "peaks": info.peaks,
        "beats": beats,
        "tempo_bpm": tempo,
    }))
}

fn project_and_scene(payload: &Obj) -> Result<(PathBuf, u32), String> {
    let project = path_text(payload, "project_folder");
    let scene = int(payload, "scene_number", 1)?;
    Ok((PathBuf::from(project), scene.max(0) as u32))
}

/// `POST /vrgdg/music_builder/latent_status`: answers without the `ok` wrapper of the other routes.
pub fn latent_status(payload: &Obj) -> Reply {
    let (project, scene) = project_and_scene(payload)?;
    let info = latents::latent_info(&project, scene).map_err(|e| e.to_string())?;
    serde_json::to_value(info).map_err(|e| e.to_string())
}

/// `POST /vrgdg/music_builder/list_scene_latent_takes`.
pub fn list_scene_latent_takes(payload: &Obj) -> Reply {
    let (project, scene) = project_and_scene(payload)?;
    Ok(json!({"scene_number": scene, "takes": takes::list_takes(&project, scene)}))
}

/// `POST /vrgdg/music_builder/check_latent_predecessor`: whether the scene before has the latent this scene continues
/// from. Answers with `ok` inside the body.
pub fn check_latent_predecessor(payload: &Obj) -> Reply {
    let (project, scene) = project_and_scene(payload)?;
    if scene <= 1 {
        return Ok(json!({
            "ok": true, "scene_number": scene, "predecessor_needed": false, "predecessor_exists": true, "predecessor_scene": 0,
            "predecessor_path": "", "frame_count": 0, "token_count": 0, "dirty": false,
        }));
    }
    let predecessor = scene - 1;
    // Each take keeps its own latent. Bring back the one of the take the predecessor has selected.
    let video = path_text(payload, "predecessor_video_path");
    let take = if video.is_empty() { json!({"status": "unchecked"}) } else { takes::activate_for_video(&project, predecessor, Some(Path::new(&video))).map_err(|e| e.to_string())? };
    let info = latents::latent_info(&project, predecessor).map_err(|e| e.to_string())?;
    Ok(json!({
        "take_status": take.get("status"),
        "take_count": take.get("take_count").cloned().unwrap_or(json!(0)),
        "ok": true,
        "scene_number": scene,
        "predecessor_scene": predecessor,
        "predecessor_needed": true,
        "predecessor_exists": info.exists,
        "predecessor_path": info.path,
        "frame_count": info.frame_count,
        "token_count": info.token_count,
        "tail_padding_known": info.tail_padding_frames.is_some(),
        "dirty": info.dirty,
    }))
}

/// `POST /vrgdg/music_builder/delete_scene_latent`.
pub fn delete_scene_latent(payload: &Obj) -> Reply {
    let project = PathBuf::from(path_text(payload, "project_folder"));
    if payload.get("all").is_some_and(h3_core::pyval::truthy) {
        let removed = latents::delete_all_latents(&project);
        return Ok(json!({"deleted": removed > 0, "removed_files": removed, "all": true}));
    }
    let scene = int(payload, "scene_number", 1)?.max(0) as u32;
    let reindex = payload.get("reindex").is_none_or(h3_core::pyval::truthy);
    let deleted = latents::delete_latent(&project, scene, reindex);
    Ok(json!({"deleted": deleted, "scene_number": scene}))
}

/// `POST /vrgdg/music_builder/list_dirty_latents`.
pub fn list_dirty_latents(payload: &Obj) -> Reply {
    Ok(json!({"dirty_scenes": latents::list_dirty(Path::new(&path_text(payload, "project_folder")))}))
}

fn renamed_json(renamed: assets::Renamed) -> Value {
    json!(renamed.into_iter().map(|(from, to)| json!([text(&from), text(&to)])).collect::<Vec<_>>())
}

fn project_folder(payload: &Obj) -> Result<PathBuf, String> {
    let folder = absolute(Path::new(&path_text(payload, "project_folder")));
    if !folder.is_dir() {
        return Err(format!("Project folder was not found: {}", folder.display()));
    }
    Ok(folder)
}

/// `POST /vrgdg/music_builder/renumber_scenes_after_removal`.
pub fn renumber_after_removal(payload: &Obj, now: &Stamp) -> Reply {
    let folder = project_folder(payload)?;
    let removed = int(payload, "removed_scene_number", 0)?;
    let renamed = assets::renumber_after_removal(&folder, removed.max(0) as u32, &now.compact).map_err(|e| e.to_string())?;
    Ok(json!({"renamed": renamed_json(renamed)}))
}

/// `POST /vrgdg/music_builder/renumber_scenes_after_insert`.
pub fn renumber_after_insert(payload: &Obj) -> Reply {
    let folder = project_folder(payload)?;
    let inserted = int(payload, "inserted_scene_number", 0)?;
    let renamed = assets::renumber_after_insert(&folder, inserted.max(0) as u32).map_err(|e| e.to_string())?;
    Ok(json!({"renamed": renamed_json(renamed)}))
}

/// `POST /vrgdg/music_builder/scan_scene_videos`: the rendered clips, thumbnails and backups a project has on disk.
pub fn scan_scene_videos(env: &Env, payload: &Obj) -> Reply {
    let folder = path_text(payload, "project_folder");
    if folder.is_empty() {
        return Err("Project folder is empty.".into());
    }
    let tools = env.tools()?;
    h3_pipeline::scratch_ops::scan_scene_videos(&tools, &absolute(Path::new(&folder))).map_err(|e| e.to_string())
}

/// `POST /vrgdg/music_builder/restore_scene_video`: put a video file in as a scene's clip.
pub fn restore_scene_video_route(env: &Env, payload: &Obj, now: &Stamp) -> Reply {
    let folder = path_text(payload, "project_folder");
    if folder.is_empty() {
        return Err("Project folder is empty.".into());
    }
    let tools = env.tools()?;
    let source = absolute(Path::new(&path_text(payload, "source_path")));
    let scene = int(payload, "scene_number", 1)?.max(1) as u32;
    let number = |key: &str| payload.get(key).and_then(h3_core::pyval::to_float).unwrap_or(0.0);
    let confirm = payload.get("confirm_duration_mismatch").is_some_and(h3_core::pyval::truthy);
    let stamp = now.compact.replace('_', "-");
    let restored = restore_scene_video(&tools, &absolute(Path::new(&folder)), &source, scene, number("expected_duration"), if number("duration_tolerance") > 0.0 { number("duration_tolerance") } else { 0.5 }, confirm, &stamp).map_err(|e| e.to_string())?;
    Ok(match restored {
        Restored::NeedsConfirmation { duration, expected_duration, duration_delta, tolerance } => json!({
            "needs_confirmation": true, "source_path": text(&source), "scene_number": scene, "duration": duration,
            "expected_duration": expected_duration, "duration_delta": duration_delta, "duration_tolerance": tolerance,
        }),
        Restored::Done { video_path, thumbnail_path, duration, backup_path, backup_thumbnail_path } => json!({
            "video_path": text(&video_path),
            "video_folder": video_path.parent().map(text).unwrap_or_default(),
            "thumbnail_path": thumbnail_path.map(|p| text(&p)).unwrap_or_default(),
            "scene_number": scene,
            "source_path": text(&source),
            "duration": duration,
            "backup_path": backup_path.map(|p| text(&p)).unwrap_or_default(),
            "backup_thumbnail_path": backup_thumbnail_path.map(|p| text(&p)).unwrap_or_default(),
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(value: Value) -> Obj {
        value.as_object().cloned().unwrap()
    }

    #[test]
    fn first_scene_has_no_predecessor() {
        let reply = check_latent_predecessor(&obj(json!({"project_folder": "x", "scene_number": 1}))).unwrap();
        assert_eq!(reply["predecessor_needed"], false);
        assert_eq!(reply["predecessor_exists"], true);
    }

    #[test]
    fn missing_predecessor_latent_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let reply = check_latent_predecessor(&obj(json!({"project_folder": text(dir.path()), "scene_number": 3}))).unwrap();
        assert_eq!(reply["predecessor_scene"], 2);
        assert_eq!(reply["predecessor_exists"], false);
        assert_eq!(reply["tail_padding_known"], false);
        let status = latent_status(&obj(json!({"project_folder": text(dir.path()), "scene_number": 2}))).unwrap();
        assert_eq!(status["exists"], false);
        assert_eq!(list_dirty_latents(&obj(json!({"project_folder": text(dir.path())}))).unwrap()["dirty_scenes"], json!([]));
    }

    #[test]
    fn renumbering_needs_an_existing_folder() {
        assert!(renumber_after_insert(&obj(json!({"project_folder": "Y:\\nowhere", "inserted_scene_number": 2}))).unwrap_err().starts_with("Project folder was not found"));
    }
}
