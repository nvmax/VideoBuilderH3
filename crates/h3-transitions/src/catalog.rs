//! The list of transitions, effects and filters and their settings, as the UI and the API show them: the gl-transitions collection and the app's own
//! transitions, and the app's effects and filters, by name.

use std::sync::OnceLock;

use serde_json::{json, Map, Value as Json};

use crate::entry::{Entry, Kind, ParamDef, ParamType, Params, Value};
use crate::{custom, library, looks, TransitionError};

fn all() -> &'static [Entry] {
    static ENTRIES: OnceLock<Vec<Entry>> = OnceLock::new();
    ENTRIES.get_or_init(|| {
        let mut entries = library::entries();
        entries.extend(custom::entries());
        entries.sort_by(|a, b| a.id.cmp(&b.id));
        entries
    })
}

pub fn transition(name: &str) -> Result<&'static Entry, TransitionError> {
    all().iter().find(|e| e.id == name).ok_or_else(|| TransitionError::Unknown(name.to_string()))
}

fn looks_of(kind: Kind) -> &'static [Entry] {
    static EFFECTS: OnceLock<Vec<Entry>> = OnceLock::new();
    static FILTERS: OnceLock<Vec<Entry>> = OnceLock::new();
    let cell = if kind == Kind::Filter { &FILTERS } else { &EFFECTS };
    cell.get_or_init(|| looks::entries(kind))
}

/// An effect or a filter by name (`kind` is `Kind::Effect` or `Kind::Filter`).
pub fn look(kind: Kind, name: &str) -> Result<&'static Entry, TransitionError> {
    looks_of(kind).iter().find(|e| e.id == name).ok_or_else(|| TransitionError::UnknownLook { kind: kind.name(), name: name.to_string() })
}

/// The source of an effect or a filter for the WebGL preview (GLSL ES 3.00). The page supplies `progress`, `ratio`, `intensity`, `time` and `getColor`.
pub fn look_glsl3(kind: Kind, name: &str) -> Result<&'static str, TransitionError> {
    look(kind, name).map(|t| t.glsl3.as_str())
}

/// The source of a transition for the WebGL preview (GLSL ES 3.00). The page supplies `progress`, `ratio`, `getFromColor` and `getToColor`.
pub fn glsl3(name: &str) -> Result<&'static str, TransitionError> {
    transition(name).map(|t| t.glsl3.as_str())
}

fn value_json(value: Value) -> Json {
    match value {
        Value::Float(v) => json!(v),
        Value::Vec2(v) => json!(v),
        Value::Vec3(v) => json!(v),
        Value::Vec4(v) => json!(v),
        Value::Int(v) => json!(v),
        Value::IVec2(v) => json!(v),
        Value::IVec3(v) => json!(v),
        Value::IVec4(v) => json!(v),
        Value::Bool(v) => json!(v),
    }
}

fn param_json(p: &ParamDef) -> Json {
    json!({
        "name": p.name,
        "type": p.ty.glsl_name(),
        "default": value_json(p.default),
        "min": p.min,
        "max": p.max,
        "step": p.step,
        "color": p.color,
        "description": p.description,
    })
}

pub fn info(t: &Entry) -> Json {
    json!({
        "id": t.id,
        "kind": t.kind.name(),
        "category": t.category,
        "origin": t.origin.name(),
        "author": t.author,
        "license": t.license,
        "description": t.description,
        "tags": t.tags,
        "params": t.params.iter().map(param_json).collect::<Vec<_>>(),
    })
}

/// Every transition, sorted by name.
pub fn catalog() -> Vec<Json> {
    all().iter().map(info).collect()
}

/// Every effect or filter, sorted by name.
pub fn look_catalog(kind: Kind) -> Vec<Json> {
    looks_of(kind).iter().map(info).collect()
}

fn floats<const N: usize>(name: &str, value: &Json) -> Result<[f32; N], TransitionError> {
    let list = value.as_array().filter(|a| a.len() == N).ok_or_else(|| TransitionError::Params(format!("`{name}` needs a list of {N} numbers.")))?;
    let mut out = [0.0f32; N];
    for (slot, item) in out.iter_mut().zip(list) {
        *slot = item.as_f64().ok_or_else(|| TransitionError::Params(format!("`{name}` needs numbers.")))? as f32;
    }
    Ok(out)
}

fn ints<const N: usize>(name: &str, value: &Json) -> Result<[i32; N], TransitionError> {
    Ok(floats::<N>(name, value)?.map(|v| v.round() as i32))
}

fn value_from_json(p: &ParamDef, value: &Json) -> Result<Value, TransitionError> {
    let number = || value.as_f64().ok_or_else(|| TransitionError::Params(format!("`{}` needs a number.", p.name)));
    Ok(match p.ty {
        ParamType::Float => Value::Float(number()? as f32),
        ParamType::Int => Value::Int(number()?.round() as i32),
        ParamType::Bool => Value::Bool(value.as_bool().or_else(|| value.as_f64().map(|n| n != 0.0)).ok_or_else(|| TransitionError::Params(format!("`{}` needs true or false.", p.name)))?),
        ParamType::Vec2 => Value::Vec2(floats(&p.name, value)?),
        ParamType::Vec3 => Value::Vec3(floats(&p.name, value)?),
        ParamType::Vec4 => Value::Vec4(floats(&p.name, value)?),
        ParamType::IVec2 => Value::IVec2(ints(&p.name, value)?),
        ParamType::IVec3 => Value::IVec3(ints(&p.name, value)?),
        ParamType::IVec4 => Value::IVec4(ints(&p.name, value)?),
    })
}

/// The defaults of a transition, with the values of the JSON object put over them. A missing or `null` value keeps the default.
/// A name the transition does not have, or a value of the wrong shape, is an error.
pub fn params_from_json(t: &'static Entry, values: &Json) -> Result<Params, TransitionError> {
    let mut params = Params::new(t);
    let empty = Map::new();
    let object = match values {
        Json::Null => &empty,
        Json::Object(map) => map,
        _ => return Err(TransitionError::Params("The parameters must be an object.".into())),
    };
    for (name, value) in object {
        if value.is_null() {
            continue;
        }
        let param = t.param(name).ok_or_else(|| TransitionError::Params(format!("`{}` has no parameter `{name}`.", t.id)))?;
        params.set(name, value_from_json(param, value)?).map_err(TransitionError::Params)?;
    }
    Ok(params)
}
