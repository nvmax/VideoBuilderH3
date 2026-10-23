//! Scene latent files and the dirty flags that say a continued scene must be rendered again. Port of the file side of
//! `SceneLatentManager` in `minimax/latent_manager.py`. The tensors themselves are written by the ComfyUI nodes. This
//! module only manages the files around them.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{json, Value};

use crate::atomic::atomic_write_json;
use crate::layout::{session_path, Stamp};
use crate::pyjson;
use crate::takes;

const EXTENSIONS: [&str; 5] = [".latent", ".latent.json", ".pass1.latent", ".pass1.latent.json", ".dirty"];

/// The `latents` folder. Like the pack, asking for a path creates the folder.
fn folder(project: &Path) -> PathBuf {
    let dir = project.join("latents");
    let _ = fs::create_dir_all(&dir);
    dir
}

pub fn latent_path(project: &Path, scene: u32) -> PathBuf {
    folder(project).join(format!("scene_{scene:03}.latent"))
}

pub fn pass1_path(project: &Path, scene: u32) -> PathBuf {
    folder(project).join(format!("scene_{scene:03}.pass1.latent"))
}

pub fn dirty_path(project: &Path, scene: u32) -> PathBuf {
    folder(project).join(format!("scene_{scene:03}.dirty"))
}

fn sidecar(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.json", path.display()))
}

pub fn latent_exists(project: &Path, scene: u32) -> bool {
    project.is_dir() && latent_path(project, scene).is_file()
}

pub fn is_dirty(project: &Path, scene: u32) -> bool {
    project.is_dir() && dirty_path(project, scene).is_file()
}

/// Mark a scene's latent dirty (for example because its predecessor was re-rendered).
pub fn mark_dirty(project: &Path, scene: u32, reason: &str, now: &Stamp) {
    if !project.is_dir() {
        return;
    }
    let reason = if reason.is_empty() { "Predecessor scene re-rendered or modified" } else { reason };
    let value = json!({"scene_number": scene, "timestamp": now.epoch, "reason": reason});
    if let Err(e) = atomic_write_json(&dirty_path(project, scene), &value) {
        eprintln!("[VRGDG-RS Latent] Failed to mark scene {scene:03} dirty: {e}");
    }
}

pub fn clear_dirty(project: &Path, scene: u32) {
    if project.is_dir() {
        let path = dirty_path(project, scene);
        if path.is_file() {
            let _ = fs::remove_file(path);
        }
    }
}

/// Whether the scene's render was built from its predecessor's latent. Reads `minimax_h3_continuity_mode_used` from the
/// saved session. A scene with no record (older project, never rendered) counts as continued.
pub fn continues_from_predecessor(project: &Path, scene: u32) -> bool {
    let read = || -> Option<bool> {
        let text = fs::read_to_string(session_path(project)).ok()?;
        let session = pyjson::parse(&text).ok()?;
        let segment = session.get("segments")?.as_array()?.get((scene as usize).checked_sub(1)?)?;
        let used = match segment.as_object()?.get("minimax_h3_continuity_mode_used") {
            Some(Value::String(s)) => s.trim().to_lowercase(),
            Some(Value::Null) | None => String::new(),
            Some(other) => other.to_string().trim().to_lowercase(),
        };
        Some(used != "off")
    };
    read().unwrap_or(true)
}

/// Mark the next scene's latent dirty, only when that latent was continued from this scene.
pub fn mark_successor_dirty(project: &Path, scene: u32, reason: &str, now: &Stamp) -> bool {
    let successor = scene + 1;
    if !latent_exists(project, successor) || !continues_from_predecessor(project, successor) {
        return false;
    }
    mark_dirty(project, successor, reason, now);
    true
}

/// Scene numbers that have a dirty flag, ascending.
pub fn list_dirty(project: &Path) -> Vec<u32> {
    let dir = project.join("latents");
    let Ok(entries) = fs::read_dir(&dir) else { return vec![] };
    let mut found: Vec<u32> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if !(name.starts_with("scene_") && name.ends_with(".dirty")) {
                return None;
            }
            name.split('_').nth(1)?.split('.').next()?.parse().ok()
        })
        .collect();
    found.sort_unstable();
    found
}

/// `^scene_(\d{3,})\.latent$`, case-insensitive.
fn latent_number(name: &str) -> Option<u32> {
    let lower = name.to_lowercase();
    let digits = lower.strip_prefix("scene_")?.strip_suffix(".latent")?;
    if digits.len() >= 3 && digits.bytes().all(|b| b.is_ascii_digit()) { digits.parse().ok() } else { None }
}

fn latent_numbers(dir: &Path) -> Vec<u32> {
    let mut numbers: Vec<u32> = fs::read_dir(dir)
        .map(|rd| rd.filter_map(Result::ok).filter_map(|e| latent_number(&e.file_name().to_string_lossy())).collect())
        .unwrap_or_default();
    numbers.sort_unstable();
    numbers.dedup();
    numbers
}

/// Delete a scene's latent file and its sidecars, take archive and dirty flag.
pub fn delete_latent(project: &Path, scene: u32, reindex: bool) -> bool {
    let path = latent_path(project, scene);
    let pass1 = pass1_path(project, scene);
    let mut deleted = false;
    for p in [path.clone(), sidecar(&path), pass1.clone(), sidecar(&pass1), dirty_path(project, scene)] {
        if p.is_file() {
            match fs::remove_file(&p) {
                Ok(()) => deleted = true,
                Err(e) => eprintln!("[VRGDG-RS Latent] Failed to delete {}: {e}", p.display()),
            }
        }
    }
    takes::delete_scene_takes(path.parent().unwrap_or(project), scene);
    if reindex {
        reindex_latents(project, scene);
    }
    deleted
}

/// Delete every scene latent, sidecar and dirty flag. Returns the number of files removed.
pub fn delete_all_latents(project: &Path) -> usize {
    let dir = project.join("latents");
    if !project.is_dir() || !dir.is_dir() {
        return 0;
    }
    takes::delete_all_takes(&dir);
    let mut removed = 0;
    for entry in fs::read_dir(&dir).into_iter().flatten().filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("scene_") && [".latent", ".latent.json", ".dirty"].iter().any(|e| name.ends_with(e)) {
            match fs::remove_file(entry.path()) {
                Ok(()) => removed += 1,
                Err(e) => eprintln!("[VRGDG-RS Latent] Failed to delete {name}: {e}"),
            }
        }
    }
    removed
}

/// Shift scene latent numbers down by one for every scene after the deleted one.
pub fn reindex_latents(project: &Path, deleted: u32) {
    let dir = project.join("latents");
    if !project.is_dir() || !dir.is_dir() {
        return;
    }
    for number in latent_numbers(&dir).into_iter().filter(|n| *n > deleted) {
        let old_base = dir.join(format!("scene_{number:03}"));
        let new_base = dir.join(format!("scene_{:03}", number - 1));
        for ext in EXTENSIONS {
            let (old, new) = (PathBuf::from(format!("{}{ext}", old_base.display())), PathBuf::from(format!("{}{ext}", new_base.display())));
            if old.is_file() {
                if new.is_file() {
                    let _ = fs::remove_file(&new);
                }
                if let Err(e) = fs::rename(&old, &new) {
                    eprintln!("[VRGDG-RS Latent] Reindex rename failed {} -> {}: {e}", old.display(), new.display());
                }
            }
        }
    }
    takes::shift_takes(&dir, deleted + 1, -1);
}

/// Shift scene latent numbers up by one for the given scene and every later scene.
pub fn make_room_for_scene(project: &Path, scene: u32) {
    let dir = project.join("latents");
    if !project.is_dir() || !dir.is_dir() {
        return;
    }
    let mut numbers: Vec<u32> = latent_numbers(&dir).into_iter().filter(|n| *n >= scene).collect();
    numbers.reverse();
    for number in numbers {
        let old_base = dir.join(format!("scene_{number:03}"));
        let new_base = dir.join(format!("scene_{:03}", number + 1));
        for ext in EXTENSIONS {
            let old = PathBuf::from(format!("{}{ext}", old_base.display()));
            if old.is_file() {
                let _ = fs::rename(&old, PathBuf::from(format!("{}{ext}", new_base.display())));
            }
        }
    }
    takes::shift_takes(&dir, scene, 1);
}

/// Copy every latent file when a project is saved under a new name.
pub fn copy_latents_folder(source: &Path, target: &Path) {
    let src = source.join("latents");
    if !source.is_dir() || !src.is_dir() {
        return;
    }
    let dst = target.join("latents");
    let _ = fs::create_dir_all(&dst);
    for entry in fs::read_dir(&src).into_iter().flatten().filter_map(Result::ok) {
        if entry.path().is_file() {
            let _ = fs::copy(entry.path(), dst.join(entry.file_name()));
        }
    }
    takes::copy_takes(&src, &dst);
}

/// File facts about a scene's latent, read without loading the tensors.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LatentInfo {
    pub ok: bool,
    pub exists: bool,
    pub path: String,
    pub scene_number: u32,
    pub size_bytes: u64,
    pub frame_count: i64,
    pub token_count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fps: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<f64>,
    /// Frames past the scene's visible end inside this latent. `None` for latents saved before this was recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tail_padding_frames: Option<i64>,
    pub dirty: bool,
}

fn meta_int(meta: &serde_json::Map<String, Value>, key: &str) -> io::Result<i64> {
    let bad = || io::Error::new(io::ErrorKind::InvalidData, format!("latent sidecar value {key} is not a whole number"));
    match meta.get(key) {
        None | Some(Value::Null) => Ok(0),
        Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)).ok_or_else(bad),
        Some(Value::String(s)) => s.trim().parse::<i64>().map_err(|_| bad()),
        Some(_) => Err(bad()),
    }
}

fn meta_float(meta: &serde_json::Map<String, Value>, key: &str) -> Option<f64> {
    match meta.get(key) {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => s.trim().parse().ok(),
        _ => None,
    }
}

/// Query metadata and file stats for a scene latent.
pub fn latent_info(project: &Path, scene: u32) -> io::Result<LatentInfo> {
    let path = latent_path(project, scene);
    let dirty = is_dirty(project, scene);
    let shown = path.display().to_string();
    if !path.is_file() {
        return Ok(LatentInfo {
            ok: true,
            exists: false,
            path: shown,
            scene_number: scene,
            size_bytes: 0,
            frame_count: 0,
            token_count: 0,
            fps: None,
            timestamp: None,
            tail_padding_frames: None,
            dirty,
        });
    }
    let metadata = fs::metadata(&path)?;
    let meta = fs::read_to_string(sidecar(&path))
        .ok()
        .and_then(|t| pyjson::parse(&t).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0.0, |d| d.as_secs_f64());
    // `float(meta.get("timestamp", 0.0) or mtime)`: a missing, empty or zero stamp falls back to the file time.
    let timestamp = match meta.get("timestamp") {
        None | Some(Value::Null) => mtime,
        Some(Value::String(s)) if s.is_empty() => mtime,
        Some(Value::Number(n)) if n.as_f64() == Some(0.0) => mtime,
        _ => meta_float(&meta, "timestamp").unwrap_or(mtime),
    };
    let tail = match meta.get("tail_padding_frames") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if s.is_empty() => None,
        Some(Value::String(s)) => s.trim().parse::<f64>().ok().map(|f| f as i64),
        Some(Value::Number(n)) => n.as_f64().map(|f| f as i64),
        Some(_) => None,
    };
    Ok(LatentInfo {
        ok: true,
        exists: true,
        path: shown,
        scene_number: scene,
        size_bytes: metadata.len(),
        frame_count: meta_int(&meta, "frame_count")?,
        token_count: meta_int(&meta, "token_count")?,
        fps: Some(meta_float(&meta, "fps").unwrap_or(24.0)),
        timestamp: Some(timestamp),
        tail_padding_frames: tail,
        dirty,
    })
}

/// Whether the scene's first pass latent was saved by the same render as its final latent. Selecting an older take, a
/// single pass re-render or a scene move leaves a first pass file that no longer belongs to the final latent, and it
/// must then be ignored.
pub fn pass1_latent_matches(project: &Path, scene: u32) -> bool {
    let Ok(info) = latent_info(project, scene) else { return false };
    let pass1 = pass1_path(project, scene);
    let side = sidecar(&pass1);
    if !info.exists || !pass1.is_file() || !side.is_file() {
        return false;
    }
    let Some(meta) = fs::read_to_string(&side).ok().and_then(|t| pyjson::parse(&t).ok()) else { return false };
    let (Some(pass1_time), Some(tokens)) = (meta_float_value(&meta, "timestamp"), meta_float_value(&meta, "token_count")) else {
        return false;
    };
    let gap = info.timestamp.unwrap_or(0.0) - pass1_time;
    // The first pass is saved during the render, the final latent at its end.
    tokens as i64 == info.token_count && (0.0..4.0 * 3600.0).contains(&gap)
}

fn meta_float_value(meta: &Value, key: &str) -> Option<f64> {
    match meta.get(key)? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}
