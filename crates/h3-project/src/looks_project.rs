//! The effects (`video_effects`) and filters (`video_filters`) saved in the session, for the agent API: read them, replace them, add, change or remove one. The keys
//! and the entries are the ones the Video Builder's timeline saves, so a project an agent edited opens in the Builder with the blocks on its lanes.
//!
//! An entry is `{id, look, start, seconds, intensity, params}`: a block of the timeline, `start` seconds in and `seconds` long. `look` names the effect or filter
//! (the caller checks it against the catalog), `intensity` (0 to 1) is how much of its change is mixed in and `params` are the settings changed from their
//! defaults. Blocks may overlap: a frame gets every filter that covers it, then every effect.

use std::path::Path;

use h3_core::pyval::Obj;
use serde_json::{json, Value};

use crate::layout::Stamp;
use crate::scene_ops::{current_revision, open, OpError};

pub const KEY_EFFECTS: &str = "video_effects";
pub const KEY_FILTERS: &str = "video_filters";
pub const MAX_LOOKS: usize = 1000;
const SECONDS_RANGE: (f64, f64) = (0.05, 86400.0);
const DEFAULT_SECONDS: f64 = 1.0;

/// The two kinds of look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookKind {
    Effect,
    Filter,
}

impl LookKind {
    pub fn key(self) -> &'static str {
        match self {
            Self::Effect => KEY_EFFECTS,
            Self::Filter => KEY_FILTERS,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Effect => "effect",
            Self::Filter => "filter",
        }
    }

    fn prefix(self) -> char {
        match self {
            Self::Effect => 'e',
            Self::Filter => 'f',
        }
    }
}

fn milli(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

fn number(entry: &Value, key: &str) -> f64 {
    entry.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

fn entry_of(kind: LookKind, raw: &Value) -> Result<Value, String> {
    let map = raw.as_object().ok_or_else(|| format!("An {} must be an object.", kind.name()))?;
    let look = map.get("look").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| format!("An {} needs `look`, its name.", kind.name()))?;
    let start = map.get("start").and_then(Value::as_f64).filter(|s| s.is_finite() && *s >= 0.0).ok_or("It needs `start`, its time on the timeline in seconds, 0 or more.")?;
    let seconds = map.get("seconds").and_then(Value::as_f64).filter(|s| s.is_finite()).unwrap_or(DEFAULT_SECONDS).clamp(SECONDS_RANGE.0, SECONDS_RANGE.1);
    let intensity = map.get("intensity").and_then(Value::as_f64).filter(|s| s.is_finite()).unwrap_or(1.0).clamp(0.0, 1.0);
    let params = map.get("params").filter(|p| p.is_object()).cloned().unwrap_or_else(|| json!({}));
    let id = map.get("id").and_then(Value::as_str).map(str::trim).unwrap_or_default();
    Ok(json!({"id": id, "look": look, "start": milli(start), "seconds": milli(seconds), "intensity": milli(intensity), "params": params}))
}

/// The list as saved: every entry has an id of its own, in the order of the timeline. `strict` fails on an entry that cannot be read. Otherwise such an entry is
/// dropped, which is how a saved list is read.
pub fn normalized(kind: LookKind, list: &[Value], strict: bool) -> Result<Vec<Value>, String> {
    let mut out: Vec<Value> = Vec::new();
    for (n, raw) in list.iter().enumerate() {
        match entry_of(kind, raw) {
            Ok(entry) => out.push(entry),
            Err(e) if strict => return Err(format!("{} {}: {e}", kind.name(), n + 1)),
            Err(_) => {}
        }
    }
    if out.len() > MAX_LOOKS {
        return Err(format!("A project holds at most {MAX_LOOKS} {}s.", kind.name()));
    }
    let mut used: Vec<String> = Vec::new();
    for entry in &mut out {
        let given = entry.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
        let id = if !given.is_empty() && !used.contains(&given) {
            given
        } else {
            (1..)
                .map(|n| format!("{}{n}", kind.prefix()))
                .find(|candidate| !used.contains(candidate) && !list.iter().any(|raw| raw.get("id").and_then(Value::as_str) == Some(candidate.as_str())))
                .unwrap_or_default()
        };
        used.push(id.clone());
        entry["id"] = json!(id);
    }
    out.sort_by(|a, b| number(a, "start").total_cmp(&number(b, "start")));
    Ok(out)
}

/// The saved entries, read the way a stitch reads them.
pub fn saved(session: &Obj, kind: LookKind) -> Vec<Value> {
    let list = session.get(kind.key()).and_then(Value::as_array).cloned().unwrap_or_default();
    normalized(kind, &list, false).unwrap_or_default()
}

/// The effects or filters of a loaded session as the stitch sees them, with the project's revision. `end` is `start + seconds`.
pub fn looks_view(session: &Obj, kind: LookKind) -> Value {
    let list: Vec<Value> = saved(session, kind)
        .into_iter()
        .map(|mut entry| {
            entry["end"] = json!(milli(number(&entry, "start") + number(&entry, "seconds")));
            entry
        })
        .collect();
    json!({kind.key(): list, "count": list.len(), "revision": current_revision(session)})
}

fn write(mut edit: crate::scene_ops::Edit, kind: LookKind, list: Vec<Value>, project: &Path, now: &Stamp) -> Result<Value, OpError> {
    edit.session.insert(kind.key().into(), Value::Array(list));
    edit.save(now)?;
    let loaded = crate::session::load_session(project)?;
    Ok(looks_view(&loaded.session, kind))
}

/// Replace the whole list. What is saved is what the normalizer made of it, and the reply shows that.
pub fn put_looks(project: &Path, kind: LookKind, list: &[Value], if_match: Option<i64>, now: &Stamp) -> Result<Value, OpError> {
    let edit = open(project, if_match)?;
    let list = normalized(kind, list, true).map_err(OpError::Validation)?;
    write(edit, kind, list, project, now)
}

/// Add one, change one, or remove one, and leave the others alone. `id` names an existing entry to change (only the fields sent change) or to remove when `look`
/// is null, empty or `none`. Without an `id` a new entry is added.
pub fn set_look(project: &Path, kind: LookKind, fields: &Obj, if_match: Option<i64>, now: &Stamp) -> Result<Value, OpError> {
    let edit = open(project, if_match)?;
    let mut list = saved(&edit.session, kind);
    let id = fields.get("id").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let existing = id.as_ref().and_then(|id| list.iter().position(|e| e.get("id").and_then(Value::as_str) == Some(id.as_str())));
    let clear = match fields.get("look") {
        None => false,
        Some(Value::Null) => true,
        Some(Value::String(s)) => s.trim().is_empty() || s.trim().eq_ignore_ascii_case("none"),
        Some(_) => false,
    };
    if clear {
        match (&id, existing) {
            (Some(id), None) => return Err(OpError::Validation(format!("No {} has the id `{id}`.", kind.name()))),
            (Some(_), Some(at)) => {
                list.remove(at);
            }
            (None, _) => return Err(OpError::Validation(format!("To remove an {}, send its `id`.", kind.name()))),
        }
    } else {
        let mut sent: Obj = existing.and_then(|at| list[at].as_object().cloned()).unwrap_or_default();
        for (key, value) in fields {
            if key != "id" || id.is_some() {
                sent.insert(key.clone(), value.clone());
            }
        }
        let entry = entry_of(kind, &Value::Object(sent)).map_err(OpError::Validation)?;
        match existing {
            Some(at) => list[at] = entry,
            None => list.push(entry),
        }
    }
    let list = normalized(kind, &list, false).map_err(OpError::Validation)?;
    write(edit, kind, list, project, now)
}
