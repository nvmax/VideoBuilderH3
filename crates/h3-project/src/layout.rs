//! Project folder layout and the clock the file names use. Mirrors `builder/paths.py`, `builder/audio.py` and the
//! scene asset naming in `builder/project.py`.

use std::path::{Path, PathBuf};

pub const SESSION_FILE: &str = "vrgdg_builder_session.json";
pub const SRT_FILE: &str = "builder_segments.srt";
pub const SCENE_NOTES_FILE: &str = "SceneNotes.json";

pub fn session_path(project: &Path) -> PathBuf {
    project.join(SESSION_FILE)
}

pub fn srt_path(project: &Path) -> PathBuf {
    project.join(SRT_FILE)
}

pub fn scene_notes_path(project: &Path) -> PathBuf {
    project.join(SCENE_NOTES_FILE)
}

pub fn images_folder(project: &Path) -> PathBuf {
    project.join("zimage_approved")
}

pub fn prompts_folder(project: &Path) -> PathBuf {
    project.join("prompts")
}

pub fn context_folder(project: &Path) -> PathBuf {
    project.join("project_context")
}

pub fn latents_folder(project: &Path) -> PathBuf {
    project.join("latents")
}

/// Every folder that holds files numbered by scene position. Insert track slots start at 10001 and never match.
pub const SCENE_ASSET_FOLDERS: [&str; 16] = [
    "rendered_scene_videos",
    "rendered_scene_videos_backup",
    "scene_video_thumbnails",
    "zimage_approved",
    "scene_image_previews",
    "scene_audio",
    "scene_audio_trimmed",
    "scene_srt",
    "project_context",
    "minimax_h3_scene_audio",
    "image_to_video_clips",
    "text_to_video_clips",
    "reference_to_video_clips",
    "ingredients_to_video_clips",
    "id_lora_i2v_clips",
    "first_last_frame_clips",
];

const ASSET_PREFIXES: [&str; 5] = ["video_", "image_", "audio_", "scene_audio_", "scene_"];

/// `(prefix, number text, rest)` for `^((?:video|image|audio|scene_audio|scene)_)(\d{4})(?!\d)`.
fn split_asset_name(name: &str) -> Option<(&'static str, &str, &str)> {
    for prefix in ASSET_PREFIXES {
        let Some(rest) = name.strip_prefix(prefix) else { continue };
        let bytes = rest.as_bytes();
        if bytes.len() >= 4 && bytes[..4].iter().all(u8::is_ascii_digit) && !bytes.get(4).is_some_and(u8::is_ascii_digit) {
            return Some((prefix, &rest[..4], &rest[4..]));
        }
    }
    None
}

/// Scene number in a per scene file name, or 0 when the name has none.
pub fn scene_asset_number(name: &str) -> u32 {
    split_asset_name(name).and_then(|(_, digits, _)| digits.parse().ok()).unwrap_or(0)
}

/// The name with its scene number replaced.
pub fn renumbered_asset_name(name: &str, number: u32) -> String {
    match split_asset_name(name) {
        Some((prefix, _, rest)) => format!("{prefix}{number:04}{rest}"),
        None => name.to_string(),
    }
}

/// The time stamps the pack puts in file names and JSON. Passed in so tests can fix the clock.
#[derive(Debug, Clone, PartialEq)]
pub struct Stamp {
    /// `%Y%m%d_%H%M%S`.
    pub compact: String,
    /// `%Y-%m-%dT%H:%M:%S`.
    pub iso: String,
    /// Seconds since the epoch, with fraction.
    pub epoch: f64,
}

impl Stamp {
    pub fn now() -> Self {
        let now = chrono::Local::now();
        Self {
            compact: now.format("%Y%m%d_%H%M%S").to_string(),
            iso: now.format("%Y-%m-%dT%H:%M:%S").to_string(),
            epoch: now.timestamp_millis() as f64 / 1000.0,
        }
    }

    /// Milliseconds since the epoch.
    pub fn millis(&self) -> u64 {
        (self.epoch * 1000.0) as u64
    }

    /// The last three digits of the millisecond clock, as in `int(time.time() * 1000) % 1000`.
    pub fn ms_part(&self) -> u32 {
        (self.millis() % 1000) as u32
    }
}
