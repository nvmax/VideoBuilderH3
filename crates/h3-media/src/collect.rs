//! Put a finished clip into the project as `rendered_scene_videos/video_NNNN-audio.mp4`. Port of
//! `_collect_scene_video` and its file helpers in `runner/video_files.py`.
//!
//! A clip that is replaced is kept as a backup the timeline can switch back to. The render's latent is tied to the new
//! clip so choosing an older take later brings its latent back.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::{Duration, Instant};

use h3_project::takes;

use crate::tools::{MediaError, Tools};
use crate::video::{create_thumbnail, legacy_thumbnail_path, thumbnail_path_for};

/// What to do with a clip that is already there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Existing {
    Backup,
    Overwrite,
}

#[derive(Debug, Clone, Default)]
pub struct CollectResult {
    pub video_path: PathBuf,
    pub thumbnail_path: Option<PathBuf>,
    pub backup_path: Option<PathBuf>,
    pub backup_thumbnail_path: Option<PathBuf>,
    pub source_path: PathBuf,
}

fn is_locked(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::PermissionDenied || error.raw_os_error() == Some(32)
}

/// Retry a file operation that fails while another program holds the file (a player, a scanner).
fn retry<T>(what: &str, attempts: u32, delay: Duration, mut op: impl FnMut() -> io::Result<T>) -> Result<T, MediaError> {
    let mut last: Option<io::Error> = None;
    for attempt in 0..attempts.max(1) {
        match op() {
            Ok(value) => return Ok(value),
            Err(e) if is_locked(&e) => last = Some(e),
            Err(e) => return Err(MediaError::Io(e)),
        }
        if attempt + 1 < attempts {
            sleep(delay);
        }
    }
    Err(MediaError::Failed(format!("{what} failed because the file stayed locked: {}", last.map(|e| e.to_string()).unwrap_or_default())))
}

/// Wait until a file has a size, can be read, and stops growing.
fn wait_for_stable_file(path: &Path, timeout: Duration, interval: Duration) -> Result<(), MediaError> {
    let deadline = Instant::now() + timeout.max(Duration::from_millis(500));
    let (mut last_size, mut stable): (Option<u64>, u32) = (None, 0);
    let mut last_error: Option<io::Error> = None;
    while Instant::now() < deadline {
        match fs::metadata(path).and_then(|m| fs::File::open(path).map(|_| m.len())) {
            Ok(size) => {
                if size > 0 && Some(size) == last_size {
                    stable += 1;
                    if stable >= 2 {
                        return Ok(());
                    }
                } else {
                    stable = 0;
                    last_size = Some(size);
                }
            }
            Err(e) => {
                last_error = Some(e);
                stable = 0;
            }
        }
        sleep(interval);
    }
    match last_error {
        Some(_) => Err(MediaError::Failed(format!("Scene video is still locked and cannot be read: {}", path.display()))),
        None => Ok(()),
    }
}

/// Copy `source` over `target` through a temporary file, then remove `source`.
fn replace_file(source: &Path, target: &Path) -> Result<(), MediaError> {
    wait_for_stable_file(source, Duration::from_secs(20), Duration::from_millis(250))?;
    let mut temp = PathBuf::from(format!("{}.copying", target.display()));
    let mut index = 2;
    while temp.exists() {
        temp = PathBuf::from(format!("{}.copying_{index:02}", target.display()));
        index += 1;
    }
    let result = (|| -> Result<(), MediaError> {
        retry(&format!("Copying scene video to temporary file '{}'", temp.display()), 30, Duration::from_millis(250), || fs::copy(source, &temp).map(|_| ()))?;
        retry(&format!("Replacing scene video '{}'", target.display()), 30, Duration::from_millis(250), || fs::rename(&temp, target))
    })();
    let _ = fs::remove_file(&temp);
    result?;
    if let Err(e) = retry(&format!("Removing scratch scene video '{}'", source.display()), 8, Duration::from_millis(250), || fs::remove_file(source)) {
        eprintln!("[VRGDG-RS Media] Copied scene video but could not remove scratch source '{}': {e}", source.display());
    }
    Ok(())
}

/// `rendered_scene_videos/video_NNNN-audio.mp4`, with the previous clip kept in `rendered_scene_videos_backup` when
/// `existing` is `Backup`. `stamp` is `%Y%m%d_%H%M%S`. A source that is not an `-audio.mp4` is swapped for the newest
/// `-audio.mp4` in its folder.
pub fn collect_scene_video(tools: &Tools, project: &Path, scene: u32, source: &Path, existing: Existing, stamp: &str, now: &h3_project::Stamp) -> Result<CollectResult, MediaError> {
    let mut source = h3_core::abspath::absolute(source).unwrap_or_else(|_| source.to_path_buf());
    if !source.is_file() {
        return Err(MediaError::Failed(format!("Scene video was not found: {}", source.display())));
    }
    let target_dir = project.join("rendered_scene_videos");
    fs::create_dir_all(&target_dir)?;
    if !source.to_string_lossy().to_lowercase().ends_with("-audio.mp4") {
        let folder = source.parent().map(Path::to_path_buf).unwrap_or_default();
        let mut candidates: Vec<(std::time::SystemTime, PathBuf)> = fs::read_dir(&folder)?
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().to_lowercase().ends_with("-audio.mp4") && e.path().is_file())
            .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
            .collect();
        candidates.sort();
        if let Some((_, newest)) = candidates.pop() {
            source = newest;
        }
    }
    let target = target_dir.join(format!("video_{scene:04}-audio.mp4"));
    let target_thumb = thumbnail_path_for(&target);
    let legacy_thumb = legacy_thumbnail_path(&target);
    let mut backup_path = None;
    let mut backup_thumb = None;
    if source != h3_core::abspath::absolute(&target).unwrap_or_else(|_| target.clone()) {
        if target.exists() {
            if existing == Existing::Backup {
                let dir = project.join("rendered_scene_videos_backup").join(format!("scene_{scene:04}"));
                fs::create_dir_all(&dir)?;
                let mut path = dir.join(format!("video_{scene:04}-audio_{stamp}.mp4"));
                let mut index = 2;
                while path.exists() {
                    path = dir.join(format!("video_{scene:04}-audio_{stamp}_{index:02}.mp4"));
                    index += 1;
                }
                retry(&format!("Backing up existing scene video '{}'", target.display()), 30, Duration::from_millis(250), || move_file(&target, &path))?;
                if target_thumb.exists() {
                    let moved = thumbnail_path_for(&path);
                    if let Some(parent) = moved.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    retry("Backing up the existing scene video thumbnail", 30, Duration::from_millis(250), || move_file(&target_thumb, &moved))?;
                    backup_thumb = Some(moved);
                }
                if legacy_thumb.exists() {
                    retry("Removing the legacy scene video thumbnail", 30, Duration::from_millis(250), || fs::remove_file(&legacy_thumb))?;
                }
                backup_path = Some(path);
            } else {
                retry(&format!("Removing existing scene video '{}'", target.display()), 30, Duration::from_millis(250), || fs::remove_file(&target))?;
                for thumb in [&target_thumb, &legacy_thumb] {
                    if thumb.exists() {
                        let _ = retry("Removing the old scene video thumbnail", 30, Duration::from_millis(250), || fs::remove_file(thumb));
                    }
                }
            }
        }
        replace_file(&source, &target)?;
    }
    if legacy_thumb.exists() {
        let _ = retry("Removing the legacy scene video thumbnail", 30, Duration::from_millis(250), || fs::remove_file(&legacy_thumb));
    }
    let thumbnail_path = create_thumbnail(tools, &target, Some(&target_thumb));
    // The latent this render saved belongs to the video just collected.
    if let Err(e) = takes::attach_video(project, scene, &target, now) {
        eprintln!("[VRGDG-RS Latent Takes] Could not tie scene {scene:03}'s latent to its video: {e}");
    }
    Ok(CollectResult { video_path: target, thumbnail_path, backup_path, backup_thumbnail_path: backup_thumb, source_path: source })
}

fn move_file(from: &Path, to: &Path) -> io::Result<()> {
    if fs::rename(from, to).is_ok() {
        return Ok(());
    }
    fs::copy(from, to)?;
    fs::remove_file(from)
}
