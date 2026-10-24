//! The Storyboard's file, `storyboard/storyboard.json` in the project: load, save with a revision count, export of its prompts and import of
//! a reference picture. Port of `storyboard/persistence.py`. The cleaning of the data is in `h3_core::story`.

use std::fs;
use std::path::{Path, PathBuf};

use h3_core::pyval::{text_or_empty, to_float, truthy, Obj};
use h3_core::story::helpers::{
    clean_of, decode_image_data_url, first_truthy, normalize_performance_mode, normalize_reference_catalog, normalize_reference_item, normalize_story_arc_detail, normalize_story_layer, safe_file_stem, speed_value,
};
use h3_core::story::mode::{text, text_s};
use h3_core::story::scene::{normalize_script_import, normalize_short_film_planning_mode, normalize_storyboard_scene};
use serde_json::{json, Value};

use crate::atomic::{atomic_write_json, atomic_write_text};
use crate::layout::{prompts_folder as project_prompts_folder, Stamp};
use crate::rehydrate::absolute;

/// Why a storyboard call failed.
#[derive(Debug, Clone, PartialEq)]
pub enum StoryboardError {
    /// The saved storyboard changed after the caller loaded it. `current_revision` is what is saved now.
    Conflict { current_revision: i64, message: String },
    Failed(String),
}

impl std::fmt::Display for StoryboardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conflict { message, .. } | Self::Failed(message) => f.write_str(message),
        }
    }
}

impl From<String> for StoryboardError {
    fn from(message: String) -> Self {
        Self::Failed(message)
    }
}

fn io_message(path: &Path, error: std::io::Error) -> String {
    format!("{}: {error}", path.display())
}

/// `_safe_project_folder`: the project folder as a full path. It is created when it is not there.
pub fn safe_project_folder(raw: &str) -> Result<PathBuf, String> {
    let trimmed = raw.trim().trim_matches('"');
    if trimmed.is_empty() {
        return Err("Project folder is missing.".into());
    }
    let folder = absolute(Path::new(trimmed));
    fs::create_dir_all(&folder).map_err(|e| io_message(&folder, e))?;
    Ok(folder)
}

pub fn storyboard_folder(project: &Path) -> Result<PathBuf, String> {
    let folder = project.join("storyboard");
    fs::create_dir_all(&folder).map_err(|e| io_message(&folder, e))?;
    Ok(folder)
}

pub fn storyboard_path(project: &Path) -> Result<PathBuf, String> {
    Ok(storyboard_folder(project)?.join("storyboard.json"))
}

fn prompts_folder(project: &Path) -> Result<PathBuf, String> {
    let folder = project_prompts_folder(project);
    fs::create_dir_all(&folder).map_err(|e| io_message(&folder, e))?;
    Ok(folder)
}

fn payload_folder(payload: &Obj) -> Result<PathBuf, String> {
    safe_project_folder(&text_or_empty(payload.get("project_folder")))
}

/// `_import_storyboard_reference_image`: a picture from the page saved under `storyboard/references`, as a reference card.
pub fn import_reference_image(payload: &Obj, now: &Stamp) -> Result<Value, String> {
    let project = payload_folder(payload)?;
    let kind = text_or_empty(payload.get("kind").filter(|v| truthy(v)).or(Some(&json!("subject")))).trim().to_lowercase();
    let kind = if kind == "location" { "location" } else { "subject" };
    let default_name = if kind == "location" { "Location" } else { "Subject" };
    let name = match first_truthy(payload, &["name"]) {
        Some(v) => text(Some(v), 240),
        None => default_name.to_string(),
    };
    let description = text(payload.get("description"), 4000);
    let source = first_truthy(payload, &["image_data", "data"]).map(|v| text_or_empty(Some(v))).unwrap_or_default();
    let (raw, ext) = decode_image_data_url(&source)?;
    let reference_dir = storyboard_folder(&project)?.join("references").join(if kind == "location" { "locations" } else { "subjects" });
    fs::create_dir_all(&reference_dir).map_err(|e| io_message(&reference_dir, e))?;
    let stem = safe_file_stem(&name, kind);
    let mut path = reference_dir.join(format!("{stem}.{ext}"));
    let mut suffix = 2;
    while path.exists() {
        path = reference_dir.join(format!("{stem}_{suffix}.{ext}"));
        suffix += 1;
    }
    fs::write(&path, raw).map_err(|e| io_message(&path, e))?;
    let id = match first_truthy(payload, &["id"]) {
        Some(v) => text(Some(v), 160),
        None => text_s(&format!("{kind}_{stem}_{}", now.compact.replace('_', "")), 160),
    };
    let file_name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let reference = normalize_reference_item(Some(&json!({"id": id, "name": name, "description": description, "image": {"path": path.display().to_string(), "name": file_name, "data": ""}})), &name, &id);
    Ok(json!({"reference": reference, "path": path.display().to_string()}))
}

fn flag_default_true(payload: &Obj, key: &str, camel: &str) -> bool {
    let value = if payload.contains_key(key) { payload.get(key) } else { payload.get(camel) };
    !matches!(value, Some(Value::Bool(false)))
}

fn speed_of(payload: &Obj, key: &str, camel: &str, default_when_missing: i64) -> i64 {
    let fallback = Value::from(default_when_missing);
    let value = if payload.contains_key(key) { payload.get(key) } else { payload.get(camel).or(Some(&fallback)) };
    speed_value(value, 4)
}

fn engine_of(source: &Obj) -> &'static str {
    let engine = first_truthy(source, &["project_video_engine", "projectVideoEngine"]).map(|v| text_or_empty(Some(v))).unwrap_or_default().trim().to_lowercase();
    if engine == "minimax_h3" { "minimax_h3" } else { "ltx" }
}

fn text_or(source: &Obj, keys: &[&str], default: &str, limit: usize) -> String {
    match first_truthy(source, keys) {
        Some(v) => text(Some(v), limit),
        None => text_s(default, limit),
    }
}

fn story_layer_of(source: &Obj) -> Value {
    let given = first_truthy(source, &["story_layer", "storyLayer"]).cloned().unwrap_or_else(|| json!({}));
    normalize_story_layer(Some(&given))
}

fn scenes_of(items: &[Value]) -> Vec<Value> {
    items.iter().enumerate().map(|(index, scene)| normalize_storyboard_scene(scene, index as i64 + 1)).collect()
}

fn scene_list(value: Option<&Value>) -> Vec<Value> {
    match value {
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    }
}

/// `_default_storyboard`: a storyboard made from the scenes of the request when there is no file yet.
pub fn default_storyboard(payload: &Obj, now: &Stamp) -> Value {
    let normalized = scenes_of(&scene_list(payload.get("scenes")));
    let has_image = normalized.iter().any(|s| s.get("image_path").is_some_and(truthy) || s.get("image_data").is_some_and(truthy));
    let folder = absolute(Path::new(&text_or_empty(payload.get("project_folder")))).display().to_string();
    let mut data = Obj::new();
    data.insert("version".into(), json!(1));
    data.insert("created_at".into(), json!(now.iso));
    data.insert("updated_at".into(), json!(now.iso));
    data.insert("project_folder".into(), json!(folder));
    data.insert("project_video_engine".into(), json!(engine_of(payload)));
    data.insert("mode".into(), json!(if has_image { "image_to_video_prep" } else { "storyboard_prompts" }));
    data.insert("performance_mode".into(), json!(normalize_performance_mode(first_truthy(payload, &["performance_mode", "performanceMode", "video_type", "videoType"]))));
    data.insert("short_film_planning_mode".into(), json!(normalize_short_film_planning_mode(first_truthy(payload, &["short_film_planning_mode", "shortFilmPlanningMode"]))));
    data.insert("camera_flow".into(), json!(text_or(payload, &["camera_flow"], "balanced", 80)));
    data.insert("image_shot_flow".into(), json!(text_or(payload, &["image_shot_flow"], "intimate", 80)));
    data.insert("image_aesthetic".into(), json!(clean_of(payload, &["image_aesthetic"], 120)));
    data.insert("video_style".into(), json!(clean_of(payload, &["video_style", "videoStyle"], 160)));
    data.insert("video_style_custom".into(), json!(clean_of(payload, &["video_style_custom", "videoStyleCustom"], 3000)));
    data.insert("temporal_world_effect".into(), json!(clean_of(payload, &["temporal_world_effect", "temporalWorldEffect"], 160)));
    data.insert("temporal_world_effect_custom".into(), json!(clean_of(payload, &["temporal_world_effect_custom", "temporalWorldEffectCustom"], 3000)));
    data.insert("temporal_allow_background_extras".into(), json!(flag_default_true(payload, "temporal_allow_background_extras", "temporalAllowBackgroundExtras")));
    data.insert("temporal_background_intensity".into(), json!(speed_of(payload, "temporal_background_intensity", "temporalBackgroundIntensity", 8)));
    data.insert("temporal_environment_time_passage".into(), json!(flag_default_true(payload, "temporal_environment_time_passage", "temporalEnvironmentTimePassage")));
    data.insert("temporal_protected_characters".into(), json!(text_or(payload, &["temporal_protected_characters", "temporalProtectedCharacters"], "all_referenced", 80)));
    data.insert("temporal_protected_custom".into(), json!(clean_of(payload, &["temporal_protected_custom", "temporalProtectedCustom"], 1000)));
    data.insert("global_consistency_phrase".into(), json!(clean_of(payload, &["global_consistency_phrase"], 1200)));
    data.insert("camera_motion_speed".into(), json!(speed_value(first_truthy(payload, &["camera_motion_speed", "cameraMotionSpeed"]), 4)));
    data.insert("character_motion_speed".into(), json!(speed_value(first_truthy(payload, &["character_motion_speed", "characterMotionSpeed"]), 4)));
    data.insert("story_arc_detail".into(), json!(normalize_story_arc_detail(first_truthy(payload, &["story_arc_detail", "storyArcDetail"]))));
    data.insert("performance_style_default".into(), json!(clean_of(payload, &["performance_style_default", "performance_style", "performanceStyle"], 120)));
    data.insert("facial_performance_default".into(), json!(clean_of(payload, &["facial_performance_default", "facial_performance"], 120)));
    data.insert("facial_performance_custom_default".into(), json!(clean_of(payload, &["facial_performance_custom_default", "facial_performance_custom"], 1200)));
    data.insert("story_layer".into(), story_layer_of(payload));
    data.insert("script_import".into(), normalize_script_import(first_truthy(payload, &["script_import", "scriptImport"])));
    data.insert("reference_builder".into(), normalize_reference_catalog(first_truthy(payload, &["reference_builder", "referenceBuilder"])));
    data.insert("scenes".into(), Value::Array(normalized));
    Value::Object(data)
}

/// `_load_storyboard`: the saved storyboard with its scenes cleaned, or a default one when none is saved. `exists` says which.
pub fn load_storyboard(payload: &Obj, now: &Stamp) -> Result<Value, String> {
    let project = payload_folder(payload)?;
    let path = storyboard_path(&project)?;
    if path.is_file() {
        let body = fs::read_to_string(&path).map_err(|e| io_message(&path, e))?;
        let mut data: Obj = match h3_core::pyjson::parse(&body).map_err(|e| e.to_string())? {
            Value::Object(map) => map,
            _ => return Err("'list' object has no attribute 'get'".into()),
        };
        let scenes: Vec<Value> = scenes_of(&scene_list(data.get("scenes")));
        data.insert("scenes".into(), Value::Array(scenes));
        let layer = story_layer_of(&data);
        data.insert("story_layer".into(), layer);
        let script = normalize_script_import(first_truthy(&data, &["script_import", "scriptImport"]));
        data.insert("script_import".into(), script);
        let planning = normalize_short_film_planning_mode(first_truthy(&data, &["short_film_planning_mode", "shortFilmPlanningMode"]));
        data.insert("short_film_planning_mode".into(), json!(planning));
        let catalog = normalize_reference_catalog(first_truthy(&data, &["reference_builder", "referenceBuilder"]));
        data.insert("reference_builder".into(), catalog);
        data.insert("path".into(), json!(path.display().to_string()));
        data.insert("exists".into(), json!(true));
        return Ok(Value::Object(data));
    }
    let mut data = default_storyboard(payload, now);
    if let Some(map) = data.as_object_mut() {
        map.insert("path".into(), json!(path.display().to_string()));
        map.insert("exists".into(), json!(false));
    }
    Ok(data)
}

/// `_saved_storyboard_revision`: the revision counter of the storyboard on disk (0 when there is none yet).
fn saved_revision(path: &Path) -> i64 {
    let Ok(body) = fs::read_to_string(path) else { return 0 };
    let Ok(Value::Object(map)) = h3_core::pyjson::parse(&body) else { return 0 };
    map.get("revision").filter(|v| truthy(v)).and_then(to_float).map_or(0, |f| f.trunc() as i64)
}

fn int_value(value: &Value) -> Result<i64, String> {
    h3_core::pyval::to_int(value).ok_or_else(|| match value {
        Value::String(s) => format!("invalid literal for int() with base 10: '{s}'"),
        other => format!("int() argument must be a string, a bytes-like object or a real number, not '{}'", python_type(other)),
    })
}

fn python_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "NoneType",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
        _ => "str",
    }
}

fn get_or_none<'a>(source: &'a Obj, key: &str) -> Option<&'a Value> {
    source.get(key).filter(|v| !v.is_null())
}

/// `_save_storyboard`: the storyboard of the request, cleaned, written atomically with its revision counted up. With `expected_revision`
/// and another revision on disk nothing is written and the answer is a conflict, so an older window cannot overwrite newer card edits.
pub fn save_storyboard(payload: &Obj, now: &Stamp) -> Result<Value, StoryboardError> {
    let project = payload_folder(payload)?;
    let storyboard = match payload.get("storyboard") {
        None => Obj::new(),
        Some(Value::Object(map)) => map.clone(),
        Some(_) => return Err("Storyboard payload is invalid.".to_string().into()),
    };
    let path = storyboard_path(&project)?;
    let current = saved_revision(&path);
    if let Some(expected) = get_or_none(payload, "expected_revision") {
        let expected = int_value(expected)?;
        if expected != current {
            return Err(StoryboardError::Conflict {
                current_revision: current,
                message: format!("The Storyboard was changed elsewhere (saved revision {current}, this window has {expected}). Its scene cards were refreshed; review them and save again."),
            });
        }
    }
    let scenes = scene_list(storyboard.get("scenes"));
    let source_scene_ids: Vec<Value> = scene_list(storyboard.get("source_scene_ids")).iter().map(|id| json!(text_or_empty(Some(id)))).collect();
    let custom_flow: Vec<Value> = scene_list(storyboard.get("custom_camera_flow_sequence"))
        .iter()
        .filter_map(Value::as_object)
        .map(|item| json!({"shot": text(item.get("shot"), 500), "camera": text(item.get("camera"), 500)}))
        .collect();
    let get = |key: &str| storyboard.get(key);
    let mut data = Obj::new();
    data.insert("version".into(), json!(1));
    data.insert("revision".into(), json!(current + 1));
    data.insert("created_at".into(), storyboard.get("created_at").filter(|v| truthy(v)).cloned().unwrap_or_else(|| json!(now.iso)));
    data.insert("updated_at".into(), json!(now.iso));
    data.insert("project_folder".into(), json!(project.display().to_string()));
    data.insert("project_video_engine".into(), json!(engine_of(&storyboard)));
    data.insert("mode".into(), storyboard.get("mode").filter(|v| truthy(v)).cloned().unwrap_or_else(|| json!("storyboard_prompts")));
    data.insert("source_scene_ids".into(), Value::Array(source_scene_ids));
    data.insert("performance_mode".into(), json!(normalize_performance_mode(first_truthy(&storyboard, &["performance_mode", "performanceMode", "video_type", "videoType"]))));
    data.insert("short_film_planning_mode".into(), json!(normalize_short_film_planning_mode(first_truthy(&storyboard, &["short_film_planning_mode", "shortFilmPlanningMode"]))));
    data.insert("camera_flow".into(), json!(text_or(&storyboard, &["camera_flow"], "balanced", 80)));
    data.insert("send_adjacent_lyric_context".into(), json!(get("send_adjacent_lyric_context").is_some_and(truthy)));
    data.insert("keep_loaded_for_storyboard_all".into(), json!(get("keep_loaded_for_storyboard_all").is_some_and(truthy)));
    data.insert("custom_camera_flow_sequence".into(), Value::Array(custom_flow));
    data.insert("minimax_h3_cut_frequency".into(), json!(speed_value(get("minimax_h3_cut_frequency"), 0)));
    data.insert("minimax_h3_cut_style".into(), json!(if get("minimax_h3_cut_style").and_then(Value::as_str) == Some("continuous") { "continuous" } else { "free" }));
    data.insert("defaults_speed_preset".into(), json!(clean_of(&storyboard, &["defaults_speed_preset"], 40)));
    data.insert("defaults_music_preset".into(), json!(clean_of(&storyboard, &["defaults_music_preset"], 40)));
    data.insert("fx_preset".into(), json!(clean_of(&storyboard, &["fx_preset"], 120)));
    data.insert("fx_custom_json".into(), json!(text_or_empty(get("fx_custom_json"))));
    data.insert("image_shot_flow".into(), json!(text_or(&storyboard, &["image_shot_flow"], "intimate", 80)));
    data.insert("image_aesthetic".into(), json!(clean_of(&storyboard, &["image_aesthetic"], 120)));
    data.insert("video_style".into(), json!(clean_of(&storyboard, &["video_style", "videoStyle"], 160)));
    data.insert("video_style_custom".into(), json!(clean_of(&storyboard, &["video_style_custom", "videoStyleCustom"], 3000)));
    data.insert("temporal_world_effect".into(), json!(clean_of(&storyboard, &["temporal_world_effect", "temporalWorldEffect"], 160)));
    data.insert("temporal_world_effect_custom".into(), json!(clean_of(&storyboard, &["temporal_world_effect_custom", "temporalWorldEffectCustom"], 3000)));
    data.insert("temporal_allow_background_extras".into(), json!(flag_default_true(&storyboard, "temporal_allow_background_extras", "temporalAllowBackgroundExtras")));
    data.insert("temporal_background_intensity".into(), json!(speed_of(&storyboard, "temporal_background_intensity", "temporalBackgroundIntensity", 8)));
    data.insert("temporal_environment_time_passage".into(), json!(flag_default_true(&storyboard, "temporal_environment_time_passage", "temporalEnvironmentTimePassage")));
    data.insert("temporal_protected_characters".into(), json!(text_or(&storyboard, &["temporal_protected_characters", "temporalProtectedCharacters"], "all_referenced", 80)));
    data.insert("temporal_protected_custom".into(), json!(clean_of(&storyboard, &["temporal_protected_custom", "temporalProtectedCustom"], 1000)));
    data.insert("global_consistency_phrase".into(), json!(clean_of(&storyboard, &["global_consistency_phrase"], 1200)));
    let speed_key = |key: &str, camel: &str| speed_value(storyboard.get(key).or_else(|| storyboard.get(camel)), 4);
    data.insert("camera_motion_speed".into(), json!(speed_key("camera_motion_speed", "cameraMotionSpeed")));
    data.insert("character_motion_speed".into(), json!(speed_key("character_motion_speed", "characterMotionSpeed")));
    data.insert("story_arc_detail".into(), json!(normalize_story_arc_detail(first_truthy(&storyboard, &["story_arc_detail", "storyArcDetail"]))));
    data.insert("performance_style_default".into(), json!(clean_of(&storyboard, &["performance_style_default", "performance_style", "performanceStyle"], 120)));
    data.insert("facial_performance_default".into(), json!(clean_of(&storyboard, &["facial_performance_default", "facial_performance"], 120)));
    data.insert("facial_performance_custom_default".into(), json!(clean_of(&storyboard, &["facial_performance_custom_default", "facial_performance_custom"], 1200)));
    data.insert("story_layer".into(), story_layer_of(&storyboard));
    data.insert("script_import".into(), normalize_script_import(first_truthy(&storyboard, &["script_import", "scriptImport"])));
    data.insert("reference_builder".into(), normalize_reference_catalog(first_truthy(&storyboard, &["reference_builder", "referenceBuilder"])));
    data.insert("scenes".into(), Value::Array(scenes_of(&scenes)));
    let mut saved = Value::Object(data);
    atomic_write_json(&path, &saved).map_err(|e| io_message(&path, e))?;
    if let Some(map) = saved.as_object_mut() {
        map.insert("path".into(), json!(path.display().to_string()));
    }
    Ok(saved)
}

fn write_key_value_file(path: &Path, prefix: &str, scenes: &[Value], field: &str) -> Result<(), String> {
    let limit = if field == "video_prompt" { 100_000 } else { 12_000 };
    let lines: String = scenes.iter().enumerate().map(|(index, scene)| format!("{prefix}{}={}\n", index + 1, text(scene.get(field), limit))).collect();
    atomic_write_text(path, &lines).map_err(|e| io_message(path, e))
}

fn prompt_json_entry(scene: &Value, index: usize, field: &str) -> Obj {
    let limit = if field == "video_prompt" { 100_000 } else { 12_000 };
    let label = text(scene.get("label"), 200);
    let mut entry = Obj::new();
    entry.insert("scene".into(), json!(index));
    entry.insert("scene_id".into(), json!(text(scene.get("id"), 120)));
    entry.insert("label".into(), json!(if scene.get("label").is_some_and(truthy) { label } else { format!("Scene {index}") }));
    entry.insert("lyric_section".into(), json!(text(scene.get("lyric_section"), 160)));
    entry.insert("lyric_line".into(), json!(text(scene.get("lyrics"), 1200)));
    entry.insert("prompt".into(), json!(text(scene.get(field), limit)));
    entry
}

/// The pass 2 prompts an earlier export holds, by scene id and by scene number.
fn existing_pass2(video_json_path: &Path) -> (std::collections::HashMap<String, String>, std::collections::HashMap<i64, String>) {
    let mut by_id = std::collections::HashMap::new();
    let mut by_number = std::collections::HashMap::new();
    let Ok(body) = fs::read_to_string(video_json_path) else { return (by_id, by_number) };
    let Ok(Value::Object(existing)) = h3_core::pyjson::parse(&body) else { return (by_id, by_number) };
    for item in scene_list(existing.get("scenes")).iter().filter_map(Value::as_object) {
        if !item.contains_key("pass2_prompt") {
            continue;
        }
        let pass2 = text_or_empty(item.get("pass2_prompt"));
        let scene_id = text_or_empty(item.get("scene_id"));
        if !scene_id.is_empty() {
            by_id.insert(scene_id, pass2.clone());
        }
        match item.get("scene").filter(|v| truthy(v)) {
            None => {
                by_number.insert(0, pass2);
            }
            Some(number) => {
                if let Some(number) = h3_core::pyval::to_int(number) {
                    by_number.insert(number, pass2);
                }
            }
        }
    }
    (by_id, by_number)
}

/// `_export_storyboard_prompts`: save the storyboard, then write its image and video prompts as text and JSON files in `prompts/`.
pub fn export_prompts(payload: &Obj, now: &Stamp) -> Result<Value, StoryboardError> {
    let saved = save_storyboard(payload, now)?;
    let project = payload_folder(payload)?;
    let prompts_dir = prompts_folder(&project)?;
    let scenes = scene_list(saved.get("scenes"));
    let t2i_path = prompts_dir.join("t2i_prompts.txt");
    let i2v_path = prompts_dir.join("i2v_prompts.txt");
    let t2i_json_path = prompts_dir.join("t2i_prompts.json");
    let video_json_path = prompts_dir.join("video_prompts.json");
    let summary_path = storyboard_folder(&project)?.join("storyboard_export.json");
    write_key_value_file(&t2i_path, "Prompt", &scenes, "image_prompt")?;
    write_key_value_file(&i2v_path, "I2V", &scenes, "video_prompt")?;
    let t2i_json = json!({
        "version": 1,
        "exported_at": now.iso,
        "type": "storyboard_t2i_prompts",
        "scene_count": scenes.len(),
        "scenes": scenes.iter().enumerate().map(|(i, s)| Value::Object(prompt_json_entry(s, i + 1, "image_prompt"))).collect::<Vec<_>>(),
    });
    let (pass2_by_id, pass2_by_number) = existing_pass2(&video_json_path);
    let no_fields = Obj::new();
    let exported_pass2 = |scene: &Value, index: usize| -> String {
        let own = text(first_truthy(scene.as_object().unwrap_or(&no_fields), &["minimax_h3_pass2_prompt", "pass2_prompt"]), 100_000);
        if !own.is_empty() {
            return own;
        }
        let id = text_or_empty(scene.get("id"));
        pass2_by_id.get(&id).filter(|s| !s.is_empty()).cloned().or_else(|| pass2_by_number.get(&(index as i64)).filter(|s| !s.is_empty()).cloned()).unwrap_or_default()
    };
    let saved_mode = saved.get("performance_mode").cloned();
    let video_scenes: Vec<Value> = scenes
        .iter()
        .enumerate()
        .map(|(i, scene)| {
            let mut entry = prompt_json_entry(scene, i + 1, "video_prompt");
            entry.insert("video_prompt_type".into(), json!(text(scene.get("video_prompt_type"), 80)));
            entry.insert("minimax_h3_mode".into(), json!(text(scene.get("minimax_h3_mode"), 80)));
            entry.insert("video_style".into(), json!(text(scene.get("video_style"), 160)));
            entry.insert("video_style_custom".into(), json!(text(scene.get("video_style_custom"), 3000)));
            let mode = scene.get("performance_mode").filter(|v| truthy(v)).or(saved_mode.as_ref().filter(|v| truthy(v)));
            entry.insert("performance_mode".into(), json!(normalize_performance_mode(mode)));
            entry.insert("pass2_prompt".into(), json!(exported_pass2(scene, i + 1)));
            Value::Object(entry)
        })
        .collect();
    let engine = saved.get("project_video_engine").filter(|v| truthy(v)).cloned().unwrap_or_else(|| json!("ltx"));
    let mode = saved.get("performance_mode").filter(|v| truthy(v)).cloned().unwrap_or_else(|| json!("singing"));
    let video_json = json!({
        "version": 1,
        "exported_at": now.iso,
        "type": "storyboard_video_prompts",
        "project_video_engine": engine,
        "performance_mode": mode,
        "scene_count": scenes.len(),
        "scenes": video_scenes,
    });
    atomic_write_json(&t2i_json_path, &t2i_json).map_err(|e| io_message(&t2i_json_path, e))?;
    atomic_write_json(&video_json_path, &video_json).map_err(|e| io_message(&video_json_path, e))?;
    let summary = json!({
        "version": 1,
        "exported_at": now.iso,
        "t2i_prompts": t2i_path.display().to_string(),
        "i2v_prompts": i2v_path.display().to_string(),
        "t2i_prompts_json": t2i_json_path.display().to_string(),
        "video_prompts_json": video_json_path.display().to_string(),
        "scenes": scenes,
    });
    atomic_write_json(&summary_path, &summary).map_err(|e| io_message(&summary_path, e))?;
    Ok(json!({
        "storyboard_path": saved.get("path").cloned().unwrap_or_else(|| json!("")),
        "storyboard_revision": saved.get("revision").cloned().unwrap_or_else(|| json!(0)),
        "t2i_prompts_path": t2i_path.display().to_string(),
        "i2v_prompts_path": i2v_path.display().to_string(),
        "t2i_prompts_json_path": t2i_json_path.display().to_string(),
        "video_prompts_json_path": video_json_path.display().to_string(),
        "export_path": summary_path.display().to_string(),
        "scene_count": scenes.len(),
    }))
}
