//! The RefMod a character or location save names, checked against the saved RefMods before the card is written. Port of `_refmod_binding` in
//! `agent_api/mutations.py`.
//!
//! `source: "refmod"` with `refmod: {name, strength}` (or `refmod` as the RefMod's name, or `refmod_name`) picks a saved RefMod from `GET /refmods`, the way
//! the Reference Builder's RefMod picker does. The folder, type, kind, tokens and frames come from the RefMod file. `source: "image"` takes the RefMod off.
//! `session_ops::upsert_reference` then writes what this leaves in the payload.

use h3_core::pyval::{text_or_empty, to_float, Obj};
use h3_pipeline::refmod_library::{find_refmod, list_refmods};
use serde_json::{json, Value};

use crate::app::{blocking, AppState};
use crate::error::ApiError;

fn given(payload: &Obj, key: &str) -> Option<Value> {
    payload.get(key).filter(|v| !v.is_null()).cloned()
}

/// Rewrite `source` and `refmod` in `payload` into what the card should get, or refuse the save.
pub(crate) async fn resolve_refmod(state: &AppState, payload: &mut Obj) -> Result<(), ApiError> {
    let (source, raw, name_only) = (given(payload, "source"), given(payload, "refmod"), given(payload, "refmod_name"));
    payload.remove("refmod_name");
    if source.is_none() && raw.is_none() && name_only.is_none() {
        payload.remove("source");
        payload.remove("refmod");
        return Ok(());
    }
    match source.as_ref().map(Value::as_str) {
        None | Some(Some("refmod")) => {}
        Some(Some("image")) => {
            payload.insert("source".into(), json!("image"));
            payload.remove("refmod");
            return Ok(());
        }
        Some(_) => return Err(ApiError::validation("source must be \"image\" or \"refmod\".")),
    }
    let picked: Obj = match (raw, name_only) {
        (Some(Value::Object(map)), _) => map,
        (Some(Value::String(name)), _) | (None, Some(Value::String(name))) => [("name".to_string(), json!(name))].into_iter().collect(),
        _ => Obj::new(),
    };
    let name = text_or_empty(picked.get("name")).trim().to_string();
    if name.is_empty() {
        return Err(ApiError::validation("source \"refmod\" needs refmod: {\"name\": ...}, a saved RefMod from GET /refmods."));
    }
    let strength = match picked.get("strength").filter(|v| !v.is_null()) {
        None => None,
        Some(value) => match to_float(value).filter(|s| (0.0..=1.0).contains(s)) {
            Some(strength) => Some(strength),
            None => return Err(ApiError::validation("refmod.strength must be a number from 0 to 1.")),
        },
    };
    let dirs = h3_pipeline::context::refmod_dirs_of(&state.cfg).await;
    let lookup = name.clone();
    let (entry, available) = blocking(move || {
        let entry = find_refmod(&dirs, &lookup);
        let available = if entry.is_none() { list_refmods(&dirs, "").iter().take(40).map(|e| e.name.clone()).collect::<Vec<_>>() } else { Vec::new() };
        (entry, available)
    })
    .await?;
    let Some(entry) = entry else {
        let list = if available.is_empty() { "none saved".to_string() } else { available.join(", ") };
        return Err(ApiError::validation(format!("RefMod '{name}' is not in models/refmods. Pick one from GET /refmods: {list}.")));
    };
    let mut refmod = Obj::new();
    for (key, value) in [("name", json!(entry.name)), ("folder", json!(entry.folder)), ("type", json!(entry.kind_of_concept)), ("kind", json!(entry.kind)), ("tokens", json!(entry.tokens)), ("frames", json!(entry.frames))] {
        refmod.insert(key.into(), value);
    }
    if let Some(strength) = strength {
        refmod.insert("strength".into(), json!(strength));
    }
    payload.insert("source".into(), json!("refmod"));
    payload.insert("refmod".into(), Value::Object(refmod));
    Ok(())
}
