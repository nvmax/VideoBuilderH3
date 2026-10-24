//! Copying, branching and rebasing projects and their assets. Port of `builder/project_copy.py`.
//!
//! "Save Project As" and "Branch Project" copy a project folder to a new one, leave out what the user did not keep,
//! and point every path in the session at the new folder. Export and import use the same rebasing.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use filetime::FileTime;
use h3_core::pyval::{text_or_empty, to_int, truthy, Obj};
use serde_json::{json, Value};

use crate::atomic::atomic_write_json;
use crate::images::unique_preview_path;
use crate::layout::{context_folder, images_folder, Stamp};
use crate::rehydrate::{absolute, is_inside_folder, is_internal_approved_image, relative_to};
use crate::session::assign_overlay_slots;

const KEEP_KEYS: [&str; 7] = ["lyrics", "notes", "prompts", "mappings", "images", "videos", "overlays"];
const VIDEO_DIRS: [&str; 3] = ["rendered_scene_videos", "rendered_scene_videos_backup", "scene_video_thumbnails"];
const IMAGE_DIRS: [&str; 2] = ["zimage_approved", "scene_image_previews"];
const NOTE_KEYS: [&str; 7] = ["timeline_note", "notes", "i2v_notes", "flux_notes", "nb_notes", "enhance_notes", "story_beat"];
const PROMPT_KEYS: [&str; 11] = [
    "t2i_prompt", "flux_prompt", "nb_prompt", "enhance_prompt", "i2v_prompt", "t2v_prompt", "flow_gpt_prompt", "ernie_t2i_prompt",
    "krea2_t2i_prompt", "minimax_h3_prompt", "minimax_h3_pass2_prompt",
];
const IMAGE_KEYS: [&str; 14] = [
    "custom_image_path", "custom_image_data", "custom_image_name", "approved_image_path", "image", "image_history", "image_history_index",
    "ref_image_path", "flux_subject_image_path", "flux_location_image_path", "image_output", "image_status",
    "minimax_h3_continuity_frame_path", "adjust_preview_image_path",
];
const VIDEO_KEYS: [&str; 20] = [
    "video_path", "video_folder", "video_thumbnail_path", "video_history", "video_thumbnail_history", "video_backup_paths",
    "video_backup_thumbnail_paths", "video_history_index", "video_output", "video_original_path", "video_original_thumbnail_path",
    "video_source_path", "minimax_h3_continuity_source_video_path", "minimax_h3_video_references", "minimax_h3_stage1_path",
    "minimax_h3_stage1_source_path", "minimax_h3_stage1_backup_path", "minimax_h3_stage2_path", "minimax_h3_stage2_source_path",
    "minimax_h3_stage2_backup_path",
];
const STAGE_PATH_KEYS: [&str; 6] = [
    "minimax_h3_stage1_path", "minimax_h3_stage1_source_path", "minimax_h3_stage1_backup_path", "minimax_h3_stage2_path",
    "minimax_h3_stage2_source_path", "minimax_h3_stage2_backup_path",
];
const MAPPING_KEYS: [&str; 7] = [
    "subject_ids", "location_id", "reference_subject_ids", "reference_location_id", "flux_image_ingredients", "nb_image_ingredients",
    "minimax_h3_reference_keys",
];
const AUDIO_KEYS: [&str; 2] = ["custom_audio_path", "custom_audio_name"];
const IMAGE_FILE_TYPES: [&str; 6] = ["png", "jpg", "jpeg", "webp", "gif", "bmp"];

// -- what a branch keeps --------------------------------------------------------------------------------------------------

/// What "Branch Project" keeps of the old project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Keep {
    pub lyrics: bool,
    pub notes: bool,
    pub prompts: bool,
    pub mappings: bool,
    pub images: bool,
    pub videos: bool,
    pub overlays: bool,
}

impl Keep {
    pub fn everything() -> Self {
        Self { lyrics: true, notes: true, prompts: true, mappings: true, images: true, videos: true, overlays: true }
    }

    /// `_normalize_branch_keep`: everything unless the request has a `keep` object, then only what it says.
    pub fn from_payload(payload: &Obj) -> Self {
        let Some(raw) = payload.get("keep").and_then(Value::as_object) else { return Self::everything() };
        let flag = |key: &str| raw.get(key).is_some_and(truthy);
        Self { lyrics: flag(KEEP_KEYS[0]), notes: flag(KEEP_KEYS[1]), prompts: flag(KEEP_KEYS[2]), mappings: flag(KEEP_KEYS[3]), images: flag(KEEP_KEYS[4]), videos: flag(KEEP_KEYS[5]), overlays: flag(KEEP_KEYS[6]) }
    }

    pub fn is_everything(&self) -> bool {
        *self == Self::everything()
    }
}

// -- file helpers ---------------------------------------------------------------------------------------------------------

/// `_looks_like_filesystem_path`: a drive path, a UNC path or a rooted path.
pub fn looks_like_filesystem_path(text: &str) -> bool {
    let value = text.trim().trim_matches('"');
    if value.is_empty() || value.contains('\n') || value.chars().count() > 4096 {
        return false;
    }
    let bytes = value.as_bytes();
    (bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/')) || value.starts_with("\\\\") || value.starts_with('/')
}

/// `shutil.copy2`: the file and its times.
pub fn copy2(source: &Path, target: &Path) -> io::Result<()> {
    fs::copy(source, target)?;
    if let Ok(meta) = fs::metadata(source) {
        let _ = filetime::set_file_times(target, FileTime::from_last_access_time(&meta), FileTime::from_last_modification_time(&meta));
    }
    Ok(())
}

fn text(path: &Path) -> String {
    path.display().to_string()
}

fn same_path(a: &Path, b: &Path) -> bool {
    let key = |p: &Path| if cfg!(windows) { text(p).replace('/', "\\").to_lowercase() } else { text(p) };
    key(a) == key(b)
}

/// `_copy_file_if_exists`: copy the file to `target` (made to be absolute) and return the target, or "" when the source is
/// not a file.
pub fn copy_file_if_exists(source: &str, target: &Path) -> io::Result<String> {
    let source = source.trim().trim_matches('"');
    if source.is_empty() || !Path::new(source).is_file() {
        return Ok(String::new());
    }
    let source = absolute(Path::new(source));
    let target = absolute(target);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    if same_path(&source, &target) {
        return Ok(text(&target));
    }
    copy2(&source, &target)?;
    Ok(text(&target))
}

fn stripped(value: Option<&Value>) -> String {
    text_or_empty(value).trim().to_string()
}

fn extension_of(path: &str) -> String {
    Path::new(path).extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default()
}

fn scene_image_path(project: &Path, scene: i64, extension: &str) -> PathBuf {
    let ext = extension.to_lowercase();
    let ext = if [".png", ".jpg", ".jpeg", ".webp"].contains(&ext.as_str()) { ext } else { ".png".to_string() };
    images_folder(project).join(format!("image_{:04}{ext}", scene.max(1)))
}

fn scene_audio_path(project: &Path, scene: i64, extension: &str) -> PathBuf {
    let ext = extension.to_lowercase();
    let ext = if [".wav", ".mp3", ".flac", ".m4a", ".ogg"].contains(&ext.as_str()) { ext } else { ".wav".to_string() };
    project.join("scene_audio").join(format!("audio_{:04}{ext}", scene.max(1)))
}

/// `re.sub(r"[^<allowed>]+", "_", text).strip("_")`.
fn squeeze(value: &str, allowed: fn(char) -> bool) -> String {
    let mut out = String::new();
    let mut pending = false;
    for c in value.chars() {
        if allowed(c) {
            out.push(c);
            pending = false;
        } else if !pending {
            out.push('_');
            pending = true;
        }
    }
    out.trim_matches('_').to_string()
}

fn key_chars(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

fn asset_key_chars(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')
}

// -- rewriting paths ------------------------------------------------------------------------------------------------------

/// `_rewrite_project_path_string`: a path under the old project folder moved under the new one. A file that exists in the
/// old folder but not in the new one is blanked.
fn rewrite_path_string(value: &str, old_folder: &str, new_folder: &str) -> String {
    let stripped = value.trim().trim_matches('"');
    if stripped.is_empty() || old_folder.is_empty() || new_folder.is_empty() || !looks_like_filesystem_path(stripped) {
        return value.to_string();
    }
    let (old_abs, new_abs, raw_abs) = (absolute(Path::new(old_folder)), absolute(Path::new(new_folder)), absolute(Path::new(stripped)));
    if !is_inside_folder(&raw_abs, &old_abs) {
        return value.to_string();
    }
    let rewritten = absolute(&new_abs.join(relative_to(&raw_abs, &old_abs)));
    if raw_abs.exists() && !rewritten.exists() {
        return String::new();
    }
    text(&rewritten)
}

/// `_rewrite_project_paths`.
pub fn rewrite_project_paths(value: &mut Value, old_folder: &str, new_folder: &str) {
    match value {
        Value::Object(map) => map.values_mut().for_each(|v| rewrite_project_paths(v, old_folder, new_folder)),
        Value::Array(items) => items.iter_mut().for_each(|v| rewrite_project_paths(v, old_folder, new_folder)),
        Value::String(s) => *s = rewrite_path_string(s, old_folder, new_folder),
        _ => {}
    }
}

/// A path under the old project folder is pointed at the same file in the new folder when that file is there. Unlike
/// [`rewrite_project_paths`] nothing is blanked: a file that exists only in the old folder keeps its path.
pub fn adopt_existing_project_files(value: &mut Value, old_folder: &str, new_folder: &str) {
    match value {
        Value::Object(map) => map.values_mut().for_each(|v| adopt_existing_project_files(v, old_folder, new_folder)),
        Value::Array(items) => items.iter_mut().for_each(|v| adopt_existing_project_files(v, old_folder, new_folder)),
        Value::String(s) => {
            let stripped = s.trim().trim_matches('"');
            if old_folder.is_empty() || !looks_like_filesystem_path(stripped) {
                return;
            }
            let (old_abs, raw_abs) = (absolute(Path::new(old_folder)), absolute(Path::new(stripped)));
            if is_inside_folder(&raw_abs, &old_abs) {
                let counterpart = absolute(&absolute(Path::new(new_folder)).join(relative_to(&raw_abs, &old_abs)));
                if counterpart.is_file() {
                    *s = text(&counterpart);
                }
            }
        }
        _ => {}
    }
}

/// `_clear_remaining_source_paths`: a path that still points into the old folder is blanked.
pub fn clear_remaining_source_paths(value: &mut Value, old_folder: &str) {
    if old_folder.is_empty() {
        return;
    }
    match value {
        Value::Object(map) => map.values_mut().for_each(|v| clear_remaining_source_paths(v, old_folder)),
        Value::Array(items) => items.iter_mut().for_each(|v| clear_remaining_source_paths(v, old_folder)),
        Value::String(s) if looks_like_filesystem_path(s) && is_inside_folder(&absolute(Path::new(s.trim().trim_matches('"'))), Path::new(old_folder)) => s.clear(),
        _ => {}
    }
}

fn rewrite_json_file(path: &Path, old_folder: &str, new_folder: &str) -> io::Result<()> {
    if !path.is_file() {
        return Ok(());
    }
    let bytes = fs::read(path)?;
    let source = String::from_utf8_lossy(&bytes);
    let mut data = h3_core::pyjson::parse(source.trim_start_matches('\u{feff}')).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    rewrite_project_paths(&mut data, old_folder, new_folder);
    clear_remaining_source_paths(&mut data, old_folder);
    atomic_write_json(path, &data)
}

/// `_rewrite_project_sidecar_json`: the storyboard, the reference descriptions and the prompt files hold paths too. A file
/// that cannot be read is left as it is.
pub fn rewrite_project_sidecar_json(project: &Path, old_folder: &str) {
    let mut files = vec![project.join("storyboard").join("storyboard.json"), project.join("subject_location").join("reference_descriptions.json")];
    if let Ok(entries) = fs::read_dir(project.join("prompts")) {
        files.extend(entries.flatten().filter(|e| e.file_name().to_string_lossy().to_lowercase().ends_with(".json")).map(|e| e.path()));
    }
    for path in files {
        let _ = rewrite_json_file(&path, old_folder, &text(project));
    }
}

/// `_rebase_project_owned_paths`: after a copy, import or export, every path into the old folder points into this one.
pub fn rebase_project_owned_paths(project: &Path, old_project: &str, session: &mut Value) {
    if old_project.is_empty() {
        return;
    }
    rewrite_project_paths(session, old_project, &text(project));
    clear_remaining_source_paths(session, old_project);
    rewrite_project_sidecar_json(project, old_project);
}

// -- copying the folder ---------------------------------------------------------------------------------------------------

/// `_is_project_root_export_video`: `FINAL_VIDEO...mp4` and `PREVIEW_SCENES_...mp4`.
fn is_root_export_video(name: &str) -> bool {
    let path = Path::new(name);
    if path.extension().map(|e| e.to_string_lossy().to_lowercase()) != Some("mp4".into()) {
        return false;
    }
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_uppercase()).unwrap_or_default();
    stem.starts_with("FINAL_VIDEO") || stem.starts_with("PREVIEW_SCENES_")
}

fn remove_root_export_videos(project: &Path) {
    for entry in fs::read_dir(project).into_iter().flatten().flatten() {
        if is_root_export_video(&entry.file_name().to_string_lossy()) && entry.path().is_file() {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// `_branch_copy_ignore`: which names of a folder a branch copy leaves out.
fn names_to_skip(keep: &Keep, source: &Path, directory: &Path, names: &[String]) -> HashSet<String> {
    let mut skip: HashSet<&str> = HashSet::from(["__pycache__", "session_backups"]);
    if !keep.videos {
        skip.extend(VIDEO_DIRS);
        skip.insert("render_logs");
    }
    if !keep.images {
        skip.extend(IMAGE_DIRS);
    }
    if !keep.mappings {
        skip.insert("subject_location");
    }
    if !keep.prompts {
        skip.insert("prompts");
    }
    if !keep.notes && !keep.prompts {
        skip.insert("storyboard");
    }
    let mut ignored: HashSet<String> = names.iter().filter(|n| skip.contains(n.as_str()) || n.ends_with(".tmp")).cloned().collect();
    let base = directory.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if same_path(&absolute(directory), &absolute(source)) {
        ignored.extend(names.iter().filter(|n| is_root_export_video(n)).cloned());
    }
    if !keep.mappings && base == "project_context" && names.iter().any(|n| n == "flux_references") {
        ignored.insert("flux_references".into());
    }
    if !keep.images && base == "storyboard" {
        ignored.extend(names.iter().filter(|n| IMAGE_FILE_TYPES.contains(&extension_of(n).trim_start_matches('.').to_lowercase().as_str())).cloned());
    }
    ignored
}

fn copy_tree(source: &Path, target: &Path, keep: &Keep, root: &Path) -> io::Result<()> {
    fs::create_dir_all(target)?;
    let names: Vec<String> = fs::read_dir(source)?.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    let ignored = names_to_skip(keep, root, source, &names);
    for name in names.iter().filter(|n| !ignored.contains(*n)) {
        let (from, to) = (source.join(name), target.join(name));
        if from.is_dir() {
            copy_tree(&from, &to, keep, root)?;
        } else {
            copy2(&from, &to)?;
        }
    }
    Ok(())
}

/// `_copy_project_tree`: copy the project folder, without what is not kept, the export videos in its root and the session
/// backups.
pub fn copy_project_tree(source: &Path, target: &Path, keep: &Keep) -> io::Result<()> {
    fs::create_dir_all(target)?;
    if !source.is_dir() {
        return Ok(());
    }
    copy_tree(source, target, keep, source)?;
    remove_root_export_videos(target);
    let _ = fs::remove_dir_all(target.join("session_backups"));
    Ok(())
}

// -- what a branch keeps in the session -----------------------------------------------------------------------------------

/// `_empty_keep_value`: the empty value of a field that is not kept, of the same kind as what it held.
fn empty_keep_value(key: &str, current: Option<&Value>) -> Value {
    if key.ends_with("_index") {
        return json!(-1);
    }
    if matches!(key, "image" | "image_output" | "video_output") {
        return Value::Null;
    }
    if key == "image_status" || key == "video_status" {
        return json!("none");
    }
    match current {
        Some(Value::Array(_)) => json!([]),
        Some(Value::Object(_)) => json!({}),
        Some(Value::Bool(_)) => json!(false),
        _ => json!(""),
    }
}

fn strip_segment(segment: &mut Value, keep: &Keep) {
    let Some(segment) = segment.as_object_mut() else { return };
    let clear = |keys: &[&str], segment: &mut Obj| {
        for key in keys {
            if segment.contains_key(*key) {
                let empty = empty_keep_value(key, segment.get(*key));
                segment.insert((*key).into(), empty);
            }
        }
    };
    if !keep.lyrics {
        segment.insert("lyric_text".into(), json!(""));
    }
    if !keep.notes {
        for key in NOTE_KEYS {
            if segment.contains_key(key) {
                segment.insert(key.into(), json!(""));
            }
        }
    }
    if !keep.prompts {
        for key in PROMPT_KEYS {
            if segment.contains_key(key) {
                segment.insert(key.into(), json!(""));
            }
        }
    }
    if !keep.mappings {
        clear(&MAPPING_KEYS, segment);
    }
    if !keep.images {
        clear(&IMAGE_KEYS, segment);
        segment.insert("preview_mode".into(), json!("image"));
    }
    if !keep.videos {
        clear(&VIDEO_KEYS, segment);
        segment.insert("video_status".into(), json!("none"));
        if segment.get("minimax_h3_video_references").is_some_and(Value::is_array) {
            segment.insert("minimax_h3_video_references".into(), json!([]));
        }
    }
    if !keep.images && !keep.videos {
        for key in AUDIO_KEYS {
            if segment.contains_key(key) {
                segment.insert(key.into(), json!(""));
            }
        }
    }
}

fn strip_segments(session: &Obj, key: &str, keep: &Keep) -> Value {
    let mut list = session.get(key).and_then(Value::as_array).cloned().unwrap_or_default();
    for item in &mut list {
        if !item.is_object() {
            *item = json!({});
        }
        strip_segment(item, keep);
    }
    Value::Array(list)
}

/// `_apply_branch_keep_to_session`: blank what the branch does not keep.
pub fn apply_branch_keep(session: &mut Obj, keep: &Keep) {
    if keep.is_everything() {
        return;
    }
    let segments = strip_segments(session, "segments", keep);
    session.insert("segments".into(), segments);
    if keep.overlays {
        let overlays = strip_segments(session, "overlay_segments", keep);
        session.insert("overlay_segments".into(), overlays);
    } else {
        session.insert("overlay_segments".into(), json!([]));
        if let Some(track) = session.get("overlay_track").and_then(Value::as_object).cloned() {
            let mut track = track;
            track.insert("enabled".into(), json!(false));
            session.insert("overlay_track".into(), Value::Object(track));
        }
    }
    if !keep.notes {
        session.insert("timeline_markers".into(), json!([]));
    }
    if !keep.prompts {
        for key in ["prompt_json_path", "i2v_motion_json_path", "theme_style_path", "story_idea_path", "subject_scene_path"] {
            session.insert(key.into(), json!(""));
        }
    }
    if !keep.mappings {
        session.insert("flux_reference_builder".into(), json!({}));
        session.insert("id_lora_reference_builder".into(), json!({}));
    }
    if !keep.images {
        session.insert("flux_global_image_ingredients".into(), json!([]));
        session.insert("use_flux_global_image_ingredients".into(), json!(false));
        session.insert("builder_agent_reference_images".into(), json!([]));
        session.insert("builder_story_reference_images".into(), json!([]));
        session.insert("builder_story_source_path".into(), json!(""));
    }
    if !keep.lyrics {
        let mut mapper = session.get("lyric_mapper").and_then(Value::as_object).cloned().unwrap_or_default();
        if !mapper.is_empty() {
            mapper.insert("source_text".into(), json!(""));
            session.insert("lyric_mapper".into(), Value::Object(mapper));
        }
    }
}

/// `_strip_storyboard_images`: the Story Builder's scene pictures are dropped from `storyboard.json`.
fn strip_storyboard_images(project: &Path) {
    let path = project.join("storyboard").join("storyboard.json");
    let Ok(bytes) = fs::read(&path) else { return };
    let source = String::from_utf8_lossy(&bytes);
    let Ok(mut data) = h3_core::pyjson::parse(source.trim_start_matches('\u{feff}')) else { return };
    let Some(map) = data.as_object_mut() else { return };
    let mut scenes = map.get("scenes").and_then(Value::as_array).cloned().unwrap_or_default();
    for scene in scenes.iter_mut().filter_map(Value::as_object_mut) {
        for key in ["image_path", "image_data", "image_name"] {
            scene.insert(key.into(), json!(""));
        }
    }
    map.insert("scenes".into(), Value::Array(scenes));
    let _ = atomic_write_json(&path, &data);
}

/// `_prune_unkept_project_folders`: folders of what the branch does not keep are removed from the new project.
pub fn prune_unkept_project_folders(project: &Path, keep: &Keep) {
    let mut removals: Vec<&str> = Vec::new();
    if !keep.videos {
        removals.extend(VIDEO_DIRS);
        removals.push("render_logs");
    }
    if !keep.images {
        removals.extend(IMAGE_DIRS);
        strip_storyboard_images(project);
    }
    if !keep.mappings {
        removals.push("subject_location");
        let _ = fs::remove_dir_all(context_folder(project).join("flux_references"));
    }
    if !keep.prompts {
        removals.push("prompts");
    }
    if !keep.notes && !keep.prompts {
        removals.push("storyboard");
    }
    for name in removals {
        let path = project.join(name);
        if path.is_dir() {
            let _ = fs::remove_dir_all(path);
        }
    }
}

// -- media outside the project --------------------------------------------------------------------------------------------

/// `_branch_scene_number`.
fn branch_scene_number(segment: &Value, index: i64) -> i64 {
    if let Some(map) = segment.as_object() {
        for key in ["scene_slot_number", "overlay_slot_number", "scene_number"] {
            let value = map.get(key).filter(|v| truthy(v)).and_then(to_int).unwrap_or(0);
            if value > 0 {
                return value;
            }
        }
    }
    index.max(1)
}

/// `_copy_external_media_into_branch`: a file outside the project goes into the scene's backup folder. Returns where it is
/// now, or "" when there is no such file.
fn copy_external_media(source: &str, project: &Path, scene: i64, key: &str, copied: &mut HashMap<String, String>) -> io::Result<String> {
    let source = source.trim().trim_matches('"');
    if source.is_empty() {
        return Ok(String::new());
    }
    let abs = absolute(Path::new(source));
    if is_inside_folder(&abs, project) {
        return Ok(if abs.is_file() { text(&abs) } else { String::new() });
    }
    if let Some(done) = copied.get(&text(&abs)) {
        return Ok(done.clone());
    }
    if !abs.is_file() {
        return Ok(String::new());
    }
    let backup = project.join("rendered_scene_videos_backup").join(format!("scene_{scene:04}"));
    fs::create_dir_all(&backup)?;
    let stem = abs.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = Some(extension_of(&text(&abs))).filter(|e| !e.is_empty()).unwrap_or_else(|| ".mp4".into());
    let safe_key = Some(squeeze(key, key_chars)).filter(|k| !k.is_empty()).unwrap_or_else(|| "media".into());
    let mut target = backup.join(format!("{safe_key}_{stem}{ext}"));
    let mut index = 2;
    while target.exists() {
        target = backup.join(format!("{safe_key}_{stem}_{index:02}{ext}"));
        index += 1;
    }
    copy2(&abs, &target)?;
    copied.insert(text(&abs), text(&target));
    Ok(text(&target))
}

/// `_relocate_external_minimax_stage_media`: the MiniMax H3 pass clips, backups and source video of every scene are copied
/// into the project when they lived elsewhere.
pub fn relocate_external_stage_media(session: &mut Obj, project: &Path) -> io::Result<()> {
    let mut copied: HashMap<String, String> = HashMap::new();
    for list_key in ["segments", "overlay_segments"] {
        let Some(list) = session.get_mut(list_key).and_then(Value::as_array_mut) else { continue };
        for (i, segment) in list.iter_mut().enumerate() {
            let scene = branch_scene_number(segment, i as i64 + 1);
            let Some(map) = segment.as_object_mut() else { continue };
            for key in STAGE_PATH_KEYS {
                if let Some(current) = map.get(key) {
                    let moved = copy_external_media(&text_or_empty(Some(current)), project, scene, key, &mut copied)?;
                    map.insert(key.into(), json!(moved));
                }
            }
            for (key, label) in [("video_backup_paths", "video_backup"), ("video_backup_thumbnail_paths", "video_backup_thumb")] {
                if let Some(items) = map.get(key).and_then(Value::as_array).cloned() {
                    let mut moved = Vec::new();
                    for item in items {
                        let path = copy_external_media(&text_or_empty(Some(&item)), project, scene, label, &mut copied)?;
                        if !path.is_empty() {
                            moved.push(json!(path));
                        }
                    }
                    map.insert(key.into(), Value::Array(moved));
                }
            }
            if map.get("video_source_path").is_some_and(truthy) {
                let moved = copy_external_media(&text_or_empty(map.get("video_source_path")), project, scene, "video_source", &mut copied)?;
                map.insert("video_source_path".into(), json!(moved));
            }
        }
    }
    Ok(())
}

// -- pulling a session's files into the project ---------------------------------------------------------------------------

/// `_copy_reference_asset`: a reference picture or audio file goes into `project_context/scene_NNNN/`.
fn copy_reference_asset(project: &Path, scene: i64, key: &str, source: &str) -> io::Result<String> {
    let source = source.trim().trim_matches('"');
    if source.is_empty() || !Path::new(source).is_file() {
        return Ok(String::new());
    }
    let ext = Some(extension_of(source).to_lowercase()).filter(|e| !e.is_empty()).unwrap_or_else(|| ".png".into());
    let ext = if [".png", ".jpg", ".jpeg", ".webp", ".wav", ".mp3", ".flac", ".m4a", ".ogg"].contains(&ext.as_str()) { ext } else { ".bin".into() };
    let safe_key = Some(squeeze(key, asset_key_chars)).filter(|k| !k.is_empty()).unwrap_or_else(|| "asset".into());
    let folder = context_folder(project).join(format!("scene_{:04}", scene.max(1)));
    copy_file_if_exists(source, &folder.join(format!("{safe_key}{ext}")))
}

/// `_overlay_scene_number`.
fn overlay_scene_number(segment: &Obj, fallback: i64) -> i64 {
    for key in ["overlay_slot_number", "scene_slot_number", "slot_number"] {
        let value = segment.get(key).filter(|v| truthy(v)).and_then(to_int).unwrap_or(0);
        if value >= 10001 {
            return value;
        }
    }
    10000 + fallback.max(1)
}

fn copy_ref_files(project: &Path, scene: i64, segment: &mut Obj) -> io::Result<()> {
    for key in ["custom_image_path", "ref_image_path", "flux_subject_image_path", "flux_location_image_path"] {
        let copied = copy_reference_asset(project, scene, key, &text_or_empty(segment.get(key)))?;
        if !copied.is_empty() {
            segment.insert(key.into(), json!(copied));
        }
    }
    Ok(())
}

fn copy_rendered_video(project: &Path, scene: i64, segment: &mut Obj) -> io::Result<()> {
    let video = stripped(segment.get("video_path"));
    if !video.is_empty() && Path::new(&video).is_file() {
        let target = project.join("rendered_scene_videos").join(format!("video_{scene:04}-audio.mp4"));
        let copied = copy_file_if_exists(&video, &target)?;
        let folder = Path::new(&copied).parent().map(text).unwrap_or_default();
        segment.insert("video_path".into(), json!(copied));
        segment.insert("video_folder".into(), json!(folder));
        segment.insert("video_status".into(), json!("done"));
    }
    Ok(())
}

fn copy_approved_image(project: &Path, scene: i64, segment: &mut Obj) -> io::Result<String> {
    let approved = stripped(segment.get("approved_image_path"));
    if !approved.is_empty() && Path::new(&approved).is_file() {
        let ext = Some(extension_of(&approved)).filter(|e| !e.is_empty()).unwrap_or_else(|| ".png".into());
        let copied = copy_file_if_exists(&approved, &scene_image_path(project, scene, &ext))?;
        segment.insert("approved_image_path".into(), json!(copied));
    }
    Ok(approved)
}

/// `_copy_session_assets_to_project`: files the session points at outside the project are copied into it, under the
/// names the Builder uses, and the session now points at the copies.
pub fn copy_session_assets_to_project(project: &Path, session: &mut Obj, now: &Stamp) -> io::Result<()> {
    let project = absolute(project);
    if let Some(ingredients) = session.get_mut("flux_global_image_ingredients").and_then(Value::as_array_mut) {
        let folder = context_folder(&project).join("flux_global");
        for (i, ingredient) in ingredients.iter_mut().enumerate() {
            let Some(map) = ingredient.as_object_mut() else { continue };
            let source = stripped(map.get("path"));
            if source.is_empty() || !Path::new(&source).is_file() {
                continue;
            }
            let ext = Some(extension_of(&source).to_lowercase()).filter(|e| !e.is_empty()).unwrap_or_else(|| ".png".into());
            let copied = copy_file_if_exists(&source, &folder.join(format!("global_ingredient_{}{ext}", i + 1)))?;
            if !copied.is_empty() {
                map.insert("path".into(), json!(copied));
            }
        }
    }
    let mut segments = match session.get_mut("segments").and_then(Value::as_array_mut) {
        Some(list) => std::mem::take(list),
        None => {
            session.insert("segments".into(), json!([]));
            return Ok(());
        }
    };
    let outcome = copy_segment_assets(&project, &mut segments, session, now);
    // The scenes go back where they were whatever happened, so a failed copy does not lose them.
    if let Some(list) = session.get_mut("segments").and_then(Value::as_array_mut) {
        *list = segments;
    }
    outcome
}

fn copy_segment_assets(project: &Path, segments: &mut [Value], session: &mut Obj, now: &Stamp) -> io::Result<()> {
    for (i, segment) in segments.iter_mut().enumerate() {
        let scene = i as i64 + 1;
        let Some(map) = segment.as_object_mut() else { continue };
        let approved = copy_approved_image(project, scene, map)?;
        let mut history = Vec::new();
        if let Some(items) = map.get("image_history").and_then(Value::as_array) {
            for item in items {
                let path = stripped(Some(item));
                if path.is_empty() || !Path::new(&path).is_file() || path == approved || is_internal_approved_image(&path) {
                    continue;
                }
                let ext = Some(extension_of(&path)).filter(|e| !e.is_empty()).unwrap_or_else(|| ".png".into());
                let target = unique_preview_path(project, scene as u32, &ext, now)?;
                let copied = copy_file_if_exists(&path, &target)?;
                if !copied.is_empty() && !history.contains(&copied) {
                    history.push(copied);
                }
            }
        }
        let index = if history.is_empty() {
            -1
        } else {
            let last = history.len() as i64 - 1;
            let current = match map.get("image_history_index") {
                None => last,
                Some(v) if !truthy(v) => 0,
                Some(v) => to_int(v).unwrap_or(last),
            };
            current.clamp(0, last)
        };
        map.insert("image_history".into(), json!(history));
        map.insert("image_history_index".into(), json!(index));
        copy_rendered_video(project, scene, map)?;
        let audio = stripped(map.get("custom_audio_path"));
        if !audio.is_empty() && Path::new(&audio).is_file() {
            let ext = Some(extension_of(&audio)).filter(|e| !e.is_empty()).unwrap_or_else(|| ".wav".into());
            let copied = copy_file_if_exists(&audio, &scene_audio_path(project, scene, &ext))?;
            map.insert("custom_audio_path".into(), json!(copied));
        }
        copy_ref_files(project, scene, map)?;
        if let Some(ingredients) = map.get_mut("flux_image_ingredients").and_then(Value::as_array_mut) {
            for (n, ingredient) in ingredients.iter_mut().enumerate() {
                let Some(entry) = ingredient.as_object_mut() else { continue };
                let copied = copy_reference_asset(project, scene, &format!("flux_ingredient_{}", n + 1), &text_or_empty(entry.get("path")))?;
                if !copied.is_empty() {
                    entry.insert("path".into(), json!(copied));
                }
            }
        }
    }
    let Some(overlays) = session.get_mut("overlay_segments").and_then(Value::as_array_mut) else { return Ok(()) };
    assign_overlay_slots(overlays);
    for (i, segment) in overlays.iter_mut().enumerate() {
        let Some(map) = segment.as_object_mut() else { continue };
        let scene = overlay_scene_number(map, i as i64 + 1);
        map.insert("track".into(), json!("overlay"));
        copy_approved_image(project, scene, map)?;
        copy_rendered_video(project, scene, map)?;
        copy_ref_files(project, scene, map)?;
    }
    Ok(())
}

/// `_assign_overlay_scene_numbers` for a session: every overlay scene gets its own slot number from 10001.
pub fn assign_overlay_numbers(session: &mut Obj) {
    if let Some(overlays) = session.get_mut("overlay_segments").and_then(Value::as_array_mut) {
        assign_overlay_slots(overlays);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn stamp() -> Stamp {
        Stamp { compact: "20260101_000000".into(), iso: "2026-01-01T00:00:00".into(), epoch: 1_767_225_600.0 }
    }

    #[test]
    fn keep_defaults_to_everything_and_follows_the_request() {
        assert!(Keep::from_payload(&Obj::new()).is_everything());
        let payload = json!({"keep": {"lyrics": true, "videos": 1}});
        let keep = Keep::from_payload(payload.as_object().unwrap());
        assert!(keep.lyrics && keep.videos && !keep.notes && !keep.images && !keep.overlays);
    }

    #[test]
    fn paths_are_recognized_as_paths() {
        assert!(looks_like_filesystem_path("C:\\a\\b.png"));
        assert!(looks_like_filesystem_path("\"D:/x\""));
        assert!(looks_like_filesystem_path("\\\\server\\share"));
        assert!(looks_like_filesystem_path("/home/a"));
        assert!(!looks_like_filesystem_path("a quiet room"));
        assert!(!looks_like_filesystem_path("C:\nfoo"));
        assert!(!looks_like_filesystem_path(""));
    }

    #[test]
    fn rewriting_moves_paths_and_blanks_missing_copies() {
        let dir = tempfile::tempdir().unwrap();
        let (old, new) = (dir.path().join("old"), dir.path().join("new"));
        write(&old.join("a.png"), "x");
        write(&old.join("only_old.png"), "x");
        write(&new.join("a.png"), "x");
        let elsewhere = dir.path().join("elsewhere.png");
        write(&elsewhere, "x");
        let mut value = json!({
            "kept": text(&old.join("a.png")),
            "missing_in_new": text(&old.join("only_old.png")),
            "gone_everywhere": text(&old.join("never.png")),
            "outside": text(&elsewhere),
            "note": "a quiet room",
            "nested": [text(&old.join("a.png")), 4]
        });
        rewrite_project_paths(&mut value, &text(&old), &text(&new));
        assert_eq!(value["kept"], text(&new.join("a.png")));
        assert_eq!(value["missing_in_new"], "");
        assert_eq!(value["gone_everywhere"], text(&new.join("never.png")));
        assert_eq!(value["outside"], text(&elsewhere));
        assert_eq!(value["note"], "a quiet room");
        assert_eq!(value["nested"][0], text(&new.join("a.png")));
        clear_remaining_source_paths(&mut value, &text(&old));
        assert_eq!(value["outside"], text(&elsewhere));
    }

    #[test]
    fn a_branch_copy_leaves_out_what_is_not_kept() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        for file in [
            "vrgdg_builder_session.json", "rendered_scene_videos/video_0001-audio.mp4", "zimage_approved/image_0001.png", "prompts/p.json",
            "subject_location/reference_descriptions.json", "storyboard/storyboard.json", "storyboard/pic.png", "session_backups/old.json",
            "FINAL_VIDEO_x.mp4", "project_audio/song.wav", "project_context/flux_references/a.png", "project_context/themestyle.txt", "work.tmp",
        ] {
            write(&source.join(file), "x");
        }
        let target = dir.path().join("target");
        let keep = Keep { lyrics: true, notes: true, prompts: false, mappings: false, images: false, videos: false, overlays: true };
        copy_project_tree(&source, &target, &keep).unwrap();
        let has = |p: &str| target.join(p).exists();
        assert!(has("vrgdg_builder_session.json") && has("project_audio/song.wav") && has("project_context/themestyle.txt") && has("storyboard/storyboard.json"));
        for gone in ["rendered_scene_videos", "zimage_approved", "prompts", "subject_location", "storyboard/pic.png", "session_backups", "FINAL_VIDEO_x.mp4", "project_context/flux_references", "work.tmp"] {
            assert!(!has(gone), "{gone} was copied");
        }
        let everything = dir.path().join("everything");
        copy_project_tree(&source, &everything, &Keep::everything()).unwrap();
        assert!(everything.join("rendered_scene_videos").is_dir() && everything.join("storyboard/pic.png").is_file());
        assert!(!everything.join("session_backups").exists() && !everything.join("FINAL_VIDEO_x.mp4").exists());
    }

    #[test]
    fn keeping_only_some_things_blanks_the_rest_of_the_session() {
        let mut session = json!({
            "segments": [{
                "lyric_text": "la", "timeline_note": "n", "t2i_prompt": "p", "subject_ids": ["a"], "approved_image_path": "x.png",
                "image_history": ["x.png"], "image_history_index": 0, "video_path": "v.mp4", "video_history_index": 2, "image_status": "done",
                "custom_audio_path": "a.wav"
            }, 5],
            "overlay_segments": [{"lyric_text": "o"}],
            "overlay_track": {"enabled": true, "x": 1},
            "lyric_mapper": {"source_text": "words", "keep": 1},
            "timeline_markers": [1]
        });
        let keep = Keep { lyrics: false, notes: false, prompts: true, mappings: false, images: false, videos: false, overlays: false };
        apply_branch_keep(session.as_object_mut().unwrap(), &keep);
        let scene = &session["segments"][0];
        assert_eq!(scene["lyric_text"], "");
        assert_eq!(scene["timeline_note"], "");
        assert_eq!(scene["t2i_prompt"], "p");
        assert_eq!(scene["subject_ids"], json!([]));
        assert_eq!(scene["approved_image_path"], "");
        assert_eq!(scene["image_history"], json!([]));
        assert_eq!(scene["image_history_index"], -1);
        assert_eq!(scene["video_history_index"], -1);
        assert_eq!(scene["image_status"], "none");
        assert_eq!(scene["video_status"], "none");
        assert_eq!(scene["custom_audio_path"], "");
        assert_eq!(scene["preview_mode"], "image");
        assert_eq!(session["segments"][1], json!({"lyric_text": "", "preview_mode": "image", "video_status": "none"}));
        assert_eq!(session["overlay_segments"], json!([]));
        assert_eq!(session["overlay_track"], json!({"enabled": false, "x": 1}));
        assert_eq!(session["lyric_mapper"], json!({"source_text": "", "keep": 1}));
        assert_eq!(session["timeline_markers"], json!([]));
        assert_eq!(session["flux_reference_builder"], json!({}));
    }

    #[test]
    fn session_files_are_copied_under_the_builders_names() {
        let dir = tempfile::tempdir().unwrap();
        let (project, outside) = (dir.path().join("project"), dir.path().join("outside"));
        write(&outside.join("pic.JPG"), "approved");
        write(&outside.join("old.png"), "history");
        write(&outside.join("clip.mp4"), "video");
        write(&outside.join("voice.wav"), "audio");
        write(&outside.join("ref.webp"), "ref");
        let mut session = json!({
            "segments": [{
                "approved_image_path": text(&outside.join("pic.JPG")), "image_history": [text(&outside.join("old.png")), "gone.png"],
                "video_path": text(&outside.join("clip.mp4")), "custom_audio_path": text(&outside.join("voice.wav")),
                "ref_image_path": text(&outside.join("ref.webp"))
            }],
            "overlay_segments": [{"video_path": text(&outside.join("clip.mp4"))}]
        });
        copy_session_assets_to_project(&project, session.as_object_mut().unwrap(), &stamp()).unwrap();
        let scene = &session["segments"][0];
        assert!(scene["approved_image_path"].as_str().unwrap().ends_with("image_0001.jpg") || scene["approved_image_path"].as_str().unwrap().ends_with("image_0001.png"));
        assert!(Path::new(scene["approved_image_path"].as_str().unwrap()).is_file());
        assert_eq!(scene["image_history"].as_array().unwrap().len(), 1);
        assert_eq!(scene["image_history_index"], 0);
        assert!(scene["video_path"].as_str().unwrap().ends_with("video_0001-audio.mp4"));
        assert_eq!(scene["video_status"], "done");
        assert!(scene["custom_audio_path"].as_str().unwrap().ends_with("audio_0001.wav"));
        assert!(scene["ref_image_path"].as_str().unwrap().contains("scene_0001"));
        let overlay = &session["overlay_segments"][0];
        assert_eq!(overlay["overlay_slot_number"], 10001);
        assert_eq!(overlay["track"], "overlay");
        assert!(overlay["video_path"].as_str().unwrap().ends_with("video_10001-audio.mp4"));
    }

    #[test]
    fn external_stage_clips_go_into_the_scene_backup_folder() {
        let dir = tempfile::tempdir().unwrap();
        let (project, outside) = (dir.path().join("project"), dir.path().join("outside"));
        write(&outside.join("stage.mp4"), "s");
        write(&project.join("inside.mp4"), "i");
        let mut session = json!({"segments": [{
            "scene_slot_number": 3, "minimax_h3_stage1_path": text(&outside.join("stage.mp4")),
            "video_backup_paths": [text(&project.join("inside.mp4")), text(&outside.join("stage.mp4")), "gone.mp4"],
            "video_source_path": text(&outside.join("stage.mp4"))
        }]});
        relocate_external_stage_media(session.as_object_mut().unwrap(), &project).unwrap();
        let scene = &session["segments"][0];
        let stage = scene["minimax_h3_stage1_path"].as_str().unwrap();
        assert!(stage.contains("scene_0003") && stage.ends_with("minimax_h3_stage1_path_stage.mp4"), "{stage}");
        assert_eq!(scene["video_backup_paths"].as_array().unwrap().len(), 2);
        // The same source file is copied once, whichever field names it.
        assert_eq!(scene["video_source_path"], scene["minimax_h3_stage1_path"]);
    }
}
