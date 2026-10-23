//! Timed Timeline Notes saved in the session. Port of the note functions in `agent_api/mutations.py`.

use std::path::Path;

use h3_core::pyval::{to_float, truthy, Obj};
use h3_core::timeline_markers::{create_marker, delete_marker, normalize_markers, overlapping_scene_ids, update_marker, MarkerError};
use serde_json::{json, Value};

use crate::layout::Stamp;
use crate::scene_ops::{open, OpError};

#[derive(Debug, thiserror::Error)]
pub enum NoteError {
    #[error(transparent)]
    Op(#[from] OpError),
    #[error("Timeline note '{0}' was not found.")]
    NotFound(String),
}

impl From<std::io::Error> for NoteError {
    fn from(e: std::io::Error) -> Self {
        NoteError::Op(OpError::Io(e))
    }
}

fn new_id() -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    format!("mark_{}_{}", now.as_millis(), (now.subsec_nanos() as u64).wrapping_mul(2654435761) % 10000)
}

fn revision_of(session: &Obj) -> i64 {
    let pick = |key: &str| session.get(key).filter(|v| truthy(v)).and_then(to_float).map(|f| f as i64);
    pick("revision").or_else(|| pick("builder_save_revision")).unwrap_or(0)
}

fn segments_of(session: &Obj) -> Vec<Value> {
    session.get("segments").and_then(Value::as_array).map(|a| a.iter().filter(|s| s.is_object()).cloned().collect()).unwrap_or_default()
}

fn view(marker: &Obj, segments: &[Value]) -> Value {
    let mut out = marker.clone();
    out.insert("scene_ids".into(), json!(overlapping_scene_ids(marker, segments)));
    Value::Object(out)
}

fn invalid(e: MarkerError) -> NoteError {
    match e {
        MarkerError::Invalid(m) => NoteError::Op(OpError::Validation(m)),
        MarkerError::NotFound(id) => NoteError::NotFound(id),
    }
}

/// The notes, sorted by start, with the scenes each one overlaps.
pub fn list_notes(session: &Obj) -> Value {
    let notes = normalize_markers(session.get("timeline_markers"), &mut new_id);
    let segments = segments_of(session);
    json!({"notes": notes.iter().map(|m| view(m, &segments)).collect::<Vec<_>>(), "count": notes.len(), "revision": revision_of(session)})
}

/// Apply `edit` to the notes under a revision check, then save.
fn edit_notes(project: &Path, if_match: Option<i64>, now: &Stamp, edit: impl FnOnce(&mut Vec<Obj>, &mut Obj) -> Result<(Option<Obj>, String), MarkerError>) -> Result<Value, NoteError> {
    let mut edit_session = open(project, None)?;
    let current = revision_of(&edit_session.session);
    if let Some(incoming) = if_match.filter(|i| *i != current) {
        return Err(OpError::Conflict { current, incoming }.into());
    }
    let mut notes = normalize_markers(edit_session.session.get("timeline_markers"), &mut new_id);
    let (note, _id) = edit(&mut notes, &mut edit_session.session).map_err(invalid)?;
    edit_session.session.insert("timeline_markers".into(), Value::Array(notes.iter().map(|m| Value::Object(m.clone())).collect()));
    let segments = segments_of(&edit_session.session);
    let note_view = note.as_ref().map_or(Value::Null, |n| view(n, &segments));
    let notes_view: Vec<Value> = notes.iter().map(|m| view(m, &segments)).collect();
    let revision = edit_session.save(now)?;
    Ok(json!({"note": note_view, "notes": notes_view, "revision": revision}))
}

pub fn create_note(project: &Path, fields: &Value, if_match: Option<i64>, now: &Stamp) -> Result<Value, NoteError> {
    edit_notes(project, if_match, now, |notes, _| {
        let note = create_marker(notes, fields, &mut new_id)?;
        let id = note.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        Ok((Some(note), id))
    })
}

pub fn update_note(project: &Path, id: &str, fields: &Value, if_match: Option<i64>, now: &Stamp) -> Result<Value, NoteError> {
    edit_notes(project, if_match, now, |notes, _| Ok((Some(update_marker(notes, id, fields)?), id.to_string())))
}

/// Remove a note. The Builder's active-note selection is cleared when it pointed at it. The answer has `deleted`.
pub fn delete_note(project: &Path, id: &str, if_match: Option<i64>, now: &Stamp) -> Result<Value, NoteError> {
    let mut result = edit_notes(project, if_match, now, |notes, session| {
        let removed = delete_marker(notes, id)?;
        if session.get("active_timeline_marker_id").and_then(Value::as_str) == Some(id) {
            session.insert("active_timeline_marker_id".into(), json!(""));
        }
        Ok((Some(removed), id.to_string()))
    })?;
    if let Some(map) = result.as_object_mut() {
        if let Some(note) = map.remove("note") {
            map.insert("deleted".into(), note);
        }
    }
    Ok(result)
}
