//! The gl-transitions collection as entries. A transition needs a WGSL shader for the final render and a GLSL ES 3.00 one for the preview, so the
//! few the collection's compilers rejected for either are left out.

use gl_transitions::{Param, ParamType as GlType, Transition, Value as GlValue};

use crate::entry::{Entry, Kind, Origin, ParamDef, ParamType, Value};

fn param_type(ty: GlType) -> Option<ParamType> {
    Some(match ty {
        GlType::Float => ParamType::Float,
        GlType::Int => ParamType::Int,
        GlType::Bool => ParamType::Bool,
        GlType::Vec2 => ParamType::Vec2,
        GlType::Vec3 => ParamType::Vec3,
        GlType::Vec4 => ParamType::Vec4,
        GlType::IVec2 => ParamType::IVec2,
        GlType::IVec3 => ParamType::IVec3,
        GlType::IVec4 => ParamType::IVec4,
        _ => return None,
    })
}

fn value(v: GlValue) -> Option<Value> {
    Some(match v {
        GlValue::Float(v) => Value::Float(v),
        GlValue::Int(v) => Value::Int(v),
        GlValue::Bool(v) => Value::Bool(v),
        GlValue::Vec2(v) => Value::Vec2(v),
        GlValue::Vec3(v) => Value::Vec3(v),
        GlValue::Vec4(v) => Value::Vec4(v),
        GlValue::IVec2(v) => Value::IVec2(v),
        GlValue::IVec3(v) => Value::IVec3(v),
        GlValue::IVec4(v) => Value::IVec4(v),
        _ => return None,
    })
}

fn param(p: &Param) -> Option<ParamDef> {
    Some(ParamDef {
        name: p.name.to_string(),
        ty: param_type(p.ty)?,
        default: value(p.default)?,
        min: p.min,
        max: p.max,
        step: p.step,
        color: p.color,
        description: p.description.map(str::to_string),
    })
}

fn entry(t: &Transition) -> Option<Entry> {
    let params = t.params().iter().map(param).collect::<Option<Vec<_>>>()?;
    Some(Entry::new(
        t.name().to_string(),
        t.author().to_string(),
        t.license().to_string(),
        t.description().unwrap_or_default().to_string(),
        t.tags().iter().map(|s| s.to_string()).collect(),
        Origin::Library,
        Kind::Transition,
        params,
        t.textures().iter().map(|s| s.to_string()).collect(),
        t.glsl3()?.to_string(),
        t.wgsl()?.to_string(),
    ))
}

pub fn entries() -> Vec<Entry> {
    gl_transitions::all().iter().filter_map(entry).collect()
}

/// The collection's own uniform buffer for a transition with its default settings, to check ours against.
pub fn reference_buffer(id: &str, progress: f32, ratio: f32) -> Option<Vec<u8>> {
    gl_transitions::get(id)?.default_params().uniform_buffer(progress, ratio)
}
