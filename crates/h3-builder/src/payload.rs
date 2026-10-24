//! Reading request bodies the way the pack's handlers do.

use h3_core::pyval::{text_or_empty, Obj};
use serde_json::Value;

/// `str(payload.get(key, "") or "").strip().strip('"')`.
pub fn path_text(payload: &Obj, key: &str) -> String {
    text_or_empty(payload.get(key)).trim().trim_matches('"').to_string()
}

/// `str(payload.get(key, "") or "").strip()`.
pub fn text(payload: &Obj, key: &str) -> String {
    text_or_empty(payload.get(key)).trim().to_string()
}

/// A JSON object as `Obj`, or an empty one.
pub fn object(value: Option<&Value>) -> Obj {
    value.and_then(Value::as_object).cloned().unwrap_or_default()
}
