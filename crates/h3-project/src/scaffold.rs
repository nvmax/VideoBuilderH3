//! New project folders. Port of `_new_builder_project` and `_unique_folder_path`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::atomic::atomic_write_text;
use crate::layout::{context_folder, images_folder, prompts_folder};

const CONTEXT_FILES: [&str; 6] =
    ["ConceptPrompts.txt", "I2VMotionNotes.txt", "themestyle.txt", "storyconcept.txt", "subjectsandscenes.txt", "full_lyrics.txt"];

/// `path`, or `path_002`, `path_003`, ... when it already exists.
pub fn unique_folder_path(path: &Path) -> io::Result<PathBuf> {
    if !path.exists() {
        return Ok(path.to_path_buf());
    }
    for index in 2..10000 {
        let candidate = PathBuf::from(format!("{}_{index:03}", path.display()));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(io::Error::other(format!("Could not find a free folder name for {}", path.display())))
}

/// Create the folders a project needs and the empty context files. Returns the project folder.
pub fn new_project(target: &Path) -> io::Result<PathBuf> {
    let target = unique_folder_path(target)?;
    for dir in [target.clone(), images_folder(&target), prompts_folder(&target), context_folder(&target), target.join("latents")] {
        fs::create_dir_all(dir)?;
    }
    for name in CONTEXT_FILES {
        let path = context_folder(&target).join(name);
        if !path.exists() {
            atomic_write_text(&path, "")?;
        }
    }
    Ok(target)
}
