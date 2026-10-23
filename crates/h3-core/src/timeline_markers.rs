//! Timed Timeline Notes (the Video Builder's `+ Timeline Note` markers). Port of `builder/timeline_markers.py`.
//! The notes live in the session's `timeline_markers` list, each with `id`, `start`, `end` (`null` for a point note),
//! `type`, `label` and `note`, sorted by start time.

use serde_json::{json, Value};

use crate::pyval::{py_str, text_or_empty, to_float, truthy, Obj};

pub const DEFAULT_TYPE: &str = "note";
pub const DEFAULT_LABEL: &str = "Timeline note";
const EDITABLE: [&str; 5] = ["start", "end", "type", "label", "note"];

/// A request that cannot be applied, as a message for the caller.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum MarkerError {
    #[error("{0}")]
    Invalid(String),
    #[error("Timeline note '{0}' was not found.")]
    NotFound(String),
}

fn invalid<T>(message: impl Into<String>) -> Result<T, MarkerError> {
    Err(MarkerError::Invalid(message.into()))
}

fn text_limit(key: &str) -> usize {
    match key {
        "type" => 120,
        "label" => 500,
        _ => 20000,
    }
}

fn seconds(value: &Value, key: &str) -> Result<f64, MarkerError> {
    if value.is_boolean() {
        return invalid(format!("{key} must be a number of seconds."));
    }
    let Some(number) = to_float(value) else { return invalid(format!("{key} must be a number of seconds.")) };
    if !number.is_finite() || number < 0.0 {
        return invalid(format!("{key} must be a number of seconds that is 0 or more."));
    }
    Ok(number)
}

fn text(value: &Value, key: &str) -> Result<String, MarkerError> {
    let out = match value {
        Value::Null => return Ok(String::new()),
        Value::String(_) | Value::Number(_) => py_str(Some(value)).trim().to_string(),
        _ => return invalid(format!("{key} must be text.")),
    };
    if out.chars().count() > text_limit(key) {
        return invalid(format!("{key} is longer than {} characters.", text_limit(key)));
    }
    Ok(out)
}

fn or_default(value: Option<&Value>, default: &str) -> String {
    let t = text_or_empty(value.filter(|v| truthy(v))).trim().to_string();
    if t.is_empty() { default.to_string() } else { t }
}

fn start_of(marker: &Obj) -> f64 {
    marker.get("start").and_then(to_float).unwrap_or(0.0)
}

/// One saved marker in the Builder's shape. A missing or non-increasing end makes a point note.
pub fn normalize_marker(source: &Value, new_id: &mut dyn FnMut() -> String) -> Obj {
    let empty = Obj::new();
    let source = source.as_object().unwrap_or(&empty);
    let start = source.get("start").filter(|v| truthy(v)).and_then(to_float).filter(|s| s.is_finite()).unwrap_or(0.0).max(0.0);
    let end = source.get("end").and_then(to_float).filter(|e| e.is_finite() && *e > start);
    let id = source.get("id").filter(|v| truthy(v)).map(|v| py_str(Some(v))).unwrap_or_else(&mut *new_id);
    let mut out = Obj::new();
    out.insert("id".into(), json!(id));
    out.insert("start".into(), json!(start));
    out.insert("end".into(), end.map_or(Value::Null, |e| json!(e)));
    out.insert("type".into(), json!(or_default(source.get("type"), DEFAULT_TYPE)));
    out.insert("label".into(), json!(or_default(source.get("label"), DEFAULT_LABEL)));
    out.insert("note".into(), json!(text_or_empty(source.get("note").filter(|v| truthy(v))).trim()));
    out
}

fn sort(markers: &mut [Obj]) {
    markers.sort_by(|a, b| start_of(a).total_cmp(&start_of(b)));
}

/// All saved markers, normalized and sorted by start.
pub fn normalize_markers(markers: Option<&Value>, new_id: &mut dyn FnMut() -> String) -> Vec<Obj> {
    let mut items: Vec<Obj> = markers.and_then(Value::as_array).map(|a| a.iter().map(|m| normalize_marker(m, new_id)).collect()).unwrap_or_default();
    sort(&mut items);
    items
}

fn apply_fields(marker: &mut Obj, fields: &Obj) -> Result<(), MarkerError> {
    if let Some(v) = fields.get("start") {
        marker.insert("start".into(), json!(seconds(v, "start")?));
    }
    if let Some(v) = fields.get("end") {
        let cleared = v.is_null() || v.as_str() == Some("");
        marker.insert("end".into(), if cleared { Value::Null } else { json!(seconds(v, "end")?) });
    }
    for key in ["type", "label", "note"] {
        if let Some(v) = fields.get(key) {
            marker.insert(key.into(), json!(text(v, key)?));
        }
    }
    if marker.get("type").and_then(Value::as_str).is_none_or(str::is_empty) {
        marker.insert("type".into(), json!(DEFAULT_TYPE));
    }
    if marker.get("label").and_then(Value::as_str).is_none_or(str::is_empty) {
        marker.insert("label".into(), json!(DEFAULT_LABEL));
    }
    if let Some(end) = marker.get("end").and_then(Value::as_f64) {
        if end <= start_of(marker) {
            return invalid("end must be later than start. Send end: null for a point note.");
        }
    }
    Ok(())
}

fn check_keys(fields: &Value, allowed: &[&str]) -> Result<(), MarkerError> {
    let Some(map) = fields.as_object() else { return invalid("The timeline note must be a JSON object.") };
    let mut unknown: Vec<&String> = map.keys().filter(|k| !allowed.contains(&k.as_str())).collect();
    unknown.sort();
    if !unknown.is_empty() {
        return invalid(format!(
            "Unknown timeline note field{}: {}. Allowed: {}.",
            if unknown.len() > 1 { "s" } else { "" },
            unknown.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "),
            allowed.join(", ")
        ));
    }
    Ok(())
}

fn allowed() -> Vec<&'static str> {
    std::iter::once("id").chain(EDITABLE).collect()
}

/// Add a note to `markers` (already normalized) and return it. `start` is required.
pub fn create_marker(markers: &mut Vec<Obj>, fields: &Value, new_id: &mut dyn FnMut() -> String) -> Result<Obj, MarkerError> {
    check_keys(fields, &allowed())?;
    let map = fields.as_object().expect("checked");
    if !map.contains_key("start") {
        return invalid("start is required (seconds on the project timeline).");
    }
    let given = map.get("id").filter(|v| truthy(v)).map(|v| py_str(Some(v)).trim().to_string()).unwrap_or_default();
    let id = if given.is_empty() { new_id() } else { given };
    if markers.iter().any(|m| m.get("id").and_then(Value::as_str) == Some(id.as_str())) {
        return invalid(format!("A timeline note with id '{id}' already exists."));
    }
    let mut marker = Obj::new();
    marker.insert("id".into(), json!(id));
    marker.insert("start".into(), json!(0.0));
    marker.insert("end".into(), Value::Null);
    marker.insert("type".into(), json!(DEFAULT_TYPE));
    marker.insert("label".into(), json!(DEFAULT_LABEL));
    marker.insert("note".into(), json!(""));
    apply_fields(&mut marker, map)?;
    markers.push(marker.clone());
    sort(markers);
    Ok(marker)
}

fn position(markers: &[Obj], id: &str) -> Option<usize> {
    markers.iter().position(|m| m.get("id").and_then(Value::as_str) == Some(id))
}

/// Change only the given fields of one note.
pub fn update_marker(markers: &mut [Obj], id: &str, fields: &Value) -> Result<Obj, MarkerError> {
    check_keys(fields, &allowed())?;
    let map = fields.as_object().expect("checked");
    if map.get("id").is_some_and(|v| py_str(Some(v)) != id) {
        return invalid("A timeline note id cannot change.");
    }
    let index = position(markers, id).ok_or_else(|| MarkerError::NotFound(id.to_string()))?;
    let mut updated = markers[index].clone();
    let changes: Obj = map.iter().filter(|(k, _)| k.as_str() != "id").map(|(k, v)| (k.clone(), v.clone())).collect();
    apply_fields(&mut updated, &changes)?;
    markers[index] = updated.clone();
    sort(markers);
    Ok(updated)
}

/// Remove one note and return it.
pub fn delete_marker(markers: &mut Vec<Obj>, id: &str) -> Result<Obj, MarkerError> {
    let index = position(markers, id).ok_or_else(|| MarkerError::NotFound(id.to_string()))?;
    Ok(markers.remove(index))
}

/// The scenes a note applies to, by the overlap rule Story Arc planning uses.
pub fn overlapping_scene_ids(marker: &Obj, segments: &[Value]) -> Vec<String> {
    let start = start_of(marker);
    let end = marker.get("end").and_then(Value::as_f64);
    let mut ordered: Vec<&Value> = segments.iter().filter(|s| s.is_object()).collect();
    ordered.sort_by(|a, b| {
        let key = |s: &Value| s.get("start").filter(|v| truthy(v)).and_then(to_float).unwrap_or(0.0);
        key(a).total_cmp(&key(b))
    });
    let mut ids = Vec::new();
    for segment in ordered {
        let number = |k: &str| segment.get(k).filter(|v| truthy(v)).map_or(Some(0.0), to_float);
        let (Some(seg_start), Some(seg_end)) = (number("start"), number("end")) else { continue };
        if seg_end <= seg_start {
            continue;
        }
        let inside = match end {
            None => seg_start <= start && start < seg_end,
            Some(end) => start < seg_end && end > seg_start,
        };
        if inside {
            ids.push(text_or_empty(segment.get("id").filter(|v| truthy(v))));
        }
    }
    ids
}
