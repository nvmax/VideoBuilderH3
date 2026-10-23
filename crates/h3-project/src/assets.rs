//! Per scene file numbering. Port of `_shift_scene_assets` and the renumber helpers in `builder/project.py`.
//!
//! Files in the scene folders are numbered by base scene position (`video_0003-audio.mp4`, `image_0003.png`). When a
//! scene is inserted, split off or removed, every later scene's files shift by one so file numbers keep equal to scene
//! positions. The renamed pairs are returned so callers can update the paths they store.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::latents;
use crate::layout::{renumbered_asset_name, scene_asset_number, SCENE_ASSET_FOLDERS};

const MAX_REMOVED_SCENE_ASSETS: usize = 20;

pub type Renamed = Vec<(PathBuf, PathBuf)>;

fn names(dir: &Path) -> Vec<String> {
    fs::read_dir(dir)
        .map(|rd| rd.filter_map(Result::ok).map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default()
}

/// Renumber every per scene file numbered `first` or later by `delta` (+1 or -1). Shifting up renames from the
/// highest number down so no file lands on one that has not moved yet.
pub fn shift_scene_assets(project: &Path, first: u32, delta: i32) -> io::Result<Renamed> {
    let mut renamed = Renamed::new();
    for folder_name in SCENE_ASSET_FOLDERS {
        let folder = project.join(folder_name);
        if !folder.is_dir() {
            continue;
        }
        let mut entries: Vec<(u32, String)> = names(&folder).into_iter().map(|n| (scene_asset_number(&n), n)).collect();
        entries.sort();
        if delta > 0 {
            entries.reverse();
        }
        for (number, name) in entries {
            if number < first {
                continue;
            }
            let new_number = (number as i64 + delta as i64) as u32;
            let source = folder.join(&name);
            let target = folder.join(renumbered_asset_name(&name, new_number));
            fs::rename(&source, &target)?;
            renamed.push((source.clone(), target.clone()));
            if target.is_dir() {
                for inner in names(&target) {
                    if scene_asset_number(&inner) == number {
                        let inner_target = target.join(renumbered_asset_name(&inner, new_number));
                        fs::rename(target.join(&inner), &inner_target)?;
                        renamed.push((source.join(&inner), inner_target));
                    }
                }
            }
        }
    }
    Ok(renamed)
}

fn modified(path: &Path) -> SystemTime {
    fs::metadata(path).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH)
}

/// Keep the newest 20 archived removed scenes.
pub fn prune_removed_scene_assets(project: &Path, max_keep: usize) {
    let root = project.join("removed_scene_assets");
    let Ok(entries) = fs::read_dir(&root) else { return };
    let mut dirs: Vec<(SystemTime, PathBuf)> =
        entries.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.is_dir()).map(|p| (modified(&p), p)).collect();
    dirs.sort_by_key(|(time, _)| *time);
    while dirs.len() > max_keep {
        let (_, old) = dirs.remove(0);
        let _ = fs::remove_dir_all(old);
    }
}

fn move_path(source: &Path, target: &Path) -> io::Result<()> {
    if fs::rename(source, target).is_ok() {
        return Ok(());
    }
    // Different drives: copy then remove.
    if source.is_dir() {
        copy_dir(source, target)?;
        fs::remove_dir_all(source)
    } else {
        fs::copy(source, target)?;
        fs::remove_file(source)
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

/// A base scene was removed. Its own files move to `removed_scene_assets/scene_NNNN_<stamp>/`, then every later
/// scene's files shift down one number. `stamp` is the `%Y%m%d_%H%M%S` time of the removal.
pub fn renumber_after_removal(project: &Path, removed: u32, stamp: &str) -> io::Result<Renamed> {
    let archive = project.join("removed_scene_assets").join(format!("scene_{removed:04}_{stamp}"));
    for folder_name in SCENE_ASSET_FOLDERS {
        let folder = project.join(folder_name);
        if !folder.is_dir() {
            continue;
        }
        for name in names(&folder) {
            if scene_asset_number(&name) == removed {
                fs::create_dir_all(archive.join(folder_name))?;
                move_path(&folder.join(&name), &archive.join(folder_name).join(&name))?;
            }
        }
    }
    prune_removed_scene_assets(project, MAX_REMOVED_SCENE_ASSETS);
    let renamed = shift_scene_assets(project, removed + 1, -1)?;
    latents::delete_latent(project, removed, false);
    latents::reindex_latents(project, removed);
    Ok(renamed)
}

/// A base scene was inserted or split off at `inserted`. That scene and every later one shift up one number, so the
/// new scene starts with no files.
pub fn renumber_after_insert(project: &Path, inserted: u32) -> io::Result<Renamed> {
    let renamed = shift_scene_assets(project, inserted, 1)?;
    latents::make_room_for_scene(project, inserted);
    Ok(renamed)
}
