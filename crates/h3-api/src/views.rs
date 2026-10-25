//! Project and scene views. Port of the read side of `agent_api/projects.py`.

use std::path::{Path, PathBuf};

use h3_core::pyval::{text_or_empty, to_float, truthy, Obj};
use h3_core::settings::normalize_settings;
use h3_project::layout::{scene_notes_path, session_path};
use h3_project::LoadedSession;
use serde_json::{json, Map, Value};

use crate::error::ApiError;

/// Folders that may hold projects: the project folder, then ComfyUI's output folder (where the Video Builder keeps its
/// projects, so they show up here too), then `H3_PROJECT_ROOTS`. A project is found in the first folder that has it.
pub fn project_roots(project_root: &Path, comfy_output: Option<&Path>) -> Vec<PathBuf> {
    let mut roots = vec![project_root.to_path_buf()];
    if let Some(output) = comfy_output.filter(|o| !o.as_os_str().is_empty() && o.is_dir() && !roots.contains(&o.to_path_buf())) {
        roots.push(output.to_path_buf());
    }
    if let Ok(extra) = std::env::var("H3_PROJECT_ROOTS") {
        for item in std::env::split_paths(&extra) {
            if !item.as_os_str().is_empty() && !roots.contains(&item) {
                roots.push(item);
            }
        }
    }
    roots
}

fn invalid_id(id: &str) -> bool {
    id.trim().is_empty() || id.contains("..") || id.chars().any(|c| matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || (c as u32) < 32)
}

/// The folder of project `id` in the first root that has it.
pub fn resolve_project(roots: &[PathBuf], id: &str) -> Result<PathBuf, ApiError> {
    if invalid_id(id) {
        return Err(ApiError::validation(format!("Invalid characters or traversal sequence in Project ID '{id}'.")));
    }
    for root in roots {
        let candidate = root.join(id);
        if session_path(&candidate).is_file() {
            return Ok(candidate);
        }
    }
    Err(ApiError::project_not_found(id))
}

pub fn project_id(folder: &Path) -> String {
    folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn modified_seconds(path: &Path) -> f64 {
    path.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0.0, |d| d.as_secs_f64())
}

fn revision_of(session: &Obj) -> i64 {
    ["revision", "builder_save_revision"].iter().filter_map(|k| session.get(*k)).filter(|v| truthy(v)).find_map(to_float).map_or(0, |f| f as i64)
}

fn is_file(text: &str) -> bool {
    !text.trim().is_empty() && Path::new(text.trim()).is_file()
}

/// Projects in the allowed roots, newest first.
pub fn list_projects(roots: &[PathBuf]) -> Vec<Value> {
    let mut found: Vec<(f64, Value)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else { continue };
        for entry in entries.flatten() {
            let folder = entry.path();
            let file = session_path(&folder);
            let id = project_id(&folder);
            if !folder.is_dir() || !file.is_file() || seen.contains(&id) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&file) else { continue };
            let Ok(Value::Object(data)) = h3_core::pyjson::parse(&text) else { continue };
            let scenes = data.get("segments").and_then(Value::as_array).map_or(0, Vec::len);
            let updated = modified_seconds(&file);
            seen.insert(id.clone());
            found.push((
                updated,
                json!({
                    "id": id,
                    "name": non_empty(data.get("project_name")).unwrap_or(id.clone()),
                    "updated": updated,
                    "revision": revision_of(&data),
                    "scene_count": scenes,
                    "has_audio": is_file(&text_or_empty(data.get("audio_path"))),
                    "video_engine": non_empty(data.get("video_engine")).unwrap_or_else(|| "minimax_h3".into()),
                    "image_mode": non_empty(data.get("image_model_mode")).unwrap_or_else(|| "zimage".into()),
                }),
            ));
        }
    }
    found.sort_by(|a, b| b.0.total_cmp(&a.0));
    found.into_iter().map(|(_, v)| v).collect()
}

fn non_empty(value: Option<&Value>) -> Option<String> {
    let text = text_or_empty(value);
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn seg_text(segment: &Obj, key: &str) -> String {
    text_or_empty(segment.get(key)).trim().to_string()
}

fn seg_number(segment: &Obj, key: &str) -> f64 {
    segment.get(key).filter(|v| truthy(v)).and_then(to_float).unwrap_or(0.0)
}

/// An asset descriptor for a file, or `null` when it is not there.
fn asset(project: &Path, path: &str, kind: &str, id: &str) -> Value {
    let text = path.trim().trim_matches('"');
    if text.is_empty() || !Path::new(text).is_file() {
        return Value::Null;
    }
    let absolute = std::path::absolute(text).unwrap_or_else(|_| PathBuf::from(text));
    let meta = absolute.metadata().ok();
    let mtime = meta.as_ref().map_or(0.0, |_| modified_seconds(&absolute)) as i64;
    let relative = absolute.strip_prefix(project).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_else(|_| absolute.to_string_lossy().replace('\\', "/"));
    json!({
        "id": id,
        "kind": kind,
        "filename": absolute.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        "path_rel": relative,
        "path": absolute.to_string_lossy(),
        "bytes": meta.map_or(0, |m| m.len()),
        "mtime": mtime,
        "exists": true,
    })
}

fn first_asset(project: &Path, paths: &[String], kind: &str, id: &str) -> Value {
    paths.iter().map(|p| asset(project, p, kind, id)).find(|a| !a.is_null()).unwrap_or(Value::Null)
}

fn image_paths(segment: &Obj) -> Vec<String> {
    let history = segment.get("image_history").and_then(Value::as_array).cloned().unwrap_or_default();
    let selected = segment
        .get("image_history_index")
        .and_then(to_float)
        .and_then(|i| history.get(i as usize).cloned())
        .map(|v| text_or_empty(Some(&v)))
        .unwrap_or_default();
    vec![seg_text(segment, "approved_image_path"), selected, seg_text(segment, "custom_image_path"), seg_text(segment, "ref_image_path")]
}

/// One scene as the API shows it.
pub fn scene_view(project: &Path, segment: &Obj, index: usize, locked_settings: bool) -> Value {
    let number = index + 1;
    let id = non_empty(segment.get("id")).unwrap_or_else(|| format!("seg_{number:04}"));
    let start = seg_number(segment, "start");
    let end = if segment.get("end").is_some_and(truthy) { seg_number(segment, "end") } else { start };
    let t2i = seg_text(segment, "t2i_prompt");
    let i2v = seg_text(segment, "i2v_prompt");
    let prompt = seg_text(segment, "minimax_h3_prompt");
    let lyric = seg_text(segment, "lyric_text");
    let notes = seg_text(segment, "notes");
    let notes_text = notes.clone();
    let image = first_asset(project, &image_paths(segment), "image", &format!("img_{number:04}"));
    let video = first_asset(project, &[seg_text(segment, "video_path"), seg_text(segment, "rendered_video_path")], "video", &format!("vid_{number:04}"));
    let thumbnail = first_asset(project, &[seg_text(segment, "video_thumbnail_path"), seg_text(segment, "thumbnail_path")], "image", &format!("thumb_{number:04}"));
    let status = if !video.is_null() {
        "has_video"
    } else if !image.is_null() {
        "has_image"
    } else if !(t2i.is_empty() && i2v.is_empty() && prompt.is_empty()) {
        "has_prompt"
    } else {
        "empty"
    };
    let round3 = |v: f64| (v * 1000.0).round() / 1000.0;
    let mut view = json!({
        "id": id,
        "number": number,
        "start": round3(start),
        "end": round3(end),
        "duration": round3((end - start).max(0.0)),
        "status": status,
        "label": seg_text(segment, "label"),
        "notes": notes_text,
        "timeline_note": seg_text(segment, "timeline_note"),
        "lyrics": if lyric.is_empty() { notes } else { lyric },
        "story_beat": seg_text(segment, "story_beat"),
        "t2i_prompt": t2i,
        "i2v_prompt": i2v,
        "minimax_h3_prompt": prompt,
        "minimax_h3_continuation_direction": seg_text(segment, "minimax_h3_continuation_direction"),
        "minimax_h3_continuation_start_seconds": segment.get("minimax_h3_continuation_start_seconds").cloned().unwrap_or(Value::Null),
        "no_character_present": segment.get("no_character_present").is_some_and(truthy),
        "lyric_no_lip_sync": segment.get("lyric_no_lip_sync").is_some_and(truthy),
        "lyric_singers": segment.get("lyric_singers").cloned().unwrap_or_else(|| json!([])),
        "approved_image": image,
        "rendered_video": video,
        "video_thumbnail": thumbnail,
    });
    if locked_settings {
        let map = view.as_object_mut().expect("object");
        // The scene's own Scene Defaults (the settings it keeps apart from the project's), as PATCH accepts them.
        map.insert("scene_defaults".into(), Value::Object(h3_core::scene_defaults::normalize_scene_defaults(segment.get("scene_defaults").unwrap_or(&Value::Null), false).unwrap_or_default()));
        map.insert("audio_mask".into(), segment.get("audio_mask").filter(|v| v.is_object()).cloned().unwrap_or(Value::Null));
        map.insert("use_scene_minimax_h3_settings".into(), json!(segment.get("use_scene_minimax_h3_settings").is_some_and(truthy)));
        map.insert("minimax_h3_settings".into(), segment.get("minimax_h3_settings").filter(|v| v.is_object()).cloned().unwrap_or(Value::Null));
        map.insert("minimax_h3_continuity_mode_used".into(), json!(h3_core::continuity::normalize_continuity_mode(segment.get("minimax_h3_continuity_mode_used"))));
        map.insert("minimax_h3_continuity_source_scene_id".into(), json!(seg_text(segment, "minimax_h3_continuity_source_scene_id")));
    }
    view
}

pub fn segments(session: &Obj) -> Vec<Obj> {
    session.get("segments").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_object().cloned()).collect()).unwrap_or_default()
}

/// Filters of the scene list.
#[derive(Debug, Default)]
pub struct SceneFilter {
    pub has_image: Option<bool>,
    pub has_video: Option<bool>,
    pub has_prompt: Option<bool>,
    pub status: Option<String>,
}

pub fn scenes_view(loaded: &LoadedSession, filter: &SceneFilter, locked_settings: bool) -> Vec<Value> {
    segments(&loaded.session)
        .iter()
        .enumerate()
        .map(|(i, segment)| scene_view(&loaded.project_folder, segment, i, locked_settings))
        .filter(|scene| {
            let has = |key: &str| !scene.get(key).is_none_or(Value::is_null);
            let has_prompt = ["t2i_prompt", "i2v_prompt", "minimax_h3_prompt"].iter().any(|k| !scene[*k].as_str().unwrap_or("").is_empty());
            filter.has_image.is_none_or(|w| has("approved_image") == w)
                && filter.has_video.is_none_or(|w| has("rendered_video") == w)
                && filter.has_prompt.is_none_or(|w| has_prompt == w)
                && filter.status.as_deref().is_none_or(|s| scene["status"] == s)
        })
        .collect()
}

/// The scene with this id or 1-based number.
pub fn find_scene(loaded: &LoadedSession, scene_ref: &str) -> Result<usize, ApiError> {
    let target = scene_ref.trim();
    let segs = segments(&loaded.session);
    for (i, segment) in segs.iter().enumerate() {
        let id = non_empty(segment.get("id")).unwrap_or_else(|| format!("seg_{:04}", i + 1));
        if id == target || (i + 1).to_string() == target {
            return Ok(i);
        }
    }
    let mut missing = ApiError::scene_not_found(target, &project_id(&loaded.project_folder));
    let count = segs.len();
    let note = match count {
        0 => "The project has no scenes.".to_string(),
        1 => "The project has 1 scene: name it by its id or by its number, 1.".to_string(),
        _ => format!("The project has {count} scenes: name one by its id or by its number, 1 to {count}."),
    };
    missing.message = format!("{} {note}", missing.message);
    Err(missing)
}

/// Settings groups the app serves: the project switches and the MiniMax H3 settings.
pub fn settings_view(session: &Obj) -> Value {
    let flag = |key: &str, default: bool| session.get(key).map_or(default, truthy);
    json!({
        "project": {
            "video_engine": non_empty(session.get("video_engine")).unwrap_or_else(|| "minimax_h3".into()),
            "video_type": non_empty(session.get("video_type")).unwrap_or_else(|| "singing".into()),
            "timing_frozen": flag("timing_frozen", false),
            "auto_save_enabled": flag("auto_save_enabled", true),
            "omit_lyrics_from_video_prompts": flag("omit_lyrics_from_video_prompts", false),
            "automatic_memory_cleanup": flag("automatic_memory_cleanup", true),
            "scene_render_wait_hours": session.get("scene_render_wait_hours").and_then(to_float).unwrap_or(2.0),
        },
        "minimax_h3": Value::Object(normalize_settings(session.get("minimax_h3_settings"))),
    })
}

pub fn project_detail(loaded: &LoadedSession) -> Value {
    let session = &loaded.session;
    let id = project_id(&loaded.project_folder);
    let audio = text_or_empty(session.get("audio_path"));
    let audio = audio.trim();
    json!({
        "id": id,
        "name": non_empty(session.get("project_name")).unwrap_or(id),
        "project_folder": loaded.project_folder.to_string_lossy(),
        "revision": loaded.revision,
        "updated": session.get("updated").cloned().unwrap_or(json!(0)),
        "video_engine": non_empty(session.get("video_engine")).unwrap_or_else(|| "minimax_h3".into()),
        "image_mode": non_empty(session.get("image_model_mode")).unwrap_or_else(|| "zimage".into()),
        "settings": settings_view(session),
        "scenes": scenes_view(loaded, &SceneFilter::default(), false),
        "audio": {
            "attached": is_file(audio),
            "path": if is_file(audio) { audio } else { "" },
            "duration": session.get("audio_duration").and_then(to_float).unwrap_or(0.0),
        },
        "story": session.get("builder_story_layer").cloned().unwrap_or_else(|| json!({})),
        "references": session.get("flux_reference_builder").cloned().unwrap_or_else(|| json!({})),
    })
}

pub fn project_summary(loaded: &LoadedSession) -> Value {
    let scenes = scenes_view(loaded, &SceneFilter::default(), false);
    let count = |key: &str| scenes.iter().filter(|s| !s[key].is_null()).count();
    let with_prompts = scenes.iter().filter(|s| ["t2i_prompt", "i2v_prompt", "minimax_h3_prompt"].iter().any(|k| !s[*k].as_str().unwrap_or("").is_empty())).count();
    let total: f64 = scenes.iter().map(|s| s["duration"].as_f64().unwrap_or(0.0)).sum();
    let id = project_id(&loaded.project_folder);
    json!({
        "id": id,
        "name": non_empty(loaded.session.get("project_name")).unwrap_or(id),
        "revision": loaded.revision,
        "scene_count": scenes.len(),
        "scenes_with_images": count("approved_image"),
        "scenes_with_prompts": with_prompts,
        "scenes_with_videos": count("rendered_video"),
        "total_duration": (total * 1000.0).round() / 1000.0,
        "has_audio": is_file(&text_or_empty(loaded.session.get("audio_path"))),
        "has_scene_notes": scene_notes_path(&loaded.project_folder).is_file(),
    })
}

/// Merge a settings patch into the session the way the pack does: the nested group is merged over what is saved.
pub fn apply_settings_patch(session: &mut Obj, patch: &Obj) -> Result<(), ApiError> {
    for (group, value) in patch {
        let Value::Object(fields) = value else {
            return Err(ApiError::validation(format!("Settings group '{group}' must be an object.")));
        };
        match group.as_str() {
            "minimax_h3" => {
                let problems = h3_core::settings::validate_patch(fields);
                if !problems.is_empty() {
                    return Err(ApiError::new(axum::http::StatusCode::BAD_REQUEST, "SETTINGS_INVALID", "Some MiniMax H3 settings are not valid.").details(json!({"invalid_fields": problems})));
                }
                let mut current: Map<String, Value> = session.get("minimax_h3_settings").and_then(Value::as_object).cloned().unwrap_or_default();
                for (k, v) in fields {
                    current.insert(k.clone(), v.clone());
                }
                session.insert("minimax_h3_settings".into(), Value::Object(current));
            }
            "project" => {
                const ALLOWED: [&str; 7] =
                    ["video_engine", "video_type", "timing_frozen", "auto_save_enabled", "omit_lyrics_from_video_prompts", "automatic_memory_cleanup", "scene_render_wait_hours"];
                let unknown: Vec<&String> = fields.keys().filter(|k| !ALLOWED.contains(&k.as_str())).collect();
                if !unknown.is_empty() {
                    return Err(ApiError::new(axum::http::StatusCode::BAD_REQUEST, "SETTINGS_INVALID", "Unknown project settings.").details(json!({"invalid_fields": unknown.iter().map(|k| (k.to_string(), json!("unknown project setting"))).collect::<Map<_, _>>()})));
                }
                if let Some(kind) = fields.get("video_type") {
                    let known = ["singing", "speaking", "no_lip_sync", "story"];
                    if !kind.as_str().is_some_and(|k| known.contains(&k)) {
                        return Err(ApiError::new(axum::http::StatusCode::BAD_REQUEST, "SETTINGS_INVALID", "video_type must be singing, speaking, no_lip_sync or story.").details(json!({"invalid_fields": {"video_type": "must be singing, speaking, no_lip_sync or story"}})));
                    }
                }
                for (k, v) in fields {
                    session.insert(k.clone(), v.clone());
                }
            }
            other => {
                return Err(ApiError::validation(format!("Unknown settings group '{other}'. This app serves 'minimax_h3' and 'project'.")));
            }
        }
    }
    Ok(())
}

/// Every media file in the project's media folders. Port of `get_project_assets`.
pub fn project_assets(project: &Path) -> Vec<Value> {
    let mut assets = Vec::new();
    for (sub, kind) in [("images", "image"), ("scene_videos", "video"), ("final_videos", "video"), ("scene_audio", "audio"), ("project_audio", "audio"), ("latents", "latent")] {
        let Ok(read) = std::fs::read_dir(project.join(sub)) else { continue };
        let mut names: Vec<String> = read.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        for name in names {
            let path = project.join(sub).join(&name);
            let stem = Path::new(&name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let item = asset(project, &path.to_string_lossy(), kind, &stem);
            if !item.is_null() {
                assets.push(item);
            }
        }
    }
    assets
}
