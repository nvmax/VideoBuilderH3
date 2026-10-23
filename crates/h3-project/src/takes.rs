//! Per take scene latents. Port of `minimax/latent_takes.py`.
//!
//! Every render saves the scene's latent to `latents/scene_NNN.latent`, which the next scene reads for Latent
//! Continuation Masked. The take archive keeps one latent per render in `latents/scene_NNN.takes/`, tied to the render's
//! video by a fingerprint (size plus first and last bytes), so choosing an older take puts its latent back as the
//! active one. The flow is `archive_latent` (pending), `attach_video` (when the final video is collected), then
//! `activate_for_video` (when the user selects a take).

use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sha1::{Digest, Sha1};

use crate::atomic::atomic_write_json;
use crate::layout::Stamp;
use crate::pyjson;

const SAMPLE_BYTES: u64 = 256 * 1024;
const INDEX_NAME: &str = "index.json";

fn folder(latents_dir: &Path, scene: u32) -> PathBuf {
    latents_dir.join(format!("scene_{scene:03}.takes"))
}

/// `^scene_(\d{3,})\.takes$`, case-insensitive. Returns the scene number.
fn folder_number(name: &str) -> Option<u32> {
    let lower = name.to_lowercase();
    let digits = lower.strip_prefix("scene_")?.strip_suffix(".takes")?;
    if digits.len() >= 3 && digits.bytes().all(|b| b.is_ascii_digit()) {
        digits.parse().ok()
    } else {
        None
    }
}

fn read_index(folder: &Path) -> Value {
    let parsed = fs::read_to_string(folder.join(INDEX_NAME)).ok().and_then(|t| pyjson::parse(&t).ok());
    match parsed {
        Some(Value::Object(map)) if map.get("takes").is_some_and(Value::is_array) => Value::Object(map),
        _ => json!({"takes": []}),
    }
}

fn write_index(folder: &Path, index: &Value) -> io::Result<()> {
    fs::create_dir_all(folder)?;
    atomic_write_json(&folder.join(INDEX_NAME), index)
}

fn take_files(folder: &Path, id: &str) -> [PathBuf; 2] {
    [folder.join(format!("{id}.latent")), folder.join(format!("{id}.latent.json"))]
}

fn remove_take_files(folder: &Path, id: &str) {
    for path in take_files(folder, id) {
        if path.is_file() {
            let _ = fs::remove_file(path);
        }
    }
}

fn text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

/// Identify a video by its size and first and last bytes. Empty when the file cannot be read.
pub fn video_fingerprint(path: &Path) -> String {
    let read = || -> io::Result<String> {
        let size = fs::metadata(path)?.len();
        let mut hasher = Sha1::new();
        hasher.update(size.to_string().as_bytes());
        let mut file = fs::File::open(path)?;
        let mut buffer = Vec::new();
        (&mut file).take(SAMPLE_BYTES).read_to_end(&mut buffer)?;
        hasher.update(&buffer);
        if size > SAMPLE_BYTES {
            file.seek(SeekFrom::Start(SAMPLE_BYTES.max(size - SAMPLE_BYTES)))?;
            let mut tail = Vec::new();
            (&mut file).take(SAMPLE_BYTES).read_to_end(&mut tail)?;
            hasher.update(&tail);
        }
        Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
    };
    read().unwrap_or_default()
}

/// Copy a just saved latent into the archive as the pending take. Returns its id.
pub fn archive_latent(latents_dir: &Path, scene: u32, latent_path: &Path, now: &Stamp) -> io::Result<String> {
    let folder = folder(latents_dir, scene);
    fs::create_dir_all(&folder)?;
    let mut index = read_index(&folder);
    let previous = text(index.get("pending"));
    if !previous.is_empty() {
        remove_take_files(&folder, &previous); // a render whose video was never collected
    }
    let id = format!("{}_{:03}", now.compact, now.ms_part());
    fs::copy(latent_path, folder.join(format!("{id}.latent")))?;
    let sidecar = PathBuf::from(format!("{}.json", latent_path.display()));
    if sidecar.is_file() {
        fs::copy(&sidecar, folder.join(format!("{id}.latent.json")))?;
    }
    index["pending"] = json!(id);
    write_index(&folder, &index)?;
    Ok(id)
}

fn glob_ci(dir: &Path, prefix: &str, suffix: &str) -> Vec<PathBuf> {
    let (prefix, suffix) = (prefix.to_lowercase(), suffix.to_lowercase());
    let mut found: Vec<PathBuf> = fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .filter(|e| {
                    let name = e.file_name().to_string_lossy().to_lowercase();
                    name.starts_with(&prefix) && name.ends_with(&suffix)
                })
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default();
    found.sort();
    found
}

/// Fingerprints of every video file of this scene: the selected clip and its backups.
fn live_fingerprints(project: &Path, scene: u32) -> Vec<String> {
    let mut paths = glob_ci(&project.join("rendered_scene_videos"), &format!("video_{scene:04}-audio"), ".mp4");
    paths.extend(glob_ci(&project.join("rendered_scene_videos_backup").join(format!("scene_{scene:04}")), "", ".mp4"));
    paths.iter().map(|p| video_fingerprint(p)).filter(|f| !f.is_empty()).collect()
}

/// Tie the pending latent to the collected final video, and drop takes whose video no longer exists.
pub fn attach_video(project: &Path, scene: u32, video: &Path, now: &Stamp) -> io::Result<Option<String>> {
    let latents_dir = project.join("latents");
    let folder = folder(&latents_dir, scene);
    let mut index = read_index(&folder);
    let pending = text(index.get("pending"));
    if pending.is_empty() || !take_files(&folder, &pending)[0].is_file() {
        return Ok(None);
    }
    let fingerprint = video_fingerprint(video);
    if fingerprint.is_empty() {
        return Ok(None);
    }
    let mut kept: Vec<Value> = Vec::new();
    for take in index["takes"].as_array().cloned().unwrap_or_default() {
        if take.get("video_fp").and_then(Value::as_str) == Some(fingerprint.as_str()) {
            remove_take_files(&folder, &text(take.get("id")));
        } else {
            kept.push(take);
        }
    }
    kept.push(json!({
        "id": pending,
        "video_fp": fingerprint,
        "video_name": video.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        "saved_at": now.iso,
    }));
    index["pending"] = json!("");
    let live = live_fingerprints(project, scene);
    if !live.is_empty() {
        kept.retain(|take| {
            let keep = take.get("video_fp").and_then(Value::as_str).is_some_and(|fp| live.iter().any(|l| l == fp));
            if !keep {
                remove_take_files(&folder, &text(take.get("id")));
            }
            keep
        });
    }
    index["takes"] = Value::Array(kept);
    write_index(&folder, &index)?;
    Ok(Some(pending))
}

/// A clip was rewritten in place (for example by the opening color match): keep its take.
pub fn rename_video(project: &Path, scene: u32, old_fingerprint: &str, new_fingerprint: &str) -> io::Result<()> {
    if old_fingerprint.is_empty() || new_fingerprint.is_empty() || old_fingerprint == new_fingerprint {
        return Ok(());
    }
    let folder = folder(&project.join("latents"), scene);
    let mut index = read_index(&folder);
    let mut changed = false;
    if let Some(takes) = index["takes"].as_array_mut() {
        for take in takes {
            if take.get("video_fp").and_then(Value::as_str) == Some(old_fingerprint) {
                take["video_fp"] = json!(new_fingerprint);
                changed = true;
            }
        }
    }
    if changed {
        write_index(&folder, &index)?;
    }
    Ok(())
}

/// The archived takes of a scene, oldest first.
pub fn list_takes(project: &Path, scene: u32) -> Vec<Value> {
    let folder = folder(&project.join("latents"), scene);
    read_index(&folder)["takes"]
        .as_array()
        .map(|takes| {
            takes
                .iter()
                .map(|t| json!({"id": t.get("id").cloned().unwrap_or(Value::Null), "video_name": text(t.get("video_name")), "saved_at": text(t.get("saved_at"))}))
                .collect()
        })
        .unwrap_or_default()
}

/// Make the latent of the take that produced `video` the scene's active latent. `status` is `activated` or
/// `already_active` when the take was found, `no_archive` when the scene has takes but none belongs to the video, and
/// `no_takes` when the scene predates per take latents (the active latent is then left alone).
pub fn activate_for_video(project: &Path, scene: u32, video: Option<&Path>) -> io::Result<Value> {
    let latents_dir = project.join("latents");
    let folder = folder(&latents_dir, scene);
    let takes = read_index(&folder)["takes"].as_array().cloned().unwrap_or_default();
    if takes.is_empty() {
        return Ok(json!({"status": "no_takes", "take_count": 0}));
    }
    let fingerprint = video.map(video_fingerprint).unwrap_or_default();
    let take = takes.iter().find(|t| !fingerprint.is_empty() && t.get("video_fp").and_then(Value::as_str) == Some(fingerprint.as_str()));
    let Some(take) = take else { return Ok(json!({"status": "no_archive", "take_count": takes.len()})) };
    let id = text(take.get("id"));
    let source = take_files(&folder, &id)[0].clone();
    if !source.is_file() {
        return Ok(json!({"status": "no_archive", "take_count": takes.len()}));
    }
    let active = latents_dir.join(format!("scene_{scene:03}.latent"));
    let mut result = json!({"take_id": take.get("id").cloned().unwrap_or(Value::Null), "take_count": takes.len(), "video_name": text(take.get("video_name"))});
    if active.is_file() && fs::read(&source)? == fs::read(&active)? {
        result["status"] = json!("already_active");
        return Ok(result);
    }
    fs::copy(&source, &active)?;
    let sidecar = PathBuf::from(format!("{}.json", source.display()));
    if sidecar.is_file() {
        fs::copy(&sidecar, PathBuf::from(format!("{}.json", active.display())))?;
    }
    result["status"] = json!("activated");
    Ok(result)
}

pub fn delete_scene_takes(latents_dir: &Path, scene: u32) {
    let _ = fs::remove_dir_all(folder(latents_dir, scene));
}

pub fn delete_all_takes(latents_dir: &Path) {
    let Ok(entries) = fs::read_dir(latents_dir) else { return };
    for entry in entries.filter_map(Result::ok) {
        if folder_number(&entry.file_name().to_string_lossy()).is_some() {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

/// Renumber the take folders of every scene from `first_scene` on, when scenes are inserted or removed.
pub fn shift_takes(latents_dir: &Path, first_scene: u32, delta: i32) {
    let Ok(entries) = fs::read_dir(latents_dir) else { return };
    let mut numbers: Vec<u32> = entries
        .filter_map(Result::ok)
        .filter_map(|e| folder_number(&e.file_name().to_string_lossy()))
        .filter(|n| *n >= first_scene)
        .collect();
    numbers.sort_unstable();
    numbers.dedup();
    if delta > 0 {
        numbers.reverse();
    }
    for number in numbers {
        let source = folder(latents_dir, number);
        let target = folder(latents_dir, (number as i64 + delta as i64) as u32);
        if target.is_dir() {
            let _ = fs::remove_dir_all(&target);
        }
        let _ = fs::rename(&source, &target);
    }
}

fn copy_dir(src: &Path, dst: &Path) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)?.filter_map(Result::ok) {
        let target = dst.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

pub fn copy_takes(source_latents_dir: &Path, target_latents_dir: &Path) {
    let Ok(entries) = fs::read_dir(source_latents_dir) else { return };
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if folder_number(&name).is_some() {
            let _ = copy_dir(&entry.path(), &target_latents_dir.join(&name));
        }
    }
}
