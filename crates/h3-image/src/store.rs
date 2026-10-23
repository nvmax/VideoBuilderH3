//! Where presets live: the built-ins in the binary, the user's own as one JSON file each in a folder.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::builtin::builtin_presets;
use crate::detect::{parse_api_workflow, suggest_bindings};
use crate::preset::{ImagePreset, PresetOrigin};
use crate::ImageError;

pub struct PresetStore {
    dir: PathBuf,
}

fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('_') && !out.is_empty() {
            out.push('_');
        }
    }
    let out = out.trim_end_matches('_').to_string();
    if out.is_empty() { "preset".into() } else { out }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

impl PresetStore {
    pub fn new(dir: &Path) -> Self {
        Self { dir: dir.to_path_buf() }
    }

    fn file(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    /// Built-ins first, then the user's presets by name.
    pub fn list(&self) -> Vec<ImagePreset> {
        let mut all = builtin_presets();
        let mut custom: Vec<ImagePreset> = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| std::fs::read_to_string(e.path()).ok())
            .filter_map(|text| serde_json::from_str::<ImagePreset>(&text).ok())
            .filter(|p| p.origin == PresetOrigin::Custom)
            .collect();
        custom.sort_by_key(|p| p.name.to_lowercase());
        all.extend(custom);
        all
    }

    pub fn get(&self, id: &str) -> Option<ImagePreset> {
        self.list().into_iter().find(|p| p.id == id)
    }

    /// Save a custom preset. A built-in id is refused.
    pub fn save(&self, preset: &ImagePreset) -> Result<(), ImageError> {
        if !valid_id(&preset.id) {
            return Err(ImageError::Preset("A preset id uses lowercase letters, digits, - and _ (up to 64).".into()));
        }
        if builtin_presets().iter().any(|b| b.id == preset.id) {
            return Err(ImageError::Preset(format!("'{}' is a built-in preset and cannot be changed. Import a copy instead.", preset.id)));
        }
        if preset.origin != PresetOrigin::Custom {
            return Err(ImageError::Preset("Only custom presets are saved.".into()));
        }
        std::fs::create_dir_all(&self.dir)?;
        let text = serde_json::to_string_pretty(preset).map_err(|e| ImageError::Other(e.to_string()))?;
        let path = self.file(&preset.id);
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, text)?;
        std::fs::rename(&temp, &path)?;
        Ok(())
    }

    pub fn delete(&self, id: &str) -> Result<(), ImageError> {
        if builtin_presets().iter().any(|b| b.id == id) {
            return Err(ImageError::Preset("A built-in preset cannot be deleted.".into()));
        }
        if !valid_id(id) {
            return Err(ImageError::Preset("Invalid preset id.".into()));
        }
        match std::fs::remove_file(self.file(id)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(ImageError::NotFound(id.to_string())),
            Err(e) => Err(e.into()),
        }
    }

    /// Import an API format workflow as a new custom preset with guessed bindings.
    pub fn import(&self, name: &str, description: &str, workflow_text: &str) -> Result<ImagePreset, ImageError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(ImageError::Preset("A preset needs a name.".into()));
        }
        let workflow: Value = parse_api_workflow(workflow_text)?;
        let (bindings, output_nodes) = suggest_bindings(&workflow);
        if !bindings.iter().any(|b| b.key == "prompt") {
            return Err(ImageError::Preset("No text prompt node was found in this workflow. A CLIPTextEncode with typed text is expected.".into()));
        }
        let taken: Vec<String> = self.list().into_iter().map(|p| p.id).collect();
        let base = slug(name);
        let mut id = base.clone();
        let mut n = 2;
        while taken.contains(&id) {
            id = format!("{base}_{n}");
            n += 1;
        }
        let preset = ImagePreset { id, name: name.to_string(), description: description.to_string(), origin: PresetOrigin::Custom, workflow, bindings, output_nodes };
        self.save(&preset)?;
        Ok(preset)
    }
}
