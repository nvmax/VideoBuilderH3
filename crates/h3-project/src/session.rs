//! Load and save the Video Builder session. Port of `_load_builder_session` and `_save_builder_session_unlocked` in
//! `builder/project.py`.
//!
//! The session stays a loose JSON object with its key order, so keys this app does not know (the Python Builder has
//! about 130 top level keys) pass through a save unchanged. Saves use the same revision rule as the pack: the saved
//! `revision` goes up by one, and a snapshot older than the saved one is ignored.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use h3_core::pyval::{to_float, truthy, Obj};
use serde_json::{json, Value};

use crate::atomic::{atomic_write_json, atomic_write_text};
use crate::backups::backup_session_file;
use crate::derived;
use crate::layout::{context_folder, images_folder, prompts_folder, session_path, srt_path, Stamp};
use crate::pyjson;

/// A session read from disk.
#[derive(Debug, Clone)]
pub struct LoadedSession {
    pub project_folder: PathBuf,
    pub session_path: PathBuf,
    pub revision: i64,
    pub session: Obj,
}

/// `int(value or 0)` for a loose JSON value.
fn int_of(value: Option<&Value>) -> i64 {
    value.filter(|v| truthy(v)).and_then(to_float).map_or(0, |f| f as i64)
}

fn absolute(path: &Path) -> PathBuf {
    h3_core::abspath::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

pub fn read_session_file(path: &Path) -> io::Result<Obj> {
    let text = fs::read_to_string(path)?;
    match pyjson::parse(&text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {e}", path.display())))? {
        Value::Object(map) => Ok(map),
        _ => Err(io::Error::new(io::ErrorKind::InvalidData, "Builder session is not a JSON object.")),
    }
}

/// Load a project's session. A session saved before revisions existed gets `revision` from `builder_save_revision`.
/// Timeline notes missing from the session are filled from `SceneNotes.json`, as the pack does.
pub fn load_session(project: &Path) -> io::Result<LoadedSession> {
    load_with(project, false)
}

/// The load the Video Builder does (`_load_builder_session`): the same, after the session's file paths are repaired
/// for a moved or imported project (see `rehydrate`).
pub fn load_builder_session(project: &Path) -> io::Result<LoadedSession> {
    load_with(project, true)
}

fn load_with(project: &Path, rehydrate: bool) -> io::Result<LoadedSession> {
    let folder = absolute(project);
    let path = session_path(&folder);
    if !path.is_file() {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("Builder session was not found: {}", path.display())));
    }
    let mut session = read_session_file(&path)?;
    if rehydrate {
        session = crate::rehydrate::rehydrate_session(&folder, session);
    }
    let fallbacks = derived::load_scene_notes(&folder);
    if !fallbacks.is_empty() {
        if let Some(Value::Array(segments)) = session.get_mut("segments") {
            for (i, segment) in segments.iter_mut().enumerate() {
                let Some(segment) = segment.as_object_mut() else { continue };
                let empty = h3_core::pyval::text_or_empty(segment.get("timeline_note")).trim().is_empty();
                if let (true, Some(note)) = (empty, fallbacks.get(&(i as u32 + 1))) {
                    if !note.is_empty() {
                        segment.insert("timeline_note".into(), json!(note));
                    }
                }
            }
        }
    }
    if !session.contains_key("revision") {
        let fallback = match int_of(session.get("builder_save_revision")) {
            0 => 1,
            n => n,
        };
        session.insert("revision".into(), json!(fallback));
    }
    let revision = match int_of(session.get("revision")) {
        0 => 1,
        n => n,
    };
    Ok(LoadedSession { project_folder: folder, session_path: path, revision, session })
}

/// Everything a save needs besides the project folder.
pub struct SaveInput {
    /// The session to save, as the caller edited it.
    pub session: Obj,
    /// Audio file path stored in the session (already inside the project when it was copied).
    pub audio_path: String,
    pub now: Stamp,
}

/// What a save did.
#[derive(Debug, Clone)]
pub struct SaveOutcome {
    /// The snapshot was older than the saved session and was ignored. `session` is then the saved one.
    pub stale: bool,
    pub revision: i64,
    pub project_folder: PathBuf,
    pub session_path: PathBuf,
    pub session: Obj,
}

/// Assign unique slot numbers (10001 and up) to overlay scenes.
pub(crate) fn assign_overlay_slots(overlay: &mut [Value]) {
    let slot_of = |segment: &Obj, fallback: i64| -> i64 {
        for key in ["overlay_slot_number", "scene_slot_number", "slot_number"] {
            let value = segment.get(key).filter(|v| truthy(v)).and_then(to_float).map_or(0, |f| f as i64);
            if value >= 10001 {
                return value;
            }
        }
        10000 + fallback.max(1)
    };
    let existing: Vec<i64> = overlay
        .iter()
        .filter_map(Value::as_object)
        .map(|s| slot_of(s, 0))
        .filter(|v| *v >= 10001)
        .collect();
    let mut next_slot = existing.iter().copied().max().unwrap_or(10000).max(10000) + 1;
    let mut used: Vec<i64> = Vec::new();
    for (i, segment) in overlay.iter_mut().enumerate() {
        let index = i as i64 + 1;
        let Some(segment) = segment.as_object_mut() else { continue };
        let mut slot = slot_of(segment, index);
        if used.contains(&slot) {
            slot = next_slot.max(10000 + index);
            while used.contains(&slot) {
                slot += 1;
            }
            next_slot = slot + 1;
        }
        segment.insert("overlay_slot_number".into(), json!(slot));
        used.push(slot);
    }
}

/// Saves from this process run one at a time, like the pack's save lock, so two renders finishing together cannot
/// interleave their file replacements into a mixed project.
static SAVE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Run `f` while holding the save lock, so a Storyboard save and a session save never write at the same time.
pub fn with_save_lock<R>(f: impl FnOnce() -> R) -> R {
    let _guard = SAVE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    f()
}

/// A save step that may change the session and the audio path that is stored.
pub type PrepareHook<'a> = &'a dyn Fn(&Path, &mut Obj, &mut String) -> io::Result<()>;

/// A save step that only reads the finished session.
pub type CommitHook<'a> = &'a dyn Fn(&Path, &Obj) -> io::Result<()>;

/// Extra steps of the Video Builder's own save, run inside the save at the points where the pack runs them.
#[derive(Default)]
pub struct SaveHooks<'a> {
    /// After the revision checks, before anything is written: copy the song and context files into the project.
    pub prepare: Option<PrepareHook<'a>>,
    /// After the derived files, right before the session is committed (context files, model defaults).
    pub before_commit: Option<CommitHook<'a>>,
    /// The Video Builder window made this save itself, so no other window needs telling (see `changes`).
    pub quiet: bool,
}

/// Save the session and its derived files. The session file is written last, so a failed save never advertises a
/// half written project.
pub fn save_session(project: &Path, input: SaveInput) -> io::Result<SaveOutcome> {
    save_session_with(project, input, &SaveHooks::default())
}

/// `save_session` with the Builder's extra steps.
pub fn save_session_with(project: &Path, input: SaveInput, hooks: &SaveHooks) -> io::Result<SaveOutcome> {
    let _guard = SAVE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let folder = absolute(project);
    for dir in [folder.clone(), images_folder(&folder), prompts_folder(&folder), context_folder(&folder)] {
        fs::create_dir_all(dir)?;
    }
    let mut session = input.session;
    let path = session_path(&folder);
    let incoming = int_of(session.get("builder_save_revision"));
    let mut server_revision = 0;
    // The session as it was, kept to say what this save changed.
    let mut previous: Option<Obj> = None;
    if path.is_file() {
        match read_session_file(&path) {
            Ok(existing) => {
                let existing_revision = int_of(existing.get("builder_save_revision"));
                server_revision = match int_of(existing.get("revision")) {
                    0 => existing_revision,
                    n => n,
                };
                if incoming > 0 && existing_revision > incoming {
                    eprintln!(
                        "[VRGDG-RS Project] Ignored stale session snapshot: incoming revision {incoming} < saved revision {existing_revision}."
                    );
                    return Ok(SaveOutcome { stale: true, revision: server_revision, project_folder: folder, session_path: path, session: existing });
                }
                previous = Some(existing);
            }
            Err(e) => eprintln!("[VRGDG-RS Project] Save revision check skipped: {e}"),
        }
    }
    let revision = server_revision + 1;
    session.insert("revision".into(), json!(revision));

    // An autosave can race a reference builder update. A missing catalog keeps the last known one.
    if session.get("flux_reference_builder").is_none_or(Value::is_null) && path.is_file() {
        if let Some(refs) = read_session_file(&path).ok().and_then(|p| p.get("flux_reference_builder").cloned()) {
            if refs.is_object() {
                session.insert("flux_reference_builder".into(), refs);
            }
        }
    }
    if let Some(overlay) = session.get("overlay_segments") {
        if !overlay.is_array() {
            session.insert("overlay_segments".into(), json!([]));
        }
    }
    if let Some(Value::Array(overlay)) = session.get_mut("overlay_segments") {
        assign_overlay_slots(overlay);
    }
    let mut audio_path = input.audio_path;
    if let Some(prepare) = hooks.prepare {
        prepare(&folder, &mut session, &mut audio_path)?;
    }
    let segments: Vec<Value> = session.get("segments").and_then(Value::as_array).cloned().unwrap_or_default();
    session.insert("audio_path".into(), json!(audio_path));
    session.insert("project_folder".into(), json!(folder.display().to_string()));
    session.insert("updated".into(), json!(input.now.epoch));
    session.insert("segments".into(), Value::Array(segments.clone()));

    let lyric_source = session.get("lyric_mapper").and_then(|m| m.get("source_text")).cloned();
    derived::save_full_lyrics(&folder, lyric_source.as_ref())?;
    backup_session_file(&folder, &input.now)?;
    atomic_write_text(&srt_path(&folder), &derived::segments_to_srt(&segments))?;
    if let Some(refs) = session.get("flux_reference_builder") {
        derived::save_reference_descriptions(&folder, refs)?;
    }
    derived::write_minimax_index(&folder, &session)?;
    derived::write_scene_notes(&folder, &segments)?;
    let overlay: Vec<Value> = session.get("overlay_segments").and_then(Value::as_array).cloned().unwrap_or_default();
    derived::write_prompt_files(&folder, &segments, &overlay)?;
    if let Some(before_commit) = hooks.before_commit {
        before_commit(&folder, &session)?;
    }
    // The session is the commit marker: every derived file is replaced first, the session last.
    atomic_write_json(&path, &Value::Object(session.clone()))?;
    if !hooks.quiet && crate::changes::listening_to(&folder) {
        crate::changes::announce(&folder, previous.as_ref(), &session);
    }
    Ok(SaveOutcome { stale: false, revision, project_folder: folder, session_path: path, session })
}

/// Save an edited session the way the pack's API does: the revision goes one above both counters (the UI counts saves in
/// `builder_save_revision`, the server in `revision`, and they can drift apart), so the save is never mistaken for a
/// stale snapshot. A save the server discarded is an error, never reported as success.
///
/// An open Video Builder window numbers its next save one above its last. The save number is left one higher than the
/// revision, so that save is refused as stale (a save that equals the saved number is accepted) until the window has reloaded,
/// and does not overwrite this edit with its older snapshot.
pub fn persist_session(project: &Path, mut session: Obj, now: Stamp) -> io::Result<SaveOutcome> {
    let next = int_of(session.get("revision")).max(int_of(session.get("builder_save_revision"))) + 1;
    session.insert("revision".into(), json!(next));
    session.insert("builder_save_revision".into(), json!(next + 1));
    let folder = absolute(project);
    session.insert("project_folder".into(), json!(folder.display().to_string()));
    let audio_path = h3_core::pyval::text_or_empty(session.get("audio_path"));
    let outcome = save_session(project, SaveInput { session, audio_path, now })?;
    if outcome.stale {
        return Err(io::Error::other(format!(
            "The project changed while this was running (saved revision {}, this save {next}). Nothing was written.",
            outcome.revision
        )));
    }
    Ok(outcome)
}
