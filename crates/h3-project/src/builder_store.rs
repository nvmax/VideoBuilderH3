//! Settings the Video Builder keeps beside its projects, in ComfyUI's output folder: model defaults, the LLM runner and
//! its keys, named video profiles, named layout (UI) profiles, Scene Defaults presets, Adjust presets and the model
//! folder. Ports of `builder/project.py` (model defaults and global LLM settings), `builder/video_profiles.py`,
//! `builder/ui_profiles.py`, `builder/scene_defaults_presets.py`, `core/model_paths.py` and the preset functions in
//! `post_process/lut_video_tools.py`.
//!
//! The folders and file formats are the pack's, so both apps read the same profiles and defaults. Every function takes
//! the output folder (ComfyUI's, or the app's own when ComfyUI's is not known).

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use h3_core::pyval::{collapse_ws, text_or_empty, to_float, truthy, Obj};
use serde_json::{json, Map, Value};

use crate::atomic::{atomic_write_json, atomic_write_text};
use crate::layout::Stamp;
use crate::pyjson;

/// Why a store call failed. The routes map these to the pack's status codes.
#[derive(Debug)]
pub enum StoreError {
    Invalid(String),
    NotFound(String),
    /// A profile or preset with this name exists and `overwrite` was not set.
    Exists { name: String, message: String },
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Invalid(m) | StoreError::NotFound(m) | StoreError::Exists { message: m, .. } => f.write_str(m),
        }
    }
}

impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self {
        StoreError::Invalid(e.to_string())
    }
}

type Stored<T> = Result<T, StoreError>;

fn invalid<T>(message: impl Into<String>) -> Stored<T> {
    Err(StoreError::Invalid(message.into()))
}

fn read_json(path: &Path) -> Option<Value> {
    let text = fs::read_to_string(path).ok()?;
    pyjson::parse(text.trim_start_matches('\u{feff}')).ok()
}

fn read_object(path: &Path) -> Obj {
    match read_json(path) {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    }
}

fn path_text(path: &Path) -> String {
    path.display().to_string()
}

fn json_files(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else { return Vec::new() };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("json")))
        .collect()
}

// -- model defaults and the global LLM settings -------------------------------------------------------------------------

const MODEL_DEFAULT_KEYS: [&str; 28] = [
    "text_gemma_runner",
    "qwen_model_file",
    "qwen_mmproj_file",
    "gemma_model_file",
    "llm_max_tokens",
    "gemma_context_limit",
    "gemma_output_token_limit",
    "gemma_gpu_layers",
    "lm_studio_base_url",
    "lm_studio_model",
    "lm_studio_api_key",
    "lm_studio_context_limit",
    "lm_studio_output_token_limit",
    "llm_api_provider",
    "llm_api_model",
    "own_server_url",
    "own_server_model",
    "own_server_output_token_limit",
    "own_server_timeout",
    "image_model_mode",
    "zimage_settings",
    "reference_krea2_settings",
    "flux_klein_settings",
    "ernie_image_settings",
    "krea2_2pass_settings",
    "z_enhance_settings",
    "video_model_mode",
    "i2v_video_settings",
];

/// API keys kept for every project, never inside a project's session.
const GLOBAL_LLM_KEY_FIELDS: [&str; 3] = ["llm_api_key", "own_server_api_key", "elevenlabs_api_key"];

/// LLM runner settings kept for every project, in session key names.
const GLOBAL_LLM_RUNNER_FIELDS: [&str; 18] = [
    "text_gemma_runner",
    "gemma_model_file",
    "qwen_model_file",
    "qwen_mmproj_file",
    "gemma_context_limit",
    "gemma_output_token_limit",
    "gemma_gpu_layers",
    "lm_studio_base_url",
    "lm_studio_model",
    "lm_studio_api_key",
    "lm_studio_context_limit",
    "lm_studio_output_token_limit",
    "llm_api_provider",
    "llm_api_model",
    "own_server_url",
    "own_server_model",
    "own_server_output_token_limit",
    "own_server_timeout",
];

fn defaults_folder(output: &Path) -> PathBuf {
    output.join("VRGDG_Model_Defaults")
}

pub fn model_defaults_path(output: &Path) -> PathBuf {
    let folder = defaults_folder(output);
    let _ = fs::create_dir_all(&folder);
    folder.join("model_defaults.json")
}

/// The global LLM keys as `{field: key}`, only the ones that are set.
pub fn load_global_llm_keys(output: &Path) -> Obj {
    let data = read_object(&defaults_folder(output).join("llm_keys.json"));
    let mut keys = Map::new();
    for name in GLOBAL_LLM_KEY_FIELDS {
        let value = text_or_empty(data.get(name)).trim().to_string();
        if !value.is_empty() {
            keys.insert(name.into(), json!(value));
        }
    }
    keys
}

/// Set the keys named in `updates`. An empty value clears that key and keys not named stay.
pub fn save_global_llm_keys(output: &Path, updates: Option<&Value>) -> std::io::Result<()> {
    let Some(Value::Object(updates)) = updates else { return Ok(()) };
    let mut keys = load_global_llm_keys(output);
    let mut changed = false;
    for name in GLOBAL_LLM_KEY_FIELDS {
        let Some(value) = updates.get(name) else { continue };
        let value = text_or_empty(Some(value)).trim().to_string();
        if value.is_empty() {
            keys.shift_remove(name);
        } else {
            keys.insert(name.into(), json!(value));
        }
        changed = true;
    }
    if changed {
        atomic_write_json(&defaults_folder(output).join("llm_keys.json"), &Value::Object(keys))?;
    }
    Ok(())
}

/// The saved LLM runner settings. Only fields that were saved with a value.
pub fn load_global_llm_runner(output: &Path) -> Obj {
    let data = read_object(&defaults_folder(output).join("llm_runner.json"));
    let mut settings = Map::new();
    for name in GLOBAL_LLM_RUNNER_FIELDS {
        if let Some(value) = data.get(name).filter(|v| !v.is_null() && v.as_str() != Some("")) {
            settings.insert(name.into(), value.clone());
        }
    }
    settings
}

/// Store the runner settings named in `updates`. Settings not named stay.
pub fn save_global_llm_runner(output: &Path, updates: Option<&Value>) -> std::io::Result<Obj> {
    let mut settings = load_global_llm_runner(output);
    let Some(Value::Object(updates)) = updates else { return Ok(settings) };
    let mut changed = false;
    for name in GLOBAL_LLM_RUNNER_FIELDS {
        if let Some(value) = updates.get(name).filter(|v| !v.is_null()) {
            if settings.get(name) != Some(value) {
                settings.insert(name.into(), value.clone());
                changed = true;
            }
        }
    }
    if changed {
        atomic_write_json(&defaults_folder(output).join("llm_runner.json"), &Value::Object(settings.clone()))?;
    }
    Ok(settings)
}

/// `POST /vrgdg/music_builder/llm_runner`.
pub fn save_global_llm_settings(output: &Path, payload: &Obj) -> std::io::Result<Value> {
    let runner = save_global_llm_runner(output, payload.get("runner"))?;
    save_global_llm_keys(output, payload.get("keys"))?;
    Ok(json!({"global_llm_runner": runner, "global_llm_keys": load_global_llm_keys(output)}))
}

/// Defaults carry no per project image source.
fn scrub_model_defaults(defaults: &Obj) -> Obj {
    let mut cleaned = defaults.clone();
    for key in ["zimage_settings", "ernie_image_settings", "krea2_2pass_settings"] {
        if let Some(Value::Object(settings)) = cleaned.get_mut(key) {
            settings.insert("use_image_to_image".into(), json!(false));
            for field in ["image_to_image_path", "image_to_image_data", "image_to_image_name"] {
                settings.insert(field.into(), json!(""));
            }
        }
    }
    cleaned
}

/// `GET /vrgdg/music_builder/model_defaults`.
pub fn load_model_defaults(output: &Path) -> Stored<Value> {
    let target = model_defaults_path(output);
    let (defaults, saved_at) = if target.is_file() {
        let text = fs::read_to_string(&target)?;
        let Value::Object(payload) = pyjson::parse(text.trim_start_matches('\u{feff}')).map_err(|e| StoreError::Invalid(e.to_string()))? else {
            return Ok(json!({"path": path_text(&target), "defaults": {}, "saved_at": "", "global_llm_keys": load_global_llm_keys(output), "global_llm_runner": load_global_llm_runner(output)}));
        };
        let defaults = payload.get("defaults").and_then(Value::as_object).map(scrub_model_defaults).unwrap_or_default();
        (defaults, text_or_empty(payload.get("saved_at")))
    } else {
        (Map::new(), String::new())
    };
    Ok(json!({
        "path": path_text(&target),
        "defaults": defaults,
        "saved_at": saved_at,
        "global_llm_keys": load_global_llm_keys(output),
        "global_llm_runner": load_global_llm_runner(output),
    }))
}

/// Keep the model related settings of a saved session as the defaults for new projects. Returns the file written.
pub fn save_model_defaults(output: &Path, session: &Obj, now: &Stamp) -> std::io::Result<Option<PathBuf>> {
    let mut defaults = Map::new();
    for key in MODEL_DEFAULT_KEYS {
        if let Some(value) = session.get(key).filter(|v| !v.is_null()) {
            defaults.insert(key.into(), value.clone());
        }
    }
    let defaults = scrub_model_defaults(&defaults);
    if defaults.is_empty() {
        return Ok(None);
    }
    let target = model_defaults_path(output);
    atomic_write_json(&target, &json!({"saved_at": now.iso.replace('T', " "), "defaults": defaults}))?;
    Ok(Some(target))
}

// -- model folder ---------------------------------------------------------------------------------------------------------

fn normalize_model_root(value: &str) -> String {
    let text = value.trim().trim_matches('"');
    if text.is_empty() {
        return String::new();
    }
    let expanded = match text.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with(['/', '\\']) => dirs::home_dir().map(|h| h.join(rest.trim_start_matches(['/', '\\']))).unwrap_or_else(|| PathBuf::from(text)),
        _ => PathBuf::from(text),
    };
    path_text(&std::path::absolute(&expanded).unwrap_or(expanded))
}

/// `GET /vrgdg/workflow_runner/model_root`: a folder with `diffusion_models`, `loras` and the like that the user keeps
/// apart from ComfyUI's own `models` folder.
pub fn load_custom_model_root(output: &Path) -> Value {
    let path = defaults_folder(output).join("custom_model_root.json");
    let root = if path.is_file() { normalize_model_root(&text_or_empty(read_object(&path).get("models_root"))) } else { String::new() };
    json!({"models_root": root, "path": path_text(&path), "exists": !root.is_empty() && Path::new(&root).is_dir(), "registered": []})
}

pub fn save_custom_model_root(output: &Path, value: &str) -> std::io::Result<Value> {
    let root = normalize_model_root(value);
    atomic_write_json(&defaults_folder(output).join("custom_model_root.json"), &json!({"models_root": root}))?;
    Ok(load_custom_model_root(output))
}

// -- named profiles -------------------------------------------------------------------------------------------------------

const MAX_PROFILE_NAME_LENGTH: usize = 60;
const LAST_SELECTED_FILE: &str = "_last_selected.json";
const DEFAULT_LAYOUT_FILE: &str = "_default_layout.json";

/// `clean_profile_name`: whitespace collapsed, not empty and not longer than 60 characters.
pub fn clean_profile_name(value: &str) -> Stored<String> {
    let name = collapse_ws(value);
    if name.is_empty() {
        return invalid("Profile name is empty.");
    }
    if name.chars().count() > MAX_PROFILE_NAME_LENGTH {
        return invalid(format!("Profile name is longer than {MAX_PROFILE_NAME_LENGTH} characters."));
    }
    Ok(name)
}

/// `re.sub(r"[^A-Za-z0-9_. -]+", "_", name).strip(" ._").lower()`.
fn sanitize(name: &str) -> String {
    let mut out = String::new();
    let mut in_run = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ' ' | '-') {
            out.push(c);
            in_run = false;
        } else if !in_run {
            out.push('_');
            in_run = true;
        }
    }
    out.trim_matches([' ', '.', '_']).to_lowercase()
}

/// The file name a profile name is stored under: lower case, odd characters replaced by `_`.
fn file_stem(name: &str) -> String {
    let stem = sanitize(name);
    if stem.is_empty() {
        "profile".into()
    } else {
        stem
    }
}

fn name_of(value: &Value) -> Option<String> {
    let name = text_or_empty(value.get("name"));
    (!name.trim().is_empty()).then_some(name)
}

fn text_value(value: &Value, key: &str) -> String {
    text_or_empty(value.get(key))
}

fn saved_at_now(now: &Stamp) -> String {
    now.iso.clone()
}

// Video profiles -------------------------------------------------------------------------------------------------------

/// Settings a video profile never carries (the audio mode, pipeline, continuity and bookkeeping keys).
const EXCLUDED_PROFILE_KEYS: [&str; 12] = [
    "audio_mode",
    "pipeline",
    "continuity_mode",
    "continuity_prompt_from_last_frame",
    "latent_context_frames",
    "location_transition_preset",
    "location_transition_custom",
    "ref_pass_profiles",
    "i2v_pass_profiles",
    "ref_pass_mode",
    "two_pass_defaults_version",
    "advanced_two_pass_defaults_version",
];

pub struct VideoProfiles {
    root: PathBuf,
}

impl VideoProfiles {
    pub fn new(output: &Path) -> Self {
        Self { root: output.join("VRGDG_Video_Profiles").join("minimax_h3") }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.join(format!("{}.json", file_stem(name)))
    }

    fn read(&self, path: &Path) -> Option<Value> {
        let data = read_json(path)?;
        (data.get("settings").is_some_and(Value::is_object) && name_of(&data).is_some()).then_some(data)
    }

    fn filter(settings: &Value) -> Obj {
        let mut normalized = h3_core::settings::normalize_settings(Some(settings));
        normalized.retain(|key, _| !EXCLUDED_PROFILE_KEYS.contains(&key.as_str()));
        normalized
    }

    pub fn list(&self) -> Vec<Value> {
        let mut profiles: Vec<Value> = json_files(&self.root)
            .iter()
            .filter_map(|p| self.read(p))
            .map(|data| {
                let settings = &data["settings"];
                json!({
                    "name": text_value(&data, "name"),
                    "saved_at": text_value(&data, "saved_at"),
                    "video_mode": text_value(settings, "video_mode"),
                    "render_pass": text_value(settings, "render_pass"),
                })
            })
            .collect();
        profiles.sort_by_key(|p| text_value(p, "name").to_lowercase());
        profiles
    }

    pub fn load(&self, name: &str) -> Stored<Value> {
        let display = clean_profile_name(name)?;
        let data = self
            .read(&self.path(&display))
            .filter(|d| text_value(d, "name").trim().to_lowercase() == display.to_lowercase())
            .ok_or_else(|| StoreError::NotFound(format!("Video profile '{display}' was not found.")))?;
        Ok(json!({"name": text_value(&data, "name"), "saved_at": text_value(&data, "saved_at"), "settings": Self::filter(&data["settings"])}))
    }

    pub fn save(&self, name: &str, settings: &Value, overwrite: bool, now: &Stamp) -> Stored<Value> {
        let display = clean_profile_name(name)?;
        if !settings.as_object().is_some_and(|s| !s.is_empty()) {
            return invalid("There are no video settings to save.");
        }
        let path = self.path(&display);
        if let Some(existing) = self.read(&path) {
            let existing_name = text_value(&existing, "name").trim().to_string();
            if existing_name.to_lowercase() != display.to_lowercase() {
                return invalid(format!("The name is too similar to the existing profile '{existing_name}'."));
            }
            if !overwrite {
                return Err(StoreError::Exists { message: format!("A video profile named '{existing_name}' already exists."), name: existing_name });
            }
        }
        let saved_at = saved_at_now(now);
        let filtered = Self::filter(settings);
        atomic_write_json(&path, &json!({"version": 1, "engine": "minimax_h3", "name": display, "saved_at": saved_at, "settings": filtered}))?;
        self.set_last(&display)?;
        eprintln!("[VRGDG-RS Video Profiles] Saved '{display}'");
        Ok(json!({"name": display, "saved_at": saved_at, "settings": filtered}))
    }

    /// Name chosen last, when that profile still exists.
    pub fn last(&self) -> String {
        let name = text_or_empty(read_object(&self.root.join(LAST_SELECTED_FILE)).get("name")).trim().to_string();
        if name.is_empty() {
            return String::new();
        }
        match self.read(&self.path(&name)) {
            Some(data) if text_value(&data, "name").trim().to_lowercase() == name.to_lowercase() => text_value(&data, "name"),
            _ => String::new(),
        }
    }

    pub fn set_last(&self, name: &str) -> Stored<String> {
        let name = name.trim();
        let clean = if name.is_empty() { String::new() } else { text_value(&self.load(name)?, "name") };
        atomic_write_json(&self.root.join(LAST_SELECTED_FILE), &json!({"name": clean}))?;
        Ok(clean)
    }

    pub fn delete(&self, name: &str) -> Stored<Value> {
        let display = clean_profile_name(name)?;
        let path = self.path(&display);
        let data = self
            .read(&path)
            .filter(|d| text_value(d, "name").trim().to_lowercase() == display.to_lowercase())
            .ok_or_else(|| StoreError::NotFound(format!("Video profile '{display}' was not found.")))?;
        fs::remove_file(&path)?;
        eprintln!("[VRGDG-RS Video Profiles] Deleted '{}'", text_value(&data, "name"));
        Ok(json!({"name": text_value(&data, "name")}))
    }
}

// UI profiles ----------------------------------------------------------------------------------------------------------

const PANEL_LIMITS: [(&str, i64, i64, i64); 5] = [
    ("left_panel_width", 180, 520, 260),
    ("right_panel_width", 280, 720, 360),
    ("timeline_panel_height", 190, 4000, 300),
    ("llm_popout_width", 300, 1600, 460),
    ("llm_popout_height", 200, 2000, 460),
];
const POSITION_LIMITS: [(&str, i64, i64); 2] = [("llm_popout_x", -2000, 10000), ("llm_popout_y", -200, 10000)];
const FLAG_KEYS: [&str; 3] = ["left_collapsed", "right_collapsed", "llm_popout_open"];

/// `normalize_layout`: every key present and every size clamped.
pub fn normalize_layout(layout: Option<&Value>) -> Obj {
    let empty = Map::new();
    let source = layout.and_then(Value::as_object).unwrap_or(&empty);
    let mut result = Map::new();
    for key in FLAG_KEYS {
        result.insert(key.into(), json!(source.get(key).is_some_and(truthy)));
    }
    for (key, low, high, default) in PANEL_LIMITS {
        let value = source.get(key).and_then(to_float).filter(|f| f.is_finite()).map_or(default, h3_core::pyval::round_half_even);
        result.insert(key.into(), json!(value.clamp(low, high)));
    }
    for (key, low, high) in POSITION_LIMITS {
        let value = match source.get(key) {
            None | Some(Value::Null) | Some(Value::Bool(_)) => Value::Null,
            Some(raw) => to_float(raw).filter(|f| f.is_finite()).map_or(Value::Null, |f| json!(h3_core::pyval::round_half_even(f).clamp(low, high))),
        };
        result.insert(key.into(), value);
    }
    result
}

pub struct UiProfiles {
    root: PathBuf,
}

impl UiProfiles {
    pub fn new(output: &Path) -> Self {
        Self { root: output.join("VRGDG_UI_Profiles").join("music_video_builder") }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.join(format!("{}.json", file_stem(name)))
    }

    fn read(&self, path: &Path) -> Option<Value> {
        let data = read_json(path)?;
        (data.get("layout").is_some_and(Value::is_object) && name_of(&data).is_some()).then_some(data)
    }

    fn find(&self, name: &str) -> Stored<Value> {
        let display = clean_profile_name(name)?;
        self.read(&self.path(&display))
            .filter(|d| text_value(d, "name").trim().to_lowercase() == display.to_lowercase())
            .ok_or_else(|| StoreError::NotFound(format!("UI profile '{display}' was not found.")))
    }

    pub fn list(&self) -> Vec<Value> {
        let mut profiles: Vec<Value> = json_files(&self.root)
            .iter()
            .filter_map(|p| self.read(p))
            .map(|d| json!({"name": text_value(&d, "name"), "saved_at": text_value(&d, "saved_at")}))
            .collect();
        profiles.sort_by_key(|p| text_value(p, "name").to_lowercase());
        profiles
    }

    pub fn load(&self, name: &str) -> Stored<Value> {
        let data = self.find(name)?;
        Ok(json!({"name": text_value(&data, "name"), "saved_at": text_value(&data, "saved_at"), "layout": normalize_layout(data.get("layout"))}))
    }

    pub fn save(&self, name: &str, layout: Option<&Value>, overwrite: bool, now: &Stamp) -> Stored<Value> {
        let display = clean_profile_name(name)?;
        let path = self.path(&display);
        if let Some(existing) = self.read(&path) {
            let existing_name = text_value(&existing, "name").trim().to_string();
            if existing_name.to_lowercase() != display.to_lowercase() {
                return invalid(format!("The name is too similar to the existing profile '{existing_name}'."));
            }
            if !overwrite {
                return Err(StoreError::Exists { message: format!("A UI profile named '{existing_name}' already exists."), name: existing_name });
            }
        }
        let saved_at = saved_at_now(now);
        let normalized = normalize_layout(layout);
        atomic_write_json(&path, &json!({"version": 1, "name": display, "saved_at": saved_at, "layout": normalized}))?;
        self.set_last(&display)?;
        eprintln!("[VRGDG-RS UI Profiles] Saved '{display}'");
        Ok(json!({"name": display, "saved_at": saved_at, "layout": normalized}))
    }

    /// Replace the layout of an existing profile.
    pub fn update_layout(&self, name: &str, layout: Option<&Value>, now: &Stamp) -> Stored<Value> {
        let data = self.find(name)?;
        self.save(&text_value(&data, "name"), layout, true, now)
    }

    pub fn last(&self) -> String {
        let name = text_or_empty(read_object(&self.root.join(LAST_SELECTED_FILE)).get("name")).trim().to_string();
        if name.is_empty() {
            return String::new();
        }
        match self.read(&self.path(&name)) {
            Some(data) if text_value(&data, "name").trim().to_lowercase() == name.to_lowercase() => text_value(&data, "name"),
            _ => String::new(),
        }
    }

    pub fn set_last(&self, name: &str) -> Stored<String> {
        let name = name.trim();
        let clean = if name.is_empty() { String::new() } else { text_value(&self.load(name)?, "name") };
        atomic_write_json(&self.root.join(LAST_SELECTED_FILE), &json!({"name": clean}))?;
        Ok(clean)
    }

    /// The layout kept for "Default layout" (no profile selected), or `None` when it was never saved.
    pub fn default_layout(&self) -> Option<Value> {
        let data = read_json(&self.root.join(DEFAULT_LAYOUT_FILE))?;
        data.get("layout").filter(|l| l.is_object()).map(|l| Value::Object(normalize_layout(Some(l))))
    }

    pub fn save_default_layout(&self, layout: Option<&Value>) -> Stored<Value> {
        let normalized = normalize_layout(layout);
        atomic_write_json(&self.root.join(DEFAULT_LAYOUT_FILE), &json!({"version": 1, "layout": normalized}))?;
        Ok(Value::Object(normalized))
    }

    pub fn delete(&self, name: &str) -> Stored<Value> {
        let data = self.find(name)?;
        fs::remove_file(self.path(&text_value(&data, "name")))?;
        eprintln!("[VRGDG-RS UI Profiles] Deleted '{}'", text_value(&data, "name"));
        Ok(json!({"name": text_value(&data, "name")}))
    }
}

// Scene Defaults presets -----------------------------------------------------------------------------------------------

const SCENE_TEXT_KEYS: [&str; 10] = [
    "cameraFlow",
    "performanceStyle",
    "facialPerformance",
    "facialPerformanceCustom",
    "globalConsistencyPhrase",
    "storyArcDetail",
    "imageAesthetic",
    "imageShotFlow",
    "imageWorldStyle",
    "imageCustomStyle",
];
const SCENE_SPEED_KEYS: [&str; 3] = ["cameraMotionSpeed", "characterMotionSpeed", "cutFrequency"];

pub struct SceneDefaultsPresets {
    root: PathBuf,
}

impl SceneDefaultsPresets {
    pub fn new(output: &Path) -> Self {
        Self { root: output.join("VRGDG_Video_Profiles").join("scene_defaults") }
    }

    fn path(&self, name: &str) -> PathBuf {
        let mut stem = sanitize(name);
        if stem.is_empty() {
            stem = "preset".into();
        }
        // Windows device names are not valid file names, even with an extension.
        let head = stem.split('.').next().unwrap_or("").to_uppercase();
        let numbered = |prefix: &str| head.strip_prefix(prefix).is_some_and(|d| d.len() == 1 && matches!(d.as_bytes()[0], b'1'..=b'9'));
        if ["CON", "PRN", "AUX", "NUL"].contains(&head.as_str()) || numbered("COM") || numbered("LPT") {
            stem = format!("preset_{stem}");
        }
        self.root.join(format!("{stem}.json"))
    }

    /// Validate Scene Defaults settings the way the Builder expects, and drop everything else.
    pub fn filter(settings: &Value) -> Stored<Value> {
        let Some(settings) = settings.as_object() else { return invalid("Scene Defaults settings must be an object.") };
        let mut result = Map::new();
        for (key, value) in [("videoStyle", ""), ("temporalWorldEffect", ""), ("fxPreset", "")] {
            result.insert(key.into(), json!(value));
        }
        // Python iterates a set here, so the order of these keys is not defined. The sorted order is stable.
        let mut text_keys: Vec<&str> = SCENE_TEXT_KEYS.to_vec();
        text_keys.sort_unstable();
        for key in text_keys {
            let Some(value) = settings.get(key) else { continue };
            match value.as_str() {
                Some(text) if text.chars().count() <= 20000 => result.insert(key.into(), json!(text)),
                _ => return invalid(format!("Invalid Scene Defaults value: {key}.")),
            };
        }
        for key in SCENE_SPEED_KEYS {
            let Some(raw) = settings.get(key) else { continue };
            let bad = || StoreError::Invalid(format!("{key} must be a whole number from 0 to 10."));
            if raw.is_boolean() {
                return Err(bad());
            }
            let value = to_float(raw).filter(|f| f.is_finite() && f.fract() == 0.0 && (0.0..=10.0).contains(f)).ok_or_else(bad)?;
            result.insert(key.into(), json!(value as i64));
        }
        if let Some(sequence) = settings.get("customCameraFlowSequence") {
            let Some(items) = sequence.as_array().filter(|a| a.len() <= 1000) else { return invalid("Invalid custom camera flow sequence.") };
            let mut clean = Vec::new();
            for item in items {
                let Some(item) = item.as_object() else { return invalid("Invalid custom camera flow entry.") };
                let mut row = Map::new();
                for key in ["shot", "camera"] {
                    if let Some(value) = item.get(key) {
                        match value.as_str() {
                            Some(text) if text.chars().count() <= 20000 => row.insert(key.into(), value.clone()),
                            _ => return invalid("Invalid custom camera flow entry."),
                        };
                    }
                }
                if row.get("shot").and_then(Value::as_str).unwrap_or("").trim().is_empty() {
                    return invalid("Custom camera flow entries need a shot description.");
                }
                clean.push(Value::Object(row));
            }
            result.insert("customCameraFlowSequence".into(), Value::Array(clean));
        }
        let custom_flow = result.get("cameraFlow").and_then(Value::as_str) == Some("custom");
        if custom_flow && !result.get("customCameraFlowSequence").is_some_and(truthy) {
            return invalid("Custom camera flow needs at least one saved shot.");
        }
        Ok(Value::Object(result))
    }

    pub fn load(&self, name: &str) -> Stored<Value> {
        let name = clean_profile_name(name)?;
        let path = self.path(&name);
        let Some(preset) = read_json(&path) else { return Err(StoreError::NotFound(format!("Scene Defaults preset '{name}' was not found."))) };
        let settings = Self::filter(preset.get("settings").unwrap_or(&Value::Null))?;
        Ok(json!({"name": clean_profile_name(&text_value(&preset, "name"))?, "settings": settings}))
    }

    pub fn list(&self) -> Vec<Value> {
        let mut presets: Vec<Value> = json_files(&self.root)
            .iter()
            .filter_map(|file| {
                let preset = read_json(file)?;
                let name = clean_profile_name(preset.get("name").and_then(Value::as_str)?).ok()?;
                Self::filter(preset.get("settings")?).ok()?;
                let expected = self.path(&name);
                (expected.to_string_lossy().to_lowercase() == file.to_string_lossy().to_lowercase()).then(|| json!({"name": name}))
            })
            .collect();
        presets.sort_by_key(|p| text_value(p, "name").to_lowercase());
        presets
    }

    pub fn save(&self, name: &str, settings: &Value, overwrite: bool) -> Stored<Value> {
        let name = clean_profile_name(name)?;
        let preset = json!({"version": 1, "name": name, "settings": Self::filter(settings)?});
        let path = self.path(&name);
        if path.exists() {
            let existing = self.load(&name)?;
            if text_value(&existing, "name").to_lowercase() != name.to_lowercase() {
                return invalid(format!("Name is too similar to '{}'.", text_value(&existing, "name")));
            }
            if !overwrite {
                return Err(StoreError::Exists { message: format!("A Scene Defaults preset named '{name}' already exists."), name });
            }
        }
        atomic_write_json(&path, &preset)?;
        Ok(preset)
    }
}

// Adjust presets -------------------------------------------------------------------------------------------------------

const ADJUST_FIELDS: [(&str, f64, f64); 13] = [
    ("temperature", -100.0, 100.0),
    ("tint", -100.0, 100.0),
    ("saturation", -100.0, 100.0),
    ("exposure", -100.0, 100.0),
    ("contrast", -100.0, 100.0),
    ("highlights", -100.0, 100.0),
    ("shadows", -100.0, 100.0),
    ("whites", -100.0, 100.0),
    ("blacks", -100.0, 100.0),
    ("sharpen", 0.0, 100.0),
    ("clarity", -100.0, 100.0),
    ("vignette", 0.0, 100.0),
    ("fade", 0.0, 100.0),
];

/// `_normalize_adjust_settings`: every slider present and inside its range.
pub fn normalize_adjust_settings(settings: Option<&Value>) -> Obj {
    let empty = Map::new();
    let source = settings.and_then(Value::as_object).unwrap_or(&empty);
    let mut normalized = Map::new();
    normalized.insert("enabled".into(), json!(source.get("enabled").is_none_or(|v| v != &json!(false))));
    for (key, low, high) in ADJUST_FIELDS {
        let value = source.get(key).and_then(to_float).filter(|f| !f.is_nan()).unwrap_or(0.0);
        normalized.insert(key.into(), json!(value.clamp(low, high)));
    }
    normalized
}

fn adjust_preset_name(name: &str) -> Stored<String> {
    let raw = name.trim().trim_matches('"');
    if raw.is_empty() {
        return invalid("Preset name is required.");
    }
    let base = raw.rsplit(['\\', '/']).next().unwrap_or(raw);
    let stem = match base.rfind('.') {
        Some(i) if i > 0 => &base[..i],
        _ => base,
    };
    let replaced: String = stem.chars().map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | ' ') { c } else { '_' }).collect();
    let safe = replaced.trim_matches([' ', '.', '_']).split_whitespace().collect::<Vec<_>>().join("_");
    if safe.is_empty() {
        return invalid("Preset name is invalid.");
    }
    Ok(safe.chars().take(80).collect())
}

pub struct AdjustPresets {
    root: PathBuf,
}

impl AdjustPresets {
    pub fn new(output: &Path) -> Self {
        Self { root: output.join("VRGDG_AdjustPresets") }
    }

    /// `GET .../adjust/presets`.
    pub fn list(&self) -> Value {
        let _ = fs::create_dir_all(&self.root);
        let mut files = json_files(&self.root);
        files.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default());
        let mut presets = Vec::new();
        for path in files.into_iter().filter(|p| p.is_file()) {
            let Some(Value::Object(data)) = read_json(&path) else { continue };
            let key = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let settings = normalize_adjust_settings(data.get("adjust").filter(|v| truthy(v)).or(data.get("settings")));
            let label = text_or_empty(data.get("name")).trim().to_string();
            presets.push(json!({
                "name": key,
                "label": if label.is_empty() { key.clone() } else { label },
                "settings": settings,
                "path": path_text(&path),
                "modified": modified_seconds(&path),
                "legacy": false,
            }));
        }
        json!({"presets": presets, "presets_dir": path_text(&self.root), "legacy_presets_dir": ""})
    }

    pub fn save(&self, name: &str, settings: Option<&Value>, now: &Stamp) -> Stored<Value> {
        let safe = adjust_preset_name(name)?;
        let path = self.root.join(format!("{safe}.json"));
        let normalized = normalize_adjust_settings(settings);
        let label = if name.trim().is_empty() { safe.clone() } else { name.trim().to_string() };
        let payload = json!({"type": "vrgdg_adjust_preset", "version": 1, "name": label, "adjust": normalized, "saved_at": now.epoch});
        atomic_write_text(&path, &pyjson::dumps(&payload))?;
        Ok(json!({"name": safe, "label": label, "settings": normalized, "path": path_text(&path), "modified": modified_seconds(&path)}))
    }

    pub fn import(&self, preset: Option<&Value>, fallback: &str, now: &Stamp) -> Stored<Value> {
        let empty = Value::Object(Map::new());
        let data = preset.filter(|v| v.is_object()).unwrap_or(&empty);
        let fallback = if fallback.trim().is_empty() { "Imported Adjust Preset" } else { fallback };
        let name = match text_or_empty(data.get("name")).trim() {
            "" => fallback.trim().to_string(),
            name => name.to_string(),
        };
        let settings = data.get("adjust").filter(|v| truthy(v)).or(data.get("settings").filter(|v| truthy(v))).unwrap_or(data);
        self.save(&name, Some(settings), now)
    }
}

fn modified_seconds(path: &Path) -> f64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0.0, |d| d.as_secs_f64())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp() -> Stamp {
        Stamp { compact: "20260101_000000".into(), iso: "2026-01-01T00:00:00".into(), epoch: 1_767_225_600.0 }
    }

    #[test]
    fn profile_names_are_cleaned_and_stored_lowercase() {
        assert_eq!(clean_profile_name("  My   Look ").unwrap(), "My Look");
        assert!(clean_profile_name("   ").is_err());
        assert!(clean_profile_name(&"x".repeat(61)).is_err());
        assert_eq!(file_stem("My Look!?"), "my look");
        assert_eq!(file_stem("..."), "profile");
    }

    #[test]
    fn ui_profile_round_trip_with_last_selected() {
        let dir = tempfile::tempdir().unwrap();
        let profiles = UiProfiles::new(dir.path());
        assert_eq!(profiles.last(), "");
        let saved = profiles.save("Wide", Some(&json!({"left_collapsed": true, "left_panel_width": 9999, "llm_popout_x": 12.5})), false, &stamp()).unwrap();
        assert_eq!(saved["layout"]["left_panel_width"], 520);
        assert_eq!(saved["layout"]["llm_popout_x"], 12);
        assert_eq!(saved["layout"]["right_panel_width"], 360);
        assert_eq!(profiles.last(), "Wide");
        assert!(matches!(profiles.save("wide", None, false, &stamp()), Err(StoreError::Exists { .. })));
        assert!(profiles.save("WIDE", None, true, &stamp()).is_ok());
        assert_eq!(profiles.list().len(), 1);
        assert_eq!(profiles.load("Wide").unwrap()["layout"]["left_collapsed"], false);
        profiles.delete("Wide").unwrap();
        assert_eq!(profiles.last(), "");
        assert!(matches!(profiles.load("Wide"), Err(StoreError::NotFound(_))));
    }

    #[test]
    fn video_profile_drops_excluded_keys() {
        let dir = tempfile::tempdir().unwrap();
        let profiles = VideoProfiles::new(dir.path());
        let saved = profiles.save("Fast", &json!({"video_mode": "text_to_video", "pipeline": "standard", "audio_mode": "x"}), false, &stamp()).unwrap();
        assert!(saved["settings"].get("pipeline").is_none());
        assert!(saved["settings"].get("audio_mode").is_none());
        assert_eq!(profiles.list()[0]["video_mode"], "text_to_video");
        assert_eq!(profiles.last(), "Fast");
        assert!(profiles.save("Empty", &json!({}), false, &stamp()).is_err());
    }

    #[test]
    fn global_llm_keys_clear_and_merge() {
        let dir = tempfile::tempdir().unwrap();
        save_global_llm_keys(dir.path(), Some(&json!({"llm_api_key": " abc ", "own_server_api_key": "k2", "other": "ignored"}))).unwrap();
        assert_eq!(load_global_llm_keys(dir.path()), json!({"llm_api_key": "abc", "own_server_api_key": "k2"}).as_object().cloned().unwrap());
        save_global_llm_keys(dir.path(), Some(&json!({"llm_api_key": ""}))).unwrap();
        assert!(load_global_llm_keys(dir.path()).get("llm_api_key").is_none());
        assert_eq!(load_global_llm_keys(dir.path())["own_server_api_key"], "k2");
    }

    #[test]
    fn model_defaults_scrub_the_image_source() {
        let dir = tempfile::tempdir().unwrap();
        let session = json!({"zimage_settings": {"use_image_to_image": true, "image_to_image_path": "C:/a.png"}, "image_model_mode": "zimage", "unrelated": 1});
        save_model_defaults(dir.path(), session.as_object().unwrap(), &stamp()).unwrap();
        let loaded = load_model_defaults(dir.path()).unwrap();
        assert_eq!(loaded["defaults"]["zimage_settings"]["use_image_to_image"], false);
        assert_eq!(loaded["defaults"]["zimage_settings"]["image_to_image_path"], "");
        assert!(loaded["defaults"].get("unrelated").is_none());
        assert_eq!(loaded["saved_at"], "2026-01-01 00:00:00");
    }

    #[test]
    fn scene_defaults_validation() {
        assert!(SceneDefaultsPresets::filter(&json!({"cameraMotionSpeed": 11})).is_err());
        assert!(SceneDefaultsPresets::filter(&json!({"cameraMotionSpeed": true})).is_err());
        assert!(SceneDefaultsPresets::filter(&json!({"cameraFlow": "custom"})).is_err());
        let ok = SceneDefaultsPresets::filter(&json!({"cameraMotionSpeed": 3.0, "performanceStyle": "calm", "junk": 1})).unwrap();
        assert_eq!(ok["cameraMotionSpeed"], 3);
        assert!(ok.get("junk").is_none());
        assert_eq!(ok["videoStyle"], "");
        let dir = tempfile::tempdir().unwrap();
        let presets = SceneDefaultsPresets::new(dir.path());
        presets.save("CON", &json!({}), false).unwrap();
        assert!(presets.path("CON").ends_with("preset_con.json"));
        assert!(matches!(presets.save("con", &json!({}), false), Err(StoreError::Exists { .. })));
        assert_eq!(presets.list().len(), 1);
    }

    #[test]
    fn adjust_presets_clamp_and_list() {
        let dir = tempfile::tempdir().unwrap();
        let presets = AdjustPresets::new(dir.path());
        let saved = presets.save("Warm Look!", Some(&json!({"temperature": 250, "sharpen": -5, "enabled": false})), &stamp()).unwrap();
        assert_eq!(saved["name"], "Warm_Look");
        assert_eq!(saved["settings"]["temperature"], 100.0);
        assert_eq!(saved["settings"]["sharpen"], 0.0);
        assert_eq!(saved["settings"]["enabled"], false);
        let listed = presets.list();
        assert_eq!(listed["presets"].as_array().unwrap().len(), 1);
        assert_eq!(listed["presets"][0]["label"], "Warm Look!");
        let imported = presets.import(Some(&json!({"name": "From Friend", "adjust": {"tint": 5}})), "x", &stamp()).unwrap();
        assert_eq!(imported["settings"]["tint"], 5.0);
    }
}
