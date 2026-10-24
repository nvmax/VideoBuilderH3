//! Projects and sessions: list, open, save, create and delete. Ports of `_list_builder_projects`,
//! `_load_builder_session`, `_save_builder_session_unlocked`, `_new_builder_project` and `_delete_builder_project` in
//! `builder/project.py`, plus the asset copying they use in `builder/project_copy.py`.

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};

use h3_core::pyval::{text_or_empty, to_float, truthy, Obj};
use h3_project::builder_store::{load_global_llm_keys, load_global_llm_runner, save_global_llm_keys, save_global_llm_runner, save_model_defaults};
use h3_project::layout::{context_folder, images_folder, prompts_folder, scene_notes_path, session_path, srt_path, Stamp};
use h3_project::{atomic::atomic_write_text, derived, scaffold, SaveHooks, SaveInput};
use serde_json::{json, Value};

use crate::payload::{object, path_text};
use crate::Env;

type Reply = Result<Value, String>;

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

fn text(path: &Path) -> String {
    path.display().to_string()
}

fn normcase(path: &Path) -> String {
    let text = text(&absolute(path));
    if cfg!(windows) {
        text.replace('/', "\\").to_lowercase()
    } else {
        text
    }
}

fn same_folder(a: &Path, b: &Path) -> bool {
    normcase(a) == normcase(b)
}

fn is_inside(path: &Path, folder: &Path) -> bool {
    PathBuf::from(normcase(path)).starts_with(normcase(folder))
}

fn modified_seconds(path: &Path) -> f64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0.0, |d| d.as_secs_f64())
}

fn sorted_entries(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else { return Vec::new() };
    let mut names: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    names.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default());
    names
}

// -- list ---------------------------------------------------------------------------------------------------------------

/// `GET /vrgdg/music_builder/list_projects`: every project folder (one with a session file) in the output folder, the
/// custom root the caller gave and the app's own project folders, newest first.
pub fn list_projects(env: &Env, project_root: &str) -> Value {
    let output = absolute(&env.output);
    let mut roots = vec![output.clone()];
    let custom = project_root.trim().trim_matches('"');
    if !custom.is_empty() && Path::new(custom).is_absolute() {
        let custom = absolute(Path::new(custom));
        if !same_folder(&custom, &output) {
            roots.push(custom);
        }
    }
    for root in &env.roots {
        if !roots.iter().any(|r| same_folder(r, root)) {
            roots.push(absolute(root));
        }
    }
    let mut seen: Vec<String> = Vec::new();
    let mut projects: Vec<Value> = Vec::new();
    for root in &roots {
        for entry in sorted_entries(root) {
            let folder = absolute(&entry);
            let key = normcase(&folder);
            if seen.contains(&key) || !folder.is_dir() {
                continue;
            }
            let session_file = session_path(&folder);
            if !session_file.is_file() {
                continue;
            }
            seen.push(key);
            let scene_count = fs::read_to_string(&session_file)
                .ok()
                .and_then(|t| h3_core::pyjson::parse(&t).ok())
                .and_then(|s| s.get("segments").and_then(Value::as_array).map(Vec::len))
                .unwrap_or(0);
            let can_delete = env.roots.iter().chain(std::iter::once(&output)).any(|r| is_inside(&folder, r));
            projects.push(json!({
                "name": entry.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                "project_folder": text(&folder),
                "session_path": text(&absolute(&session_file)),
                "updated": modified_seconds(&session_file),
                "scene_count": scene_count,
                "can_delete": can_delete,
            }));
        }
    }
    projects.sort_by(|a, b| b["updated"].as_f64().partial_cmp(&a["updated"].as_f64()).unwrap_or(std::cmp::Ordering::Equal));
    json!({"projects": projects, "output_dir": text(&output), "project_roots": roots.iter().map(|r| text(r)).collect::<Vec<_>>()})
}

// -- open ---------------------------------------------------------------------------------------------------------------

/// `POST /vrgdg/music_builder/load_session`.
pub fn load_session(env: &Env, payload: &Obj) -> Reply {
    let folder = path_text(payload, "project_folder");
    if folder.is_empty() {
        return Err("Project folder is empty.".into());
    }
    let loaded = h3_project::load_builder_session(Path::new(&folder)).map_err(|e| e.to_string())?;
    let project = &loaded.project_folder;
    Ok(json!({
        "project_folder": text(project),
        "session_path": text(&loaded.session_path),
        "srt_path": text(&srt_path(project)),
        "scene_notes_path": text(&scene_notes_path(project)),
        "revision": loaded.revision,
        "session": Value::Object(loaded.session),
        "global_llm_keys": load_global_llm_keys(&env.output),
        "global_llm_runner": load_global_llm_runner(&env.output),
    }))
}

// -- save ---------------------------------------------------------------------------------------------------------------

/// `_copy_file_into_folder`: copy `source` into `target_folder` under a safe version of `target_name`. Returns the target,
/// or "" when the source is missing.
pub fn copy_file_into_folder(source: &str, target_folder: &Path, target_name: Option<&str>) -> std::io::Result<String> {
    if source.is_empty() {
        return Ok(String::new());
    }
    let source = absolute(Path::new(source.trim().trim_matches('"')));
    if !source.is_file() {
        return Ok(String::new());
    }
    fs::create_dir_all(target_folder)?;
    let name = target_name.map(str::to_string).unwrap_or_else(|| source.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
    let stem = Path::new(&name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let ext = Path::new(&name).extension().or_else(|| source.extension()).map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let target = target_folder.join(format!("{}{ext}", derived::safe_project_name(&stem)));
    if absolute(&source) != absolute(&target) {
        fs::copy(&source, &target)?;
    }
    Ok(text(&target))
}

/// `_copy_or_convert_project_audio`: copy the song into the project. An `.m4a` is converted to a WAV (the render path reads
/// WAV and MP3).
fn copy_or_convert_audio(env: &Env, source: &str, target_folder: &Path, target_name: &str) -> std::io::Result<String> {
    let is_m4a = Path::new(source).extension().is_some_and(|e| e.eq_ignore_ascii_case("m4a"));
    if !is_m4a {
        return copy_file_into_folder(source, target_folder, Some(target_name));
    }
    let tools = env.tools().map_err(std::io::Error::other)?;
    let stem = Path::new(target_name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let target = target_folder.join(format!("{}.wav", derived::safe_project_name(&stem)));
    fs::create_dir_all(target_folder)?;
    let args: Vec<String> = ["-hide_banner", "-loglevel", "error", "-y", "-i", source, "-vn", "-acodec", "pcm_s16le", "-ar", "44100", "-ac", "2"]
        .iter()
        .map(|s| s.to_string())
        .chain(std::iter::once(text(&target)))
        .collect();
    tools.ffmpeg_checked(&args, "Could not convert audio to WAV").map_err(|e| std::io::Error::other(format!("Could not convert audio to WAV: {e}")))?;
    if fs::metadata(&target).map_or(true, |m| m.len() == 0) {
        return Err(std::io::Error::other("Audio conversion did not create a WAV file."));
    }
    Ok(text(&target))
}

/// `_snapshot_project_assets`: keep a copy of the song and the context files inside the project.
fn snapshot_project_assets(env: &Env, project: &Path, session: &mut Obj, audio_path: &mut String) -> std::io::Result<()> {
    if !audio_path.is_empty() && Path::new(audio_path.as_str()).is_file() {
        let ext = Path::new(audio_path.as_str()).extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        let copied = copy_or_convert_audio(env, audio_path, &project.join("project_audio"), &format!("project_audio{ext}"))?;
        if !copied.is_empty() {
            *audio_path = copied;
        }
    }
    for (key, file) in [("prompt_json_path", "ConceptPrompts.txt"), ("theme_style_path", "themestyle.txt"), ("story_idea_path", "storyconcept.txt"), ("subject_scene_path", "subjectsandscenes.txt")] {
        let raw = text_or_empty(session.get(key)).trim().to_string();
        if !raw.is_empty() && Path::new(&raw).is_file() {
            let copied = copy_file_into_folder(&raw, &context_folder(project), Some(file))?;
            if !copied.is_empty() {
                session.insert(key.into(), json!(copied));
            }
        }
    }
    Ok(())
}

/// `_fallback_project_context_text`: portable context from the session when no context file exists.
fn fallback_context_text(filename: &str, session: &Obj) -> String {
    let obj = |key: &str| session.get(key).and_then(Value::as_object).cloned().unwrap_or_default();
    let field = |source: &Obj, key: &str| text_or_empty(source.get(key)).trim().to_string();
    let story_layer = obj("builder_story_layer");
    let storyboard = obj("builder_storyboard_defaults");
    let refs = obj("flux_reference_builder");
    let segments = session.get("segments").and_then(Value::as_array).cloned().unwrap_or_default();
    match filename {
        "storyconcept.txt" => {
            let mut lines: Vec<String> = [("Overall story idea", "overall_story_idea"), ("Story arc", "user_story_arc"), ("Song/story brief", "song_story_brief")]
                .iter()
                .filter_map(|(label, key)| Some(format!("{label}: {}", Some(field(&story_layer, key)).filter(|v| !v.is_empty())?)))
                .collect();
            if lines.is_empty() {
                for (i, segment) in segments.iter().enumerate() {
                    let Some(segment) = segment.as_object() else { continue };
                    let value = ["scene_summary", "timeline_note", "t2i_prompt"].iter().map(|k| field(segment, k)).find(|v| !v.is_empty()).unwrap_or_default();
                    if !value.is_empty() {
                        lines.push(format!("Scene {}: {value}", i + 1));
                    }
                }
            }
            if lines.is_empty() {
                "Use the saved scene prompts, lyrics, and timeline order as the canonical project story.".into()
            } else {
                lines.join("\n\n")
            }
        }
        "subjectsandscenes.txt" => {
            let mut lines: Vec<String> = Vec::new();
            for (kind, key) in [("Subject", "subjects"), ("Location", "locations")] {
                for (i, item) in refs.get(key).and_then(Value::as_array).cloned().unwrap_or_default().iter().enumerate() {
                    let Some(item) = item.as_object() else { continue };
                    let name = Some(field(item, "name")).filter(|n| !n.is_empty()).unwrap_or_else(|| format!("{kind} {}", i + 1));
                    let description = field(item, "description");
                    lines.push(format!("{kind}: {name}") + &if description.is_empty() { String::new() } else { format!("\n{description}") });
                }
            }
            for map_name in ["subject_scene_map", "performer_scene_map", "scene_map", "scene_trigger_map"] {
                if let Some(map) = refs.get(map_name).filter(|m| m.as_object().is_some_and(|o| !o.is_empty())) {
                    lines.push(format!("{map_name}:\n{}", h3_core::pyjson::dumps(map)));
                }
            }
            if lines.is_empty() {
                "No subjects or locations are currently mapped; use the saved scene records as canonical scene context.".into()
            } else {
                lines.join("\n\n")
            }
        }
        "themestyle.txt" => {
            let lines: Vec<String> = [
                ("Image world style", &story_layer, "image_world_style"),
                ("Custom style direction", &story_layer, "image_custom_style_direction"),
                ("Image aesthetic", &storyboard, "image_aesthetic"),
                ("Video style", &storyboard, "video_style"),
                ("Custom video style", &storyboard, "video_style_custom"),
                ("Performance style", &storyboard, "performance_style"),
                ("Global consistency", &storyboard, "global_consistency_phrase"),
            ]
            .iter()
            .filter_map(|(label, source, key)| Some(format!("{label}: {}", Some(field(source, key)).filter(|v| !v.is_empty())?)))
            .collect();
            if lines.is_empty() {
                "Preserve the visual style, wardrobe, lighting, materials, and continuity established by the saved scene prompts and reference images.".into()
            } else {
                lines.join("\n")
            }
        }
        _ => String::new(),
    }
}

/// `_save_project_context_files`: the three context files the render prompts read. A caller that could not read a file
/// never erases an existing brief.
fn save_project_context_files(project: &Path, session: &Obj) -> std::io::Result<Vec<PathBuf>> {
    let Some(files) = session.get("project_context_files").and_then(Value::as_object) else { return Ok(Vec::new()) };
    let mut saved = Vec::new();
    for filename in ["storyconcept.txt", "subjectsandscenes.txt", "themestyle.txt"] {
        let path = context_folder(project).join(filename);
        let mut value = text_or_empty(files.get(filename)).trim().to_string();
        if value.is_empty() && path.is_file() {
            value = String::from_utf8_lossy(&fs::read(&path)?).trim_start_matches('\u{feff}').trim().to_string();
        }
        if value.is_empty() {
            value = fallback_context_text(filename, session);
        }
        atomic_write_text(&path, &format!("{}\n", value.trim_end()))?;
        saved.push(path);
    }
    Ok(saved)
}

/// `_validate_saved_project`.
fn validate_saved_project(project: &Path, session: &Obj, context_paths: &[PathBuf]) -> Result<(), String> {
    let refs = session.get("flux_reference_builder").filter(|r| r.is_object());
    let manifest_path = project.join("subject_location").join("reference_descriptions.json");
    let mut required = vec![session_path(project), srt_path(project)];
    required.extend(context_paths.iter().cloned());
    if refs.is_some() {
        required.push(manifest_path.clone());
    }
    let missing: Vec<String> = required.iter().filter(|p| !p.is_file()).map(|p| text(p)).collect();
    if !missing.is_empty() {
        return Err(format!("Project save validation failed; missing files: {}", missing.join(", ")));
    }
    let empty: Vec<String> = context_paths.iter().filter(|p| fs::metadata(p).map_or(true, |m| m.len() == 0)).filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string())).collect();
    if !empty.is_empty() {
        return Err(format!("Project save validation failed; empty context files: {}", empty.join(", ")));
    }
    if let Some(refs) = refs {
        let manifest = fs::read_to_string(&manifest_path).ok().and_then(|t| h3_core::pyjson::parse(&t).ok());
        let valid = manifest.as_ref().is_some_and(|m| m.get("subjects").is_some_and(Value::is_array) && m.get("locations").is_some_and(Value::is_array));
        if !valid {
            return Err("Project save validation failed; reference-builder manifest is invalid.".into());
        }
        let mut expected = refs.clone();
        if let Some(map) = expected.as_object_mut() {
            map.entry("subjects").or_insert_with(|| json!([]));
            map.entry("locations").or_insert_with(|| json!([]));
        }
        if manifest != Some(expected) {
            return Err("Project save validation failed; reference-builder manifest does not match the saved session state.".into());
        }
    }
    Ok(())
}

fn int_of(value: Option<&Value>) -> i64 {
    value.filter(|v| truthy(v)).and_then(to_float).map_or(0, |f| f as i64)
}

/// `POST /vrgdg/music_builder/save_session`: Autosave, Quick Save and the Reference Builder save.
pub fn save_session(env: &Env, payload: &Obj, now: &Stamp) -> Reply {
    let audio_raw = path_text(payload, "audio_path");
    let audio_path = if audio_raw.is_empty() {
        String::new()
    } else {
        let path = absolute(Path::new(&audio_raw));
        if !path.is_file() {
            return Err(format!("Audio file was not found: {}", path.display()));
        }
        text(&path)
    };
    let mut project_folder = path_text(payload, "project_folder");
    if project_folder.is_empty() {
        let name = crate::payload::text(payload, "project_name");
        project_folder = if !audio_path.is_empty() {
            let audio = Path::new(&audio_path);
            let stem = audio.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let name = derived::safe_project_name(&if name.is_empty() { format!("{stem}_builder") } else { name });
            text(&audio.parent().unwrap_or(Path::new("")).join(name))
        } else {
            let name = if name.is_empty() { format!("VRGDG_Project_{}", now.compact) } else { name };
            text(&env.output.join(derived::safe_project_name(&name)))
        };
    }
    let folder = absolute(Path::new(&project_folder));
    fs::create_dir_all(&folder).map_err(|e| e.to_string())?;

    let mut session = object(payload.get("session"));
    // Global API keys and LLM runner settings go to their own files, never into the project's session.
    save_global_llm_keys(&env.output, session.remove("global_llm_keys").as_ref()).map_err(|e| e.to_string())?;
    save_global_llm_runner(&env.output, session.remove("global_llm_runner").as_ref()).map_err(|e| e.to_string())?;
    if let Some(files) = payload.get("project_context_files").filter(|f| f.is_object()) {
        session.insert("project_context_files".into(), files.clone());
    }
    let incoming = int_of(session.get("builder_save_revision"));

    let model_defaults_path: RefCell<Option<PathBuf>> = RefCell::new(None);
    let context_paths: RefCell<Vec<PathBuf>> = RefCell::new(Vec::new());
    let prepare = |project: &Path, session: &mut Obj, audio: &mut String| snapshot_project_assets(env, project, session, audio);
    let before_commit = |project: &Path, session: &Obj| -> std::io::Result<()> {
        *context_paths.borrow_mut() = save_project_context_files(project, session)?;
        *model_defaults_path.borrow_mut() = save_model_defaults(&env.output, session, now)?;
        Ok(())
    };
    let hooks = SaveHooks { prepare: Some(&prepare), before_commit: Some(&before_commit) };
    let outcome = h3_project::save_session_with(&folder, SaveInput { session, audio_path, now: now.clone() }, &hooks).map_err(|e| e.to_string())?;

    let project = &outcome.project_folder;
    if outcome.stale {
        let existing = int_of(outcome.session.get("builder_save_revision"));
        return Ok(json!({
            "stale": true,
            "stale_reason": format!("incoming revision {incoming} < saved revision {existing}"),
            "current_revision": outcome.revision,
            "revision": outcome.revision,
            "project_folder": text(project),
            "session_path": text(&outcome.session_path),
            "srt_path": text(&srt_path(project)),
            "session": Value::Object(outcome.session),
        }));
    }
    let context_paths = context_paths.into_inner();
    validate_saved_project(project, &outcome.session, &context_paths)?;
    let lyric_source = outcome.session.get("lyric_mapper").and_then(|m| m.get("source_text"));
    let full_lyrics = if text_or_empty(lyric_source).trim().is_empty() { String::new() } else { text(&context_folder(project).join("full_lyrics.txt")) };
    let references = if outcome.session.get("flux_reference_builder").is_some_and(Value::is_object) { text(&project.join("subject_location").join("reference_descriptions.json")) } else { String::new() };
    let engine_is_minimax = text_or_empty(outcome.session.get("video_engine")).trim().to_lowercase() == "minimax_h3";
    Ok(json!({
        "stale": false,
        "revision": outcome.revision,
        "project_folder": text(project),
        "session_path": text(&outcome.session_path),
        "srt_path": text(&srt_path(project)),
        "images_folder": text(&images_folder(project)),
        "prompts_folder": text(&prompts_folder(project)),
        "context_folder": text(&context_folder(project)),
        "context_paths": context_paths.iter().map(|p| text(p)).collect::<Vec<_>>(),
        "full_lyrics_path": full_lyrics,
        "model_defaults_path": model_defaults_path.into_inner().map(|p| text(&p)).unwrap_or_default(),
        "scene_notes_path": text(&scene_notes_path(project)),
        "reference_descriptions_path": references,
        "minimax_project_index_path": if engine_is_minimax { text(&project.join("MINIMAX_PROJECT_FILES.md")) } else { String::new() },
        "session": Value::Object(outcome.session),
    }))
}

// -- create and delete ----------------------------------------------------------------------------------------------------

/// `_project_target_from_payload`: the folder a new project goes in.
pub fn project_target(env: &Env, payload: &Obj, preferred_key: &str, now: &Stamp) -> Result<PathBuf, String> {
    let mut raw = path_text(payload, preferred_key);
    if raw.is_empty() {
        raw = path_text(payload, "project_name");
    }
    if raw.is_empty() {
        raw = format!("VRGDG_Project_{}", now.compact);
    }
    if Path::new(&raw).is_absolute() || Path::new(&raw).parent().is_some_and(|p| !p.as_os_str().is_empty()) {
        return Ok(absolute(Path::new(&raw)));
    }
    let root = path_text(payload, "project_root");
    if !root.is_empty() {
        if !Path::new(&root).is_absolute() {
            return Err("Custom project root must be a full absolute folder path.".into());
        }
        return Ok(absolute(Path::new(&root)).join(derived::safe_project_name(&raw)));
    }
    Ok(env.output.join(derived::safe_project_name(&raw)))
}

/// `POST /vrgdg/music_builder/new_project`.
pub fn new_project(env: &Env, payload: &Obj, now: &Stamp) -> Reply {
    let target = project_target(env, payload, "project_folder", now)?;
    let target = scaffold::new_project(&target).map_err(|e| e.to_string())?;
    let context = context_folder(&target);
    Ok(json!({
        "project_folder": text(&target),
        "session_path": text(&session_path(&target)),
        "srt_path": text(&srt_path(&target)),
        "images_folder": text(&images_folder(&target)),
        "prompts_folder": text(&prompts_folder(&target)),
        "context_folder": text(&context),
        "concept_prompts_path": text(&context.join("ConceptPrompts.txt")),
        "i2v_motion_notes_path": text(&context.join("I2VMotionNotes.txt")),
        "theme_style_path": text(&context.join("themestyle.txt")),
        "story_idea_path": text(&context.join("storyconcept.txt")),
        "subject_scene_path": text(&context.join("subjectsandscenes.txt")),
    }))
}

/// `POST /vrgdg/music_builder/delete_project`. Only projects in the output folder or the app's project folders can go.
pub fn delete_project(env: &Env, payload: &Obj) -> Reply {
    let folder = path_text(payload, "project_folder");
    if folder.is_empty() {
        return Err("Project folder is empty.".into());
    }
    let folder = absolute(Path::new(&folder));
    let allowed = env.roots.iter().chain(std::iter::once(&env.output)).any(|r| is_inside(&folder, r) && !same_folder(&folder, r));
    if !allowed {
        return Err("Project is outside the project folders, so it was not deleted.".into());
    }
    if !folder.is_dir() {
        return Ok(json!({"deleted": false, "project_folder": text(&folder), "reason": "Project folder was already missing."}));
    }
    if !session_path(&folder).is_file() {
        return Err("This folder does not look like a Music Video Builder project.".into());
    }
    fs::remove_dir_all(&folder).map_err(|e| e.to_string())?;
    Ok(json!({"deleted": true, "project_folder": text(&folder)}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(dir: &Path) -> Env {
        Env { output: dir.join("output"), roots: vec![dir.join("projects")], ffmpeg: "ffmpeg".into(), ffprobe: "ffprobe".into() }
    }

    fn obj(value: Value) -> Obj {
        value.as_object().cloned().unwrap()
    }

    fn stamp() -> Stamp {
        Stamp { compact: "20260101_000000".into(), iso: "2026-01-01T00:00:00".into(), epoch: 1_767_225_600.0 }
    }

    #[test]
    fn new_save_list_open_delete_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let env = env(dir.path());
        fs::create_dir_all(&env.output).unwrap();
        let created = new_project(&env, &obj(json!({"project_name": "My Song"})), &stamp()).unwrap();
        let folder = created["project_folder"].as_str().unwrap().to_string();
        assert!(folder.ends_with("My Song"));

        let song = dir.path().join("song.wav");
        fs::write(&song, b"RIFF").unwrap();
        let saved = save_session(
            &env,
            &obj(json!({
                "project_folder": folder,
                "audio_path": text(&song),
                "session": {"segments": [{"start": 0, "end": 4, "label": "Scene 1"}], "video_engine": "minimax_h3", "image_model_mode": "zimage"},
                "project_context_files": {"storyconcept.txt": "A story", "themestyle.txt": "", "subjectsandscenes.txt": ""},
            })),
            &stamp(),
        )
        .unwrap();
        assert_eq!(saved["stale"], false);
        assert_eq!(saved["revision"], 1);
        assert!(saved["session"]["audio_path"].as_str().unwrap().ends_with("project_audio.wav"));
        assert_eq!(saved["context_paths"].as_array().unwrap().len(), 3);
        assert!(saved["minimax_project_index_path"].as_str().unwrap().ends_with("MINIMAX_PROJECT_FILES.md"));
        assert!(saved["model_defaults_path"].as_str().unwrap().ends_with("model_defaults.json"));
        let story = fs::read_to_string(context_folder(Path::new(&folder)).join("storyconcept.txt")).unwrap();
        assert_eq!(story, "A story\n");
        assert!(fs::read_to_string(context_folder(Path::new(&folder)).join("themestyle.txt")).unwrap().starts_with("Preserve the visual style"));

        let listed = list_projects(&env, "");
        assert_eq!(listed["projects"].as_array().unwrap().len(), 1);
        assert_eq!(listed["projects"][0]["scene_count"], 1);
        let opened = load_session(&env, &obj(json!({"project_folder": folder}))).unwrap();
        assert_eq!(opened["revision"], 1);
        assert_eq!(opened["session"]["segments"][0]["label"], "Scene 1");

        // An older snapshot is ignored and reported.
        let again = save_session(&env, &obj(json!({"project_folder": folder, "session": {"builder_save_revision": 5, "segments": []}})), &stamp()).unwrap();
        assert_eq!(again["stale"], false);
        let stale = save_session(&env, &obj(json!({"project_folder": folder, "session": {"builder_save_revision": 1, "segments": []}})), &stamp()).unwrap();
        assert_eq!(stale["stale"], true);
        assert_eq!(stale["stale_reason"], "incoming revision 1 < saved revision 5");
        assert_eq!(stale["current_revision"], 2);

        delete_project(&env, &obj(json!({"project_folder": folder}))).unwrap();
        assert_eq!(list_projects(&env, "")["projects"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn delete_refuses_a_folder_outside_the_roots() {
        let dir = tempfile::tempdir().unwrap();
        let env = env(dir.path());
        let elsewhere = dir.path().join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        fs::write(elsewhere.join("vrgdg_builder_session.json"), "{}").unwrap();
        let error = delete_project(&env, &obj(json!({"project_folder": text(&elsewhere)}))).unwrap_err();
        assert!(error.contains("outside"));
        assert!(elsewhere.is_dir());
    }

    #[test]
    fn missing_audio_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let env = env(dir.path());
        let error = save_session(&env, &obj(json!({"audio_path": "Y:\\nowhere\\song.wav"})), &stamp()).unwrap_err();
        assert!(error.starts_with("Audio file was not found"));
    }
}
