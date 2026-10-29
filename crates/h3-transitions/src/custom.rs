//! The app's own transitions, `transitions/*.glsl`. Each is written once, in the gl-transitions style: a header, the settings as `uniform` lines
//! and `vec4 transition(vec2 uv)`, which may call `getFromColor`, `getToColor` and read `progress` and `ratio`. The preview compiles the source
//! as it is (after `prelude.glsl`, the helpers every transition can use). For the final render it is wrapped in a Vulkan style GLSL 450 shader and
//! translated to WGSL with naga, so the two cannot drift apart.
//!
//! ```text
//! // Author: H3 Studio
//! // License: H3 Studio
//! // Description: One sentence on what it looks like
//! // Tags: zoom, blur, original
//!
//! // @param What the setting does
//! uniform float strength; // = 0.6 (0.0 .. 2.0)
//! uniform vec4 color; // = vec4(1.0, 1.0, 1.0, 1.0) color
//! ```
//!
//! After `= ` comes the default (`0.6`, `3`, `vec2(1.0, 0.0)`), then optionally `(min .. max)` and the word `color`. A setting can be a float,
//! an int or a vector (a bool cannot live in a uniform block).

use crate::entry::{Entry, Kind, Origin, ParamDef, ParamType, Value};

const PRELUDE: &str = include_str!("../transitions/prelude.glsl");

macro_rules! sources {
    ($($name:literal),* $(,)?) => {
        &[$(($name, include_str!(concat!("../transitions/", $name, ".glsl")))),*]
    };
}

const SOURCES: &[(&str, &str)] = sources![
    "ZoomPullIn", "ZoomPullOut", "WhipPanLeft", "WhipPanRight", "WhipPanUp", "WhipPanDown", "SpinBlur", "PulsePop", "Stretch", "CameraShake", "RgbGlitch",
    "DigitalTear", "VhsTracking", "TvOff", "FlashColor", "LightLeak", "BlurDissolve", "ChromaticFade",
];

pub(crate) fn header<'a>(source: &'a str, key: &str) -> &'a str {
    let prefix = format!("// {key}:");
    source.lines().find_map(|l| l.trim().strip_prefix(prefix.as_str())).map_or("", str::trim)
}

fn numbers(text: &str) -> Option<Vec<f32>> {
    let inner = text.find('(').and_then(|open| text[open + 1..].find(')').map(|close| &text[open + 1..open + 1 + close])).unwrap_or(text);
    inner.split(',').map(|n| n.trim().parse::<f32>().ok()).collect()
}

fn value_of(ty: ParamType, text: &str) -> Option<Value> {
    let n = numbers(text)?;
    if n.len() != ty.components() {
        return None;
    }
    let i = |k: usize| n[k].round() as i32;
    Some(match ty {
        ParamType::Float => Value::Float(n[0]),
        ParamType::Int => Value::Int(i(0)),
        ParamType::Bool => return None,
        ParamType::Vec2 => Value::Vec2([n[0], n[1]]),
        ParamType::Vec3 => Value::Vec3([n[0], n[1], n[2]]),
        ParamType::Vec4 => Value::Vec4([n[0], n[1], n[2], n[3]]),
        ParamType::IVec2 => Value::IVec2([i(0), i(1)]),
        ParamType::IVec3 => Value::IVec3([i(0), i(1), i(2)]),
        ParamType::IVec4 => Value::IVec4([i(0), i(1), i(2), i(3)]),
    })
}

/// The settings a source declares, in order.
pub(crate) fn params(id: &str, source: &str) -> Result<Vec<ParamDef>, String> {
    let mut out = Vec::new();
    let mut description: Option<String> = None;
    for line in source.lines() {
        let line = line.trim();
        if let Some(text) = line.strip_prefix("// @param") {
            description = Some(text.trim().to_string());
            continue;
        }
        let Some(declaration) = line.strip_prefix("uniform ") else {
            if !line.is_empty() && !line.starts_with("//") {
                description = None;
            }
            continue;
        };
        let (declaration, comment) = declaration.split_once("//").ok_or_else(|| format!("{id}: `{line}` has no `// = default`."))?;
        let mut words = declaration.trim().trim_end_matches(';').split_whitespace();
        let (ty, name) = (words.next().unwrap_or(""), words.next().unwrap_or(""));
        let ty = ParamType::from_glsl_name(ty).filter(|t| *t != ParamType::Bool).ok_or_else(|| format!("{id}: `{line}` is not a float, int or vector."))?;
        let rest = comment.trim().strip_prefix('=').ok_or_else(|| format!("{id}: `{line}` has no `// = default`."))?.trim();
        let default_end = if rest.starts_with("vec") || rest.starts_with("ivec") { rest.find(')').map_or(rest.len(), |p| p + 1) } else { rest.find(char::is_whitespace).unwrap_or(rest.len()) };
        let default = value_of(ty, &rest[..default_end]).ok_or_else(|| format!("{id}: the default of `{name}` is not a {}.", ty.glsl_name()))?;
        let tail = &rest[default_end..];
        let range = tail.find('(').and_then(|open| tail[open + 1..].find(')').map(|close| &tail[open + 1..open + 1 + close])).and_then(|r| r.split_once(".."));
        let (min, max) = match range {
            Some((a, b)) => (a.trim().parse::<f32>().ok(), b.trim().parse::<f32>().ok()),
            None => (None, None),
        };
        out.push(ParamDef {
            name: name.to_string(),
            ty,
            default,
            min,
            max,
            step: min.zip(max).map(|(a, b)| (b - a) / 100.0),
            color: tail.split_whitespace().any(|w| w == "color"),
            description: description.take(),
        });
    }
    Ok(out)
}

/// The final shader's GLSL 450 source: the settings in one uniform block with `progress` and `ratio`, the two pictures, and the same
/// `getFromColor` and `getToColor` the preview has (the pictures' rows run top down, the transition's uv bottom up, hence the flip).
fn vulkan_glsl(params: &[ParamDef], source: &str) -> String {
    let mut block = String::from("layout(std140, set = 0, binding = 0) uniform Params {\n  float progress;\n  float ratio;\n");
    for p in params {
        block.push_str(&format!("  {} {};\n", p.ty.glsl_name(), p.name));
    }
    block.push_str("};\n");
    let body: String = source.lines().filter(|l| !l.trim_start().starts_with("uniform ")).map(|l| format!("{l}\n")).collect();
    format!(
        "#version 450\n{block}\
         layout(set = 0, binding = 1) uniform sampler _sampler;\n\
         layout(set = 0, binding = 2) uniform texture2D _from;\n\
         layout(set = 0, binding = 3) uniform texture2D _to;\n\
         layout(location = 0) in vec2 _uv;\n\
         layout(location = 0) out vec4 _color;\n\
         vec4 getFromColor(vec2 uv) {{ return textureLod(sampler2D(_from, _sampler), vec2(uv.x, 1.0 - uv.y), 0.0); }}\n\
         vec4 getToColor(vec2 uv) {{ return textureLod(sampler2D(_to, _sampler), vec2(uv.x, 1.0 - uv.y), 0.0); }}\n\
         {PRELUDE}\n{body}\n\
         void main() {{ _color = transition(_uv); }}\n"
    )
}

pub(crate) fn to_wgsl(source: &str) -> Result<String, String> {
    let mut frontend = naga::front::glsl::Frontend::default();
    let module = frontend.parse(&naga::front::glsl::Options::from(naga::ShaderStage::Fragment), source).map_err(|e| format!("{e:?}"))?;
    let info = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all()).validate(&module).map_err(|e| format!("{e:?}"))?;
    naga::back::wgsl::write_string(&module, &info, naga::back::wgsl::WriterFlags::empty()).map_err(|e| e.to_string())
}

fn entry(id: &str, source: &str) -> Result<Entry, String> {
    let params = params(id, source)?;
    let wgsl = to_wgsl(&vulkan_glsl(&params, source)).map_err(|e| format!("{id}: {e}"))?;
    Ok(Entry::new(
        id.to_string(),
        header(source, "Author").to_string(),
        header(source, "License").to_string(),
        header(source, "Description").to_string(),
        header(source, "Tags").split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect(),
        Origin::Original,
        Kind::Transition,
        params,
        Vec::new(),
        format!("{PRELUDE}\n{source}"),
        wgsl,
    ))
}

/// The app's own transitions. One that fails to translate is reported on stderr and left out (a test names every one that must be there).
pub fn entries() -> Vec<Entry> {
    SOURCES
        .iter()
        .filter_map(|(id, source)| match entry(id, source) {
            Ok(entry) => Some(entry),
            Err(error) => {
                eprintln!("[VRGDG Transitions] {error}");
                None
            }
        })
        .collect()
}

/// Every transition the app has of its own, translation errors included, for the test that checks them.
pub fn try_entries() -> Vec<Result<Entry, String>> {
    SOURCES.iter().map(|(id, source)| entry(id, source)).collect()
}
