//! Branch a project under a new name. A simplified "Save Project As": the whole folder is copied and every path in the
//! session that pointed into the old folder now points into the new one. The pack's version can also leave out videos,
//! prompts and notes, which this one does not.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::atomic::atomic_write_json;
use crate::layout::{session_path, Stamp};
use crate::scene_ops::OpError;
use crate::session_ops::safe_project_name;

fn normcase(text: &str) -> String {
    if cfg!(windows) { text.replace('/', "\\").to_lowercase() } else { text.to_string() }
}

/// Replace the old project folder at the start of every string in `value` with the new one.
pub fn rebase_paths(value: &mut Value, from: &str, to: &str) -> usize {
    let from_key = normcase(from.trim_end_matches(['\\', '/']));
    let mut changed = 0;
    match value {
        Value::String(text) => {
            let key = normcase(text);
            let boundary = key.as_bytes().get(from_key.len()).is_none_or(|b| *b == b'\\' || *b == b'/');
            if key.starts_with(&from_key) && boundary {
                let rest = &text[from_key.len().min(text.len())..];
                let separator_fixed = if rest.is_empty() { String::new() } else { rest.to_string() };
                *text = format!("{}{}", to.trim_end_matches(['\\', '/']), separator_fixed);
                changed += 1;
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|v| changed += rebase_paths(v, from, to)),
        Value::Object(map) => map.values_mut().for_each(|v| changed += rebase_paths(v, from, to)),
        _ => {}
    }
    changed
}

fn copy_tree(source: &Path, target: &Path) -> io::Result<()> {
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let (from, to) = (entry.path(), target.join(entry.file_name()));
        if entry.file_type()?.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// Copy `source` to a new project folder under `root` named after `new_name`. Returns the new folder.
pub fn duplicate_project(root: &Path, source: &Path, new_name: &str, now: &Stamp) -> Result<PathBuf, OpError> {
    let clean = new_name.trim();
    if clean.is_empty() {
        return Err(OpError::Validation("A new project name is required.".into()));
    }
    if !session_path(source).is_file() {
        return Err(OpError::Validation("That folder is not a project.".into()));
    }
    fs::create_dir_all(root)?;
    let target = crate::scaffold::new_project(&root.join(safe_project_name(clean)))?;
    let source_abs = std::path::absolute(source).unwrap_or_else(|_| source.to_path_buf());
    let target_abs = std::path::absolute(&target).unwrap_or_else(|_| target.clone());
    if normcase(&target_abs.display().to_string()).starts_with(&normcase(&source_abs.display().to_string())) {
        let _ = fs::remove_dir_all(&target);
        return Err(OpError::Validation("The copy cannot be inside the project it copies.".into()));
    }
    copy_tree(source, &target)?;
    let text = fs::read_to_string(session_path(&target))?;
    let mut session: Value = serde_json::from_str(&text).map_err(|e| OpError::Validation(format!("The session file is not valid JSON: {e}")))?;
    rebase_paths(&mut session, &source_abs.display().to_string(), &target_abs.display().to_string());
    if let Some(map) = session.as_object_mut() {
        map.insert("project_name".into(), json!(clean));
        map.insert("project_folder".into(), json!(target_abs.display().to_string()));
        map.insert("revision".into(), json!(1));
        map.insert("builder_save_revision".into(), json!(1));
        map.insert("updated".into(), json!(now.epoch));
    }
    atomic_write_json(&session_path(&target), &session)?;
    Ok(target)
}
