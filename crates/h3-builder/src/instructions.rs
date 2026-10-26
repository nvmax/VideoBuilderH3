//! The instruction editor of the Builder: the default LLM instructions, a project's own versions (for all scenes or one
//! scene) and named presets. Port of `llm/builder_instructions.py`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use h3_core::pyval::{text_or_empty, Obj};
use h3_llm::instructions as defaults;
use h3_project::atomic::atomic_write_text;
use h3_project::layout::context_folder;
use serde_json::{json, Value};

use crate::payload::path_text;
use crate::Env;

type Reply = Result<Value, String>;

fn absolute(path: &Path) -> PathBuf {
    h3_core::abspath::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

fn text(path: &Path) -> String {
    path.display().to_string()
}

/// `_safe_preset_name`: letters, digits, `_`, `.`, `-` and spaces, at most 80 characters.
fn safe_preset_name(value: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut pending = false;
    for c in value.trim().chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ' ' | '-') {
            out.push(c);
            pending = false;
        } else if !pending {
            out.push('_');
            pending = true;
        }
    }
    let name = out.trim_matches([' ', '.', '_']).to_string();
    if name.is_empty() {
        return Err("Preset name is empty.".into());
    }
    Ok(name.chars().take(80).collect())
}

/// `_safe_builder_scene_id`: `[^A-Za-z0-9_.-]+` becomes `_`, `._-` is trimmed, at most 120 characters.
fn safe_scene_id(value: &str) -> String {
    let mut out = String::new();
    let mut pending = false;
    for c in value.trim().chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
            out.push(c);
            pending = false;
        } else if !pending {
            out.push('_');
            pending = true;
        }
    }
    out.trim_matches(['.', '_', '-']).chars().take(120).collect()
}

fn instruction_folder(project: &Path) -> PathBuf {
    context_folder(project).join("custom_builder_instructions")
}

fn all_scenes_path(project: &Path, key: &str) -> PathBuf {
    instruction_folder(project).join(format!("{key}.txt"))
}

fn scene_path(project: &Path, key: &str, scene_id: &str) -> Result<PathBuf, String> {
    let scene = safe_scene_id(scene_id);
    if scene.is_empty() {
        return Err("Scene id is missing.".into());
    }
    Ok(instruction_folder(project).join("scenes").join(scene).join(format!("{key}.txt")))
}

fn preset_root(env: &Env) -> PathBuf {
    env.output.join("VRGDG_LLM_Instruction_Presets").join("builder")
}

fn preset_path(env: &Env, key: &str, name: &str) -> Result<PathBuf, String> {
    Ok(preset_root(env).join(defaults::preset_group(key)).join(format!("{}.txt", safe_preset_name(name)?)))
}

fn legacy_preset_path(env: &Env, key: &str, name: &str) -> Result<PathBuf, String> {
    Ok(preset_root(env).join(key).join(format!("{}.txt", safe_preset_name(name)?)))
}

/// `_read_optional_text_file`: the trimmed text, or "" when there is no such file.
fn read_optional(path: &Path) -> String {
    if !path.is_file() {
        return String::new();
    }
    fs::read(path).map(|b| String::from_utf8_lossy(&b).trim_start_matches('\u{feff}').trim().to_string()).unwrap_or_default()
}

fn project_of(payload: &Obj) -> Result<PathBuf, String> {
    let raw = path_text(payload, "project_folder");
    if raw.is_empty() {
        return Err("Create or load a Builder project before editing instructions.".into());
    }
    Ok(absolute(Path::new(&raw)))
}

fn scene_of(payload: &Obj) -> String {
    text_or_empty(payload.get("scene_id"))
}

/// `_builder_instruction_state`.
fn state(project: &Path, key: &str, scene_id: &str) -> Result<Value, String> {
    let default_text = defaults::default_text(key).unwrap_or("").to_string();
    let (mut scene_file, mut scene_text) = (String::new(), String::new());
    if !scene_id.is_empty() {
        let path = scene_path(project, key, scene_id)?;
        scene_text = read_optional(&path);
        scene_file = text(&path);
    }
    let all_file = all_scenes_path(project, key);
    let all_text = read_optional(&all_file);
    let (source, current, path) = if !scene_text.is_empty() {
        ("scene", scene_text.clone(), scene_file.clone())
    } else if !all_text.is_empty() {
        ("all_scenes", all_text.clone(), text(&all_file))
    } else {
        ("default", default_text.clone(), String::new())
    };
    Ok(json!({
        "key": key,
        "label": defaults::label(key),
        "scene_id": scene_id,
        "default_text": default_text,
        "scene_text": scene_text,
        "all_scenes_text": all_text,
        "text": current,
        "source": source,
        "path": path,
        "scene_path": scene_file,
        "all_scenes_path": text(&all_file),
        "has_scene_custom": !scene_text.is_empty(),
        "has_all_scenes_custom": !all_text.is_empty(),
    }))
}

/// `_effective_builder_instruction`: the instruction a generation uses: the scene's own, the project's, or the default.
pub fn effective_instruction(payload: &Obj, key: &str, default_text: &str) -> String {
    let raw = path_text(payload, "project_folder");
    if raw.is_empty() {
        return default_text.to_string();
    }
    match state(&absolute(Path::new(&raw)), key, &scene_of(payload)) {
        Ok(found) => Some(text_or_empty(found.get("text"))).filter(|t| !t.is_empty()).unwrap_or_else(|| default_text.to_string()),
        Err(_) => default_text.to_string(),
    }
}

/// `_prompt_creator_custom_instruction`: a project's own text for a Gemma4 draft step (`project_context/custom_llm_instructions/<key>.txt`), or the default.
pub fn prompt_creator_instruction(payload: &Obj, key: &str, default_text: &str) -> String {
    let mut safe_key = String::new();
    let mut pending = false;
    for c in key.trim().to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' {
            safe_key.push(c);
            pending = false;
        } else if !pending {
            safe_key.push('_');
            pending = true;
        }
    }
    let safe_key = safe_key.trim_matches('_');
    if safe_key.is_empty() {
        return default_text.to_string();
    }
    let project = absolute(Path::new(&path_text(payload, "project_folder")));
    let path = context_folder(&project).join("custom_llm_instructions").join(format!("{safe_key}.txt"));
    let found = fs::read(&path).ok().and_then(|bytes| String::from_utf8(bytes).ok()).map(|body| body.trim_start_matches('\u{feff}').trim().to_string());
    found.filter(|body| !body.is_empty()).unwrap_or_else(|| default_text.to_string())
}

fn merged(project: &Path, state: Value) -> Value {
    let mut map = serde_json::Map::new();
    map.insert("project_folder".into(), json!(text(project)));
    if let Value::Object(fields) = state {
        map.extend(fields);
    }
    Value::Object(map)
}

fn is_all_scenes(payload: &Obj) -> bool {
    let scope = Some(text_or_empty(payload.get("scope")).trim().to_lowercase()).filter(|s| !s.is_empty()).unwrap_or_else(|| "scene".into());
    matches!(scope.as_str(), "all" | "all_scenes" | "global")
}

/// `POST /vrgdg/music_builder/get_instruction`.
pub fn get_instruction(payload: &Obj) -> Reply {
    let project = project_of(payload)?;
    let key = h3_llm::instructions::safe_key(&text_or_empty(payload.get("key")))?;
    Ok(merged(&project, state(&project, &key, &scene_of(payload))?))
}

/// `POST /vrgdg/music_builder/save_instruction`.
pub fn save_instruction(payload: &Obj) -> Reply {
    let project = project_of(payload)?;
    let key = h3_llm::instructions::safe_key(&text_or_empty(payload.get("key")))?;
    let body = text_or_empty(payload.get("text")).trim().to_string();
    if body.is_empty() {
        return Err("Instruction text is empty.".into());
    }
    let path = if is_all_scenes(payload) { all_scenes_path(&project, &key) } else { scene_path(&project, &key, &scene_of(payload))? };
    atomic_write_text(&path, &format!("{body}\n")).map_err(|e| e.to_string())?;
    Ok(merged(&project, state(&project, &key, &scene_of(payload))?))
}

/// `POST /vrgdg/music_builder/reset_instruction`.
pub fn reset_instruction(payload: &Obj) -> Reply {
    let project = project_of(payload)?;
    let key = h3_llm::instructions::safe_key(&text_or_empty(payload.get("key")))?;
    let path = if is_all_scenes(payload) { all_scenes_path(&project, &key) } else { scene_path(&project, &key, &scene_of(payload))? };
    if path.is_file() {
        fs::remove_file(&path).map_err(|e| e.to_string())?;
    }
    Ok(merged(&project, state(&project, &key, &scene_of(payload))?))
}

fn seconds(path: &Path) -> f64 {
    fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0.0, |d| d.as_secs_f64())
}

/// `POST /vrgdg/music_builder/list_instruction_presets`.
pub fn list_presets(env: &Env, payload: &Obj) -> Reply {
    let key = h3_llm::instructions::safe_key(&text_or_empty(payload.get("key")))?;
    let group = defaults::preset_group(&key);
    let folder = preset_root(env).join(&group);
    let legacy = preset_root(env).join(&key);
    let mut folders = vec![(folder.clone(), false)];
    if absolute(&legacy).display().to_string().to_lowercase() != absolute(&folder).display().to_string().to_lowercase() {
        folders.push((legacy, true));
    }
    let mut presets: Vec<(f64, Value)> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for (scan, is_legacy) in folders {
        let Ok(entries) = fs::read_dir(&scan) else { continue };
        for entry in entries.flatten() {
            let file = entry.file_name().to_string_lossy().into_owned();
            if !file.to_lowercase().ends_with(".txt") {
                continue;
            }
            let name = file[..file.len() - 4].to_string();
            let path = entry.path();
            if seen.contains(&name.to_lowercase()) || !path.is_file() {
                continue;
            }
            seen.push(name.to_lowercase());
            let updated = seconds(&path);
            presets.push((updated, json!({"name": name, "path": text(&absolute(&path)), "updated": updated, "legacy": is_legacy})));
        }
    }
    presets.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    Ok(json!({
        "key": key,
        "label": defaults::label(&key),
        "preset_group": group,
        "preset_group_label": defaults::preset_group_label(&key),
        "presets": presets.into_iter().map(|(_, v)| v).collect::<Vec<_>>(),
        "preset_folder": text(&folder),
    }))
}

/// `POST /vrgdg/music_builder/save_instruction_preset`.
pub fn save_preset(env: &Env, payload: &Obj) -> Reply {
    let key = h3_llm::instructions::safe_key(&text_or_empty(payload.get("key")))?;
    let name = safe_preset_name(&text_or_empty(payload.get("name")))?;
    let body = text_or_empty(payload.get("text")).trim().to_string();
    if body.is_empty() {
        return Err("Preset instruction text is empty.".into());
    }
    let path = preset_path(env, &key, &name)?;
    atomic_write_text(&path, &format!("{body}\n")).map_err(|e| e.to_string())?;
    Ok(json!({
        "key": key,
        "name": name,
        "path": text(&path),
        "preset_folder": path.parent().map(text).unwrap_or_default(),
        "preset_group": defaults::preset_group(&key),
        "preset_group_label": defaults::preset_group_label(&key),
    }))
}

/// `POST /vrgdg/music_builder/load_instruction_preset`.
pub fn load_preset(env: &Env, payload: &Obj) -> Reply {
    let key = h3_llm::instructions::safe_key(&text_or_empty(payload.get("key")))?;
    let name = safe_preset_name(&text_or_empty(payload.get("name")))?;
    let mut path = preset_path(env, &key, &name)?;
    let mut body = read_optional(&path);
    if body.is_empty() {
        let legacy = legacy_preset_path(env, &key, &name)?;
        if absolute(&legacy) != absolute(&path) {
            let found = read_optional(&legacy);
            if !found.is_empty() {
                path = legacy;
                body = found;
            }
        }
    }
    if body.is_empty() {
        return Err(format!("Instruction preset was not found or is empty: {}", path.display()));
    }
    Ok(json!({
        "key": key,
        "name": name,
        "path": text(&path),
        "preset_folder": path.parent().map(text).unwrap_or_default(),
        "preset_group": defaults::preset_group(&key),
        "preset_group_label": defaults::preset_group_label(&key),
        "text": body,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(value: Value) -> Obj {
        value.as_object().cloned().unwrap()
    }

    fn env(dir: &Path) -> Env {
        Env { output: dir.join("output"), roots: vec![], ffmpeg: "ffmpeg".into(), ffprobe: "ffprobe".into(), luts: dir.join("luts") }
    }

    #[test]
    fn an_instruction_is_the_default_until_the_project_or_scene_changes_it() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("p").display().to_string();
        let ask = json!({"project_folder": project, "key": "zimage_t2i", "scene_id": "scene 1"});
        let first = get_instruction(&obj(ask.clone())).unwrap();
        assert_eq!(first["source"], "default");
        assert_eq!(first["text"], first["default_text"]);
        assert_eq!(first["label"], "ZImage Text to Image");
        let saved_all = save_instruction(&obj(json!({"project_folder": project, "key": "zimage_t2i", "scope": "all_scenes", "text": " all scenes text "}))).unwrap();
        assert_eq!(saved_all["source"], "all_scenes");
        assert_eq!(saved_all["text"], "all scenes text");
        assert_eq!(get_instruction(&obj(ask.clone())).unwrap()["source"], "all_scenes");
        let scene = save_instruction(&obj(json!({"project_folder": project, "key": "zimage_t2i", "scene_id": "scene 1", "text": "scene text"}))).unwrap();
        assert_eq!((scene["source"].as_str(), scene["has_scene_custom"].as_bool(), scene["has_all_scenes_custom"].as_bool()), (Some("scene"), Some(true), Some(true)));
        assert!(scene["scene_path"].as_str().unwrap().replace('\\', "/").ends_with("custom_builder_instructions/scenes/scene_1/zimage_t2i.txt"));
        assert_eq!(effective_instruction(&obj(ask.clone()), "zimage_t2i", "fallback"), "scene text");
        assert_eq!(effective_instruction(&obj(json!({"project_folder": project, "key": "zimage_t2i", "scene_id": "scene 2"})), "zimage_t2i", "fallback"), "all scenes text");
        let reset = reset_instruction(&obj(json!({"project_folder": project, "key": "zimage_t2i", "scene_id": "scene 1"}))).unwrap();
        assert_eq!(reset["source"], "all_scenes");
        let reset_all = reset_instruction(&obj(json!({"project_folder": project, "key": "zimage_t2i", "scope": "global"}))).unwrap();
        assert_eq!(reset_all["source"], "default");
        assert_eq!(effective_instruction(&obj(json!({"key": "zimage_t2i"})), "zimage_t2i", "fallback"), "fallback");
    }

    #[test]
    fn bad_requests_get_the_packs_messages() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().display().to_string();
        assert_eq!(get_instruction(&obj(json!({"key": "zimage_t2i"}))).unwrap_err(), "Create or load a Builder project before editing instructions.");
        assert_eq!(get_instruction(&obj(json!({"project_folder": project, "key": "nope"}))).unwrap_err(), "Unknown Builder instruction key: nope");
        assert_eq!(save_instruction(&obj(json!({"project_folder": project, "key": "zimage_t2i", "scene_id": "s", "text": "  "}))).unwrap_err(), "Instruction text is empty.");
        assert_eq!(save_instruction(&obj(json!({"project_folder": project, "key": "zimage_t2i", "text": "x"}))).unwrap_err(), "Scene id is missing.");
    }

    #[test]
    fn presets_are_shared_inside_a_group_and_the_newest_comes_first() {
        let dir = tempfile::tempdir().unwrap();
        let env = env(dir.path());
        let saved = save_preset(&env, &obj(json!({"key": "zimage_t2i", "name": "My: Style?", "text": "Be brief."}))).unwrap();
        assert_eq!(saved["name"], "My_ Style");
        assert_eq!(saved["preset_group"], "standard_image_t2i");
        // A preset saved for one Standard Image model shows for the others.
        let listed = list_presets(&env, &obj(json!({"key": "krea2_t2i"}))).unwrap();
        assert_eq!(listed["presets"].as_array().unwrap().len(), 1);
        assert_eq!(listed["preset_group_label"], "Standard Image T2I");
        let loaded = load_preset(&env, &obj(json!({"key": "ernie_t2i", "name": "My: Style?"}))).unwrap();
        assert_eq!(loaded["text"], "Be brief.");
        let legacy = env.output.join("VRGDG_LLM_Instruction_Presets").join("builder").join("krea2_t2i");
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("Old.txt"), "old text").unwrap();
        let both = list_presets(&env, &obj(json!({"key": "krea2_t2i"}))).unwrap();
        assert_eq!(both["presets"].as_array().unwrap().len(), 2);
        assert_eq!(load_preset(&env, &obj(json!({"key": "krea2_t2i", "name": "Old"}))).unwrap()["text"], "old text");
        assert!(load_preset(&env, &obj(json!({"key": "krea2_t2i", "name": "Missing"}))).unwrap_err().starts_with("Instruction preset was not found or is empty: "));
        assert_eq!(save_preset(&env, &obj(json!({"key": "krea2_t2i", "name": "x", "text": " "}))).unwrap_err(), "Preset instruction text is empty.");
        assert_eq!(save_preset(&env, &obj(json!({"key": "krea2_t2i", "name": "...", "text": "x"}))).unwrap_err(), "Preset name is empty.");
    }
}
