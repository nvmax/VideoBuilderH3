//! Seedance 2.5 Reference to Video: the prompt backup made before a conversion, the scene's audio excerpt and the ComfyUI graph.
//! Ports of `backup_seedance_prompts` and `load_backup_scene` (`llm/seedance_conversion.py`) and of `_prepare_seedance_scene_audio` and
//! `_build_seedance25_reference_prompt` (`runner/api_video_workflows.py`). What the prompt becomes is in `h3_core::seedance`.

use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine;
use h3_core::pyval::{py_str, text_or_empty, to_float, truthy, Obj};
use h3_core::seedance::scene_audio_prompt;
use h3_media::Tools;
use serde_json::{json, Value};

use crate::payload::path_text;

type Reply = Result<Value, String>;

/// The scene keys a backup keeps.
const BACKUP_KEYS: [&str; 21] = [
    "id", "label", "start", "end", "source_key", "source_prompt", "minimax_h3_prompt", "i2v_prompt", "api_video_prompt", "api_video_settings",
    "api_video_image_paths", "api_video_image_references", "api_video_video_paths", "api_video_audio_paths", "api_video_prompt_reference_binding",
    "image_references", "reference_aliases", "video_paths", "audio_paths", "original_soundtrack", "use_scene_audio",
];

const MAX_PROMPT: usize = 15000;
/// Seedance makes whole seconds, at least this many.
const MIN_GENERATION_SECONDS: f64 = 4.0;
/// A Seedance scene is at most this long.
const MAX_SCENE_SECONDS: f64 = 30.0;

fn absolute(path: &Path) -> PathBuf {
    h3_core::abspath::absolute(path).unwrap_or_else(|_| path.to_path_buf()).components().collect()
}

/// `project_context/prompt_backups` of an existing project.
fn backup_folder(project: Option<&Value>) -> Result<PathBuf, String> {
    let given = text_or_empty(project);
    let root = absolute(Path::new(given.trim().trim_matches('"')));
    if given.trim().is_empty() || !root.is_dir() {
        return Err("Create or load an existing project before converting prompts.".into());
    }
    Ok(root.join("project_context").join("prompt_backups"))
}

/// `backup_seedance_prompts`: keep the prompts and references as they are before the LLM touches anything, in a file of their own.
/// Nothing is ever written over an earlier backup.
pub fn backup_prompts(payload: &Obj) -> Reply {
    let folder = backup_folder(payload.get("project_folder"))?;
    let scenes = payload.get("scenes").and_then(Value::as_array).filter(|scenes| !scenes.is_empty()).ok_or("No scene prompts were supplied for backup.")?;
    let mut clean: Vec<Value> = Vec::new();
    let mut seen: Vec<&Value> = Vec::new();
    for scene in scenes {
        let scene = scene.as_object().filter(|scene| scene.get("id").is_some_and(truthy)).ok_or("Every backup scene needs a unique ID.")?;
        let id = &scene["id"];
        if seen.contains(&id) {
            return Err("Every backup scene needs a unique ID.".into());
        }
        if text_or_empty(scene.get("source_prompt")).trim().is_empty() {
            return Err("A backup source prompt is empty.".into());
        }
        seen.push(id);
        clean.push(Value::Object(scene.iter().filter(|(key, _)| BACKUP_KEYS.contains(&key.as_str())).map(|(key, value)| (key.clone(), value.clone())).collect()));
    }
    let token = crate::assets::random_hex();
    let path = folder.join(format!("seedance_{token}.json"));
    fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    let document = json!({
        "schema_version": 1,
        "created_at": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, false),
        "video_engine": payload.get("video_engine").cloned().unwrap_or(Value::Null),
        "scenes": clean,
    });
    h3_project::atomic::atomic_write_json(&path, &document).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(json!({"backup_id": token, "backup_path": path.display().to_string(), "scene_count": clean.len()}))
}

/// `load_backup_scene`: the scene as it was saved before the conversion.
pub fn load_backup_scene(payload: &Obj) -> Result<Obj, String> {
    let token = text_or_empty(payload.get("backup_id"));
    if token.len() != 32 || !token.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')) {
        return Err("Save a prompt backup before converting.".into());
    }
    let path = backup_folder(payload.get("project_folder"))?.join(format!("seedance_{token}.json"));
    let text = fs::read_to_string(&path).map_err(|_| "The saved prompt backup was not found. Save a prompt backup before converting.".to_string())?;
    let backup: Value = h3_core::pyjson::parse(&text).map_err(|_| "The saved prompt backup could not be read.".to_string())?;
    let wanted = py_str(payload.get("scene_id"));
    backup["scenes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .find(|scene| py_str(scene.get("id")) == wanted)
        .cloned()
        .ok_or_else(|| "Scene was not found in the saved prompt backup.".to_string())
}

/// `float(payload.get(key, default))`: the default when the key is missing.
fn number(payload: &Obj, key: &str, default: f64) -> Result<f64, String> {
    match payload.get(key) {
        None => Ok(default),
        Some(value) => to_float(value).ok_or_else(|| format!("{key} must be a number.")),
    }
}

/// `_prepare_seedance_scene_audio`: this scene's part of the song, cut to its length and padded with silence to the whole seconds
/// Seedance makes, as a wav file in the project's `seedance_scene_audio` folder.
pub fn prepare_scene_audio(tools: &Tools, payload: &Obj) -> Reply {
    let source = absolute(Path::new(&path_text(payload, "source_path")));
    let project = path_text(payload, "project_folder");
    if !source.is_file() {
        return Err("Project/custom scene audio was not found. Load audio before using Seedance scene-audio mode.".into());
    }
    if project.is_empty() || !Path::new(&project).is_dir() {
        return Err("Set an existing project folder before preparing Seedance audio.".into());
    }
    let timing = "Invalid Seedance scene audio timing. Scenes must be longer than zero and at most 30 seconds.";
    let (start, duration) = (number(payload, "start", 0.0).map_err(|_| timing.to_string())?, number(payload, "duration", 0.0).map_err(|_| timing.to_string())?);
    if !start.is_finite() || start < 0.0 || !duration.is_finite() || !(duration > 0.0 && duration <= MAX_SCENE_SECONDS) {
        return Err(timing.into());
    }
    let source_text = source.display().to_string();
    let source_duration = h3_media::audio::probe_duration(tools, &source_text).map_err(|e| e.to_string())?;
    if source_duration <= 0.0 || start >= source_duration - 0.001 {
        return Err("The scene starts after its source audio ends. Check the scene's audio offset.".into());
    }
    let placement = "Invalid custom audio clip placement or duration.";
    let read_duration = number(payload, "source_duration", duration).map_err(|_| placement.to_string())?;
    let leading = number(payload, "leading_silence", 0.0).map_err(|_| placement.to_string())?;
    if !read_duration.is_finite() || read_duration <= 0.0 || read_duration > duration || !leading.is_finite() || !(0.0..duration).contains(&leading) {
        return Err(placement.into());
    }
    let generation = MIN_GENERATION_SECONDS.max(duration.ceil()) as i64;
    let folder = absolute(Path::new(&project)).join("seedance_scene_audio");
    fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    let path = folder.join(format!("scene_audio_{}.wav", crate::assets::random_hex()));
    let filter = format!("atrim=duration={read_duration:.9},asetpts=PTS-STARTPTS,adelay={:.6}:all=1,apad=whole_dur={generation}", leading * 1000.0);
    let args: Vec<String> = ["-y", "-ss", &format!("{start:.9}"), "-i", &source_text, "-vn", "-af", &filter, "-t", &format!("{generation}"), "-ac", "2", "-ar", "44100", "-c:a", "pcm_s16le", &path.display().to_string()]
        .iter()
        .map(|arg| arg.to_string())
        .collect();
    tools.ffmpeg_checked(&args, "Could not prepare Seedance scene audio.").map_err(|e| e.to_string())?;
    if !path.is_file() {
        return Err("Could not prepare Seedance scene audio.".into());
    }
    // An empty seek must fail before a paid API submission, instead of becoming all-silence guidance.
    let frames = hound::WavReader::open(&path).map(|reader| reader.duration()).map_err(|e| format!("Could not prepare Seedance scene audio: {e}"))?;
    if frames == 0 {
        return Err("The scene starts after its source audio ends. Check the scene's audio offset.".into());
    }
    Ok(json!({"audio_path": path.display().to_string(), "source_path": source_text, "start": start, "duration": duration, "generation_duration": generation}))
}

/// Copy one reference into ComfyUI's input folder under a name of its own, so references staged in the same moment never collide.
/// A picture may also arrive as a data URL.
fn stage_reference(item: &Value, kind: &str, input_dir: &Path) -> Result<String, String> {
    let item: Obj = match item {
        Value::String(path) => [("path".to_string(), json!(path))].into_iter().collect(),
        Value::Object(map) => map.clone(),
        other => return Err(format!("Seedance {kind} reference was not found: {}", py_str(Some(other)))),
    };
    let source = text_or_empty(item.get("path")).trim().trim_matches('"').to_string();
    if source.is_empty() && kind == "image" && item.get("data").is_some_and(truthy) {
        let raw = text_or_empty(item.get("data"));
        let (header, encoded) = match raw.strip_prefix("data:").and_then(|rest| rest.split_once(',')) {
            Some((header, encoded)) => (header.to_string(), encoded.to_string()),
            None => (String::new(), raw.clone()),
        };
        let extension = if header.contains("jpeg") {
            ".jpg"
        } else if header.contains("webp") {
            ".webp"
        } else {
            ".png"
        };
        let bytes = base64::engine::general_purpose::STANDARD.decode(encoded.trim()).map_err(|e| format!("Incorrect padding or bad image data: {e}"))?;
        let target = format!("vrgdg_seedance_{}{extension}", crate::assets::random_hex());
        fs::write(input_dir.join(&target), bytes).map_err(|e| format!("{}: {e}", input_dir.join(&target).display()))?;
        return Ok(target);
    }
    let path = Path::new(&source);
    if source.is_empty() || !path.is_file() {
        return Err(format!("Seedance {kind} reference was not found: {}", if source.is_empty() { "(empty)" } else { &source }));
    }
    let allowed: &[&str] = match kind {
        "image" => &[".png", ".jpg", ".jpeg", ".webp", ".bmp"],
        "video" => &[".mp4", ".mov", ".webm", ".mkv", ".m4v"],
        _ => &[".wav", ".mp3", ".flac", ".ogg", ".m4a", ".aac", ".mp4"],
    };
    let extension = path.extension().map(|e| format!(".{}", e.to_string_lossy().to_lowercase())).unwrap_or_default();
    if !allowed.contains(&extension.as_str()) {
        return Err(format!("Unsupported Seedance {kind} reference format: {extension}"));
    }
    let target = format!("vrgdg_seedance_{}{extension}", crate::assets::random_hex());
    fs::copy(path, input_dir.join(&target)).map_err(|e| format!("{}: {e}", input_dir.join(&target).display()))?;
    Ok(target)
}

/// `_build_seedance25_reference_prompt`: the ComfyUI graph that sends one scene's prompt and references to Seedance 2.5 and saves
/// the video. `has_node` says whether ComfyUI has a node class, `input_dir` and `output_root` are ComfyUI's input and output folders.
pub fn reference_prompt(has_node: &dyn Fn(&str) -> bool, input_dir: &Path, output_root: &str, payload: &Obj) -> Reply {
    let mut missing: Vec<&str> = ["ByteDance2ReferenceNodeV2", "SaveVideo"].into_iter().filter(|class| !has_node(class)).collect();
    missing.sort_unstable();
    if !missing.is_empty() {
        return Err(format!("Update ComfyUI and enable API nodes. Missing nodes: {}", missing.join(", ")));
    }
    let text = text_or_empty(payload.get("prompt")).trim().to_string();
    if text.is_empty() || text.chars().count() > MAX_PROMPT {
        return Err("Seedance needs a prompt between 1 and 15,000 characters.".into());
    }
    let too_long = "Seedance scenes must be longer than zero and at most 30 seconds. Split longer scenes first.";
    let duration = number(payload, "duration", 5.0).map_err(|_| too_long.to_string())?;
    if !duration.is_finite() || !(duration > 0.0 && duration <= MAX_SCENE_SECONDS) {
        return Err(too_long.into());
    }
    let resolution = payload.get("resolution").cloned().unwrap_or_else(|| json!("720p"));
    let ratio = payload.get("ratio").cloned().unwrap_or_else(|| json!("16:9"));
    if !["480p", "720p", "1080p"].iter().any(|allowed| resolution == json!(allowed)) {
        return Err("Unsupported Seedance resolution.".into());
    }
    if !["16:9", "4:3", "1:1", "3:4", "9:16", "21:9", "adaptive"].iter().any(|allowed| ratio == json!(allowed)) {
        return Err("Unsupported Seedance aspect ratio.".into());
    }
    let mut groups: Vec<(&str, &str, &str, Vec<Value>)> = Vec::new();
    for (kind, loader, field, limit) in [("image", "LoadImage", "image", 30), ("video", "LoadVideo", "file", 10), ("audio", "LoadAudio", "audio", 10)] {
        let items = match payload.get(&format!("reference_{kind}s")).filter(|value| truthy(value)) {
            None => Vec::new(),
            Some(Value::Array(items)) if items.len() <= limit => items.clone(),
            Some(_) => return Err(format!("Seedance accepts at most {limit} {kind} references.")),
        };
        groups.push((kind, loader, field, items));
    }
    if groups.iter().all(|(.., items)| items.is_empty()) {
        return Err("Reference to Video needs at least one image, video, or audio reference.".into());
    }
    for (_, loader, _, items) in &groups {
        if !items.is_empty() && !has_node(loader) {
            return Err(format!("ComfyUI is missing {loader}. Update ComfyUI first."));
        }
    }
    let text = scene_audio_prompt(&text, payload)?;
    if text.chars().count() > MAX_PROMPT {
        return Err("The prompt plus scene-audio timing instructions exceeds 15,000 characters. Shorten the prompt first.".into());
    }
    let seed = payload.get("seed").and_then(h3_core::pyval::to_int).unwrap_or(0).rem_euclid(2_147_483_648);
    let generation = MIN_GENERATION_SECONDS.max(duration.ceil()) as i64;
    let mut inputs = Obj::new();
    inputs.insert("model".into(), json!("Seedance 2.5"));
    inputs.insert("model.prompt".into(), json!(text));
    inputs.insert("model.resolution".into(), resolution);
    inputs.insert("model.ratio".into(), ratio);
    inputs.insert("model.duration".into(), json!(generation));
    inputs.insert("model.generate_audio".into(), json!(payload.get("generate_audio").map_or(true, truthy)));
    inputs.insert("model.task_type".into(), json!("reference"));
    inputs.insert("model.output_format".into(), json!("mp4"));
    inputs.insert("model.auto_downscale".into(), json!(true));
    inputs.insert("model.auto_upscale".into(), json!(false));
    inputs.insert("seed".into(), json!(seed));
    inputs.insert("watermark".into(), json!(false));
    let mut prompt = Obj::new();
    for (kind, loader, field, items) in &groups {
        for (index, item) in items.iter().enumerate() {
            let node_id = format!("{kind}_{}", index + 1);
            let staged = stage_reference(item, kind, input_dir)?;
            prompt.insert(node_id.clone(), json!({"class_type": loader, "inputs": {(field.to_string()): staged}}));
            inputs.insert(format!("model.reference_{kind}s.{kind}_{}", index + 1), json!([node_id, 0]));
        }
    }
    let prefix = format!("vrgdg_api_video/seedance25_{}/video", crate::assets::random_hex());
    prompt.insert("seedance".into(), json!({"class_type": "ByteDance2ReferenceNodeV2", "inputs": inputs}));
    prompt.insert("save_video".into(), json!({"class_type": "SaveVideo", "inputs": {"video": ["seedance", 0], "filename_prefix": prefix, "format": "mp4", "format.codec": "auto"}}));
    Ok(json!({"prompt": prompt, "output_root": output_root, "generation_duration": generation, "timeline_duration": duration}))
}
