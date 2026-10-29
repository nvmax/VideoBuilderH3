//! The transitions saved in the session (`scene_transitions`) for the agent API: read them, replace them, set or clear one. The key and the entries are the
//! ones the Video Builder's timeline saves, so a project an agent edited opens in the Builder with the transitions on its timeline.
//!
//! An entry is `{id, transition, start, seconds, params}`: a block of the timeline like an overlay, `start` seconds in and `seconds` long. On a cut between
//! two scenes it draws the one scene into the other, inside a scene it acts on the footage (see `h3_media::transition`). The names of the transitions are
//! checked by the caller, which knows the catalog. An entry of an earlier version, `{after_scene, transition, seconds, align, params}`, is read as the block
//! it stood for.

use std::path::Path;

use h3_core::pyval::{text_or_empty, Obj};
use serde_json::{json, Value};

use crate::layout::Stamp;
use crate::scene_ops::{current_revision, open, OpError};

pub const KEY: &str = "scene_transitions";
pub const MAX_TRANSITIONS: usize = 500;
const DEFAULT_SECONDS: f64 = 0.5;
const SECONDS_RANGE: (f64, f64) = (0.1, 10.0);
/// The frame rate of the timeline.
const FPS: f64 = 24.0;

/// The index of the scene a caller means: an id, or a scene number (1 = first scene). `None` when no scene is meant.
fn scene_index(segments: &[Value], key: &Value) -> Option<usize> {
    let key = match key {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) if n.is_u64() => n.to_string(),
        _ => return None,
    };
    if key.is_empty() {
        return None;
    }
    segments.iter().position(|s| text_or_empty(s.get("id")).trim() == key).or_else(|| key.parse::<usize>().ok().filter(|n| (1..=segments.len()).contains(n)).map(|n| n - 1))
}

fn seconds_of(segment: &Value, key: &str) -> f64 {
    segment.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

fn milli(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

/// Where the block of an earlier version's entry starts: its length, placed around the end of the scene before the cut the way `align` said.
fn start_on_cut(segments: &[Value], after: usize, seconds: f64, align: &str) -> f64 {
    let cut = seconds_of(&segments[after], "end");
    let frames = (seconds * FPS).round();
    let before = match align {
        "end" | "before" => frames,
        "start" | "after" => 0.0,
        _ => (frames / 2.0).floor(),
    };
    (cut - before / FPS).max(0.0)
}

fn entry_of(segments: &[Value], raw: &Value) -> Result<Value, String> {
    let map = raw.as_object().ok_or("A transition must be an object.")?;
    let name = map.get("transition").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty()).ok_or("A transition needs `transition`, its name.")?;
    let seconds = map.get("seconds").and_then(Value::as_f64).filter(|s| s.is_finite()).unwrap_or(DEFAULT_SECONDS).clamp(SECONDS_RANGE.0, SECONDS_RANGE.1);
    let start = match map.get("start") {
        Some(v) if !v.is_null() => v.as_f64().filter(|s| s.is_finite() && *s >= 0.0).ok_or("`start` must be a number of seconds, 0 or more.")?,
        _ => {
            let after = map.get("after_scene").filter(|v| !v.is_null()).ok_or("A transition needs `start`, its time on the timeline in seconds (or `after_scene`, to put it on a cut).")?;
            let index = scene_index(segments, after).ok_or_else(|| format!("Unknown scene {after}. `after_scene` is the number (1 = first) or id of the scene before the cut."))?;
            let align = map.get("align").and_then(Value::as_str).map(|a| a.trim().to_lowercase()).unwrap_or_default();
            start_on_cut(segments, index, seconds, &align)
        }
    };
    let params = map.get("params").filter(|p| p.is_object()).cloned().unwrap_or_else(|| json!({}));
    let id = map.get("id").and_then(Value::as_str).map(str::trim).unwrap_or_default();
    Ok(json!({"id": id, "transition": name, "start": milli(start), "seconds": milli(seconds), "params": params}))
}

/// The list as saved (the entries of `list` read against the project's scenes): every entry has an id of its own, in the order of the timeline. `strict` fails on an entry that cannot be read. Otherwise such an
/// entry (a deleted scene, say) is dropped, which is how a saved list is read.
pub fn normalized(segments: &[Value], list: &[Value], strict: bool) -> Result<Vec<Value>, String> {
    let mut out: Vec<Value> = Vec::new();
    for (n, raw) in list.iter().enumerate() {
        match entry_of(segments, raw) {
            Ok(entry) => out.push(entry),
            Err(e) if strict => return Err(format!("Transition {}: {e}", n + 1)),
            Err(_) => {}
        }
    }
    if out.len() > MAX_TRANSITIONS {
        return Err(format!("A project holds at most {MAX_TRANSITIONS} transitions."));
    }
    let mut used: Vec<String> = Vec::new();
    for entry in &mut out {
        let given = entry.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
        let id = if !given.is_empty() && !used.contains(&given) {
            given
        } else {
            (1..).map(|n| format!("t{n}")).find(|candidate| !used.contains(candidate) && !list.iter().any(|raw| raw.get("id").and_then(Value::as_str) == Some(candidate.as_str()))).unwrap_or_default()
        };
        used.push(id.clone());
        entry["id"] = json!(id);
    }
    out.sort_by(|a, b| seconds_of(a, "start").total_cmp(&seconds_of(b, "start")));
    Ok(out)
}

fn segments_of(session: &Obj) -> Vec<Value> {
    session.get("segments").and_then(Value::as_array).cloned().unwrap_or_default()
}

/// The saved entries, read the way a stitch reads them.
pub fn saved(session: &Obj) -> Vec<Value> {
    let list = session.get(KEY).and_then(Value::as_array).cloned().unwrap_or_default();
    normalized(&segments_of(session), &list, false).unwrap_or_default()
}

/// The transitions of a loaded session as the stitch sees them, with the project's revision. `end` is `start + seconds`.
pub fn transitions_view(session: &Obj) -> Value {
    let list: Vec<Value> = saved(session)
        .into_iter()
        .map(|mut entry| {
            entry["end"] = json!(milli(seconds_of(&entry, "start") + seconds_of(&entry, "seconds")));
            entry
        })
        .collect();
    json!({"scene_transitions": list, "count": list.len(), "revision": current_revision(session)})
}

fn write(mut edit: crate::scene_ops::Edit, list: Vec<Value>, project: &Path, now: &Stamp) -> Result<Value, OpError> {
    edit.session.insert(KEY.into(), Value::Array(list));
    edit.save(now)?;
    let loaded = crate::session::load_session(project)?;
    Ok(transitions_view(&loaded.session))
}

/// Replace the whole list. What is saved is what the normalizer made of it, and the reply shows that.
pub fn put_transitions(project: &Path, list: &[Value], if_match: Option<i64>, now: &Stamp) -> Result<Value, OpError> {
    let edit = open(project, if_match)?;
    let normalized = normalized(&segments_of(&edit.session), list, true).map_err(OpError::Validation)?;
    write(edit, normalized, project, now)
}

/// Whether the block `[start, start + seconds]` covers or touches the end of scene `index`, the cut after it.
fn touches_cut(segments: &[Value], index: usize, entry: &Value) -> bool {
    let cut = seconds_of(&segments[index], "end");
    let start = seconds_of(entry, "start");
    let end = start + seconds_of(entry, "seconds");
    let slack = 0.5 / FPS;
    start - slack <= cut && cut <= end + slack
}

/// Add a transition, change one, or remove one. `id` names an existing entry to change (only the fields sent change) or removes it when `transition` is
/// null, empty or `none`. Without an `id` a new entry is added at `start` (or on the cut after `after_scene`, placed by `align`), and a clear with
/// `after_scene` removes the entries that cover that cut. The reply is the whole list.
pub fn set_transition(project: &Path, fields: &Obj, if_match: Option<i64>, now: &Stamp) -> Result<Value, OpError> {
    let edit = open(project, if_match)?;
    let segments = segments_of(&edit.session);
    let mut list = saved(&edit.session);
    let id = fields.get("id").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let clear = match fields.get("transition") {
        None => id.is_none() && !fields.contains_key("start"),
        Some(Value::Null) => true,
        Some(Value::String(s)) => s.trim().is_empty() || s.trim().eq_ignore_ascii_case("none"),
        Some(_) => false,
    };
    let existing = id.as_ref().and_then(|id| list.iter().position(|e| e.get("id").and_then(Value::as_str) == Some(id.as_str())));
    if clear {
        match (&id, existing) {
            (Some(id), None) => return Err(OpError::Validation(format!("No transition has the id `{id}`."))),
            (Some(_), Some(at)) => {
                list.remove(at);
            }
            (None, _) => {
                let after = fields.get("after_scene").and_then(|v| scene_index(&segments, v)).ok_or_else(|| {
                    OpError::Validation("To clear, send the `id` of a transition, or `after_scene`, the number (1 = first) or id of the scene before the cut.".into())
                })?;
                list.retain(|e| !touches_cut(&segments, after, e));
            }
        }
    } else {
        let mut sent: Obj = existing.and_then(|at| list[at].as_object().cloned()).unwrap_or_default();
        // A new place replaces the old one, whichever way it is given.
        if fields.contains_key("start") || fields.contains_key("after_scene") {
            sent.remove("start");
        }
        for (key, value) in fields {
            if key != "id" || id.is_some() {
                sent.insert(key.clone(), value.clone());
            }
        }
        if fields.contains_key("start") {
            sent.remove("after_scene");
        }
        let entry = entry_of(&segments, &Value::Object(sent)).map_err(OpError::Validation)?;
        match existing {
            Some(at) => list[at] = entry,
            None => list.push(entry),
        }
    }
    let list = normalized(&segments, &list, false).map_err(OpError::Validation)?;
    write(edit, list, project, now)
}
