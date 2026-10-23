//! The masked mix an enabled Audio Mask renders with. Port of `audio_override_for_scene` in `builder/audio_stems.py`.
//! Splitting a scene into stems (Demucs) and building the mix stay in the Video Builder. This app uses what it saved
//! in `<project>/audio_masks/<scene id>/`.

use std::path::{Path, PathBuf};

use h3_core::pyval::{to_float, truthy, Obj};
use serde_json::Value;

/// The audio a masked scene renders with.
#[derive(Debug, Clone, PartialEq)]
pub struct MaskedAudio {
    pub path: PathBuf,
    pub duration_seconds: f64,
}

fn valid_scene_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 80 && id != "." && id != ".." && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// The scene's masked mix, or `None` when its Audio Mask is off. Errors when the mask is on but the mix is missing or no
/// longer matches the scene's length, so a render never uses audio that does not fit.
pub fn audio_override_for_scene(segment: &Obj, project: &Path, duration: f64) -> Result<Option<MaskedAudio>, String> {
    let enabled = segment.get("audio_mask").and_then(Value::as_object).is_some_and(|m| m.get("enabled").is_some_and(truthy));
    if !enabled {
        return Ok(None);
    }
    let id = segment.get("id").and_then(Value::as_str).unwrap_or("").trim().to_string();
    if !valid_scene_id(&id) {
        return Err("Scene id is missing or not valid.".into());
    }
    let folder = project.join("audio_masks").join(&id);
    let meta: Value = std::fs::read_to_string(folder.join("mask.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
    let mix = meta.get("mix").and_then(Value::as_object);
    let path = folder.join("masked_mix.wav");
    let Some(mix) = mix.filter(|_| path.is_file()) else {
        return Err("This scene's Audio Mask is on but its masked mix has not been built. Open Audio Mask and build it, or turn the mask off.".into());
    };
    let built = mix.get("duration_seconds").and_then(to_float).unwrap_or(0.0);
    if (built - duration).abs() > 0.02 {
        return Err("This scene's length changed after its Audio Mask was built. Open Audio Mask, split again and rebuild.".into());
    }
    Ok(Some(MaskedAudio { path, duration_seconds: duration }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn seg(v: Value) -> Obj {
        v.as_object().unwrap().clone()
    }

    fn project(duration: f64, with_wav: bool) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("audio_masks").join("s1");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("mask.json"), json!({"mix": {"duration_seconds": duration}}).to_string()).unwrap();
        if with_wav {
            std::fs::write(folder.join("masked_mix.wav"), b"x").unwrap();
        }
        dir
    }

    #[test]
    fn off_masks_use_the_song() {
        let dir = project(4.0, true);
        assert_eq!(audio_override_for_scene(&seg(json!({"id": "s1"})), dir.path(), 4.0).unwrap(), None);
        assert_eq!(audio_override_for_scene(&seg(json!({"id": "s1", "audio_mask": {"enabled": false}})), dir.path(), 4.0).unwrap(), None);
    }

    #[test]
    fn an_enabled_mask_uses_the_mix() {
        let dir = project(4.0, true);
        let got = audio_override_for_scene(&seg(json!({"id": "s1", "audio_mask": {"enabled": true}})), dir.path(), 4.01).unwrap().unwrap();
        assert!(got.path.ends_with("masked_mix.wav"));
        assert_eq!(got.duration_seconds, 4.01);
    }

    #[test]
    fn a_missing_or_stale_mix_is_refused() {
        let on = seg(json!({"id": "s1", "audio_mask": {"enabled": true}}));
        let missing = project(4.0, false);
        assert!(audio_override_for_scene(&on, missing.path(), 4.0).unwrap_err().contains("not been built"));
        let stale = project(4.0, true);
        assert!(audio_override_for_scene(&on, stale.path(), 5.0).unwrap_err().contains("length changed"));
        assert!(audio_override_for_scene(&seg(json!({"id": "../x", "audio_mask": {"enabled": true}})), stale.path(), 4.0).is_err());
    }
}
