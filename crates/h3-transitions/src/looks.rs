//! The app's effects and filters: `looks/effects/*.glsl` and `looks/filters/*.glsl`. They are written like the app's own transitions (see `custom.rs` for
//! the header and the `uniform` lines), but a look changes one picture instead of blending two. The file defines `vec4 effect(vec2 uv)`, which reads the
//! picture with `getColor(uv)` and returns the changed colour, and may use `progress` (0 to 1 over the look's block), `ratio` (width over height) and
//! `time` (seconds since the block started). `intensity`, how much of the change is mixed in, is applied around it and is not a setting of the file.
//!
//! ```text
//! // Author: H3 Studio
//! // License: H3 Studio
//! // Description: One sentence on what it looks like
//! // Category: Glitch
//! // Tags: glitch, colour split
//!
//! // @param How far the colours are pulled apart
//! uniform float amount; // = 0.4 (0.0 .. 1.0)
//!
//! vec4 effect(vec2 uv) { ... }
//! ```
//!
//! The file name (without `.glsl`) is the look's id. The preview compiles `prelude.glsl` and the file as they are. For the final render the file is wrapped in
//! a Vulkan style GLSL 450 shader and translated to WGSL with naga, so the two cannot drift apart.

use include_dir::{include_dir, Dir};

use crate::custom::{header, params, to_wgsl};
use crate::entry::{Entry, Kind, Origin, ParamDef};

static EFFECTS: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/looks/effects");
static FILTERS: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/looks/filters");
const PRELUDE: &str = include_str!("../looks/prelude.glsl");

/// The final shader's GLSL 450 source: the settings in one uniform block after `progress`, `ratio`, `intensity` and `time`, the picture, and the same
/// `getColor` the preview has (the picture's rows run top down, the look's uv bottom up, hence the flip). `intensity` mixes the change with the original.
fn vulkan_glsl(params: &[ParamDef], source: &str) -> String {
    let mut block = String::from("layout(std140, set = 0, binding = 0) uniform Params {\n  float progress;\n  float ratio;\n  float intensity;\n  float time;\n");
    for p in params {
        block.push_str(&format!("  {} {};\n", p.ty.glsl_name(), p.name));
    }
    block.push_str("};\n");
    let body: String = source.lines().filter(|l| !l.trim_start().starts_with("uniform ")).map(|l| format!("{l}\n")).collect();
    format!(
        "#version 450\n{block}\
         layout(set = 0, binding = 1) uniform sampler _sampler;\n\
         layout(set = 0, binding = 2) uniform texture2D _from;\n\
         layout(location = 0) in vec2 _uv;\n\
         layout(location = 0) out vec4 _color;\n\
         vec4 getColor(vec2 uv) {{ return textureLod(sampler2D(_from, _sampler), vec2(uv.x, 1.0 - uv.y), 0.0); }}\n\
         {PRELUDE}\n{body}\n\
         void main() {{ vec4 src = getColor(_uv); vec4 fx = effect(_uv); _color = vec4(mix(src.rgb, fx.rgb, clamp(intensity, 0.0, 1.0)), 1.0); }}\n"
    )
}

fn sources(kind: Kind) -> Vec<(String, &'static str)> {
    let dir = if kind == Kind::Effect { &EFFECTS } else { &FILTERS };
    let mut list: Vec<(String, &'static str)> = dir
        .files()
        .filter(|f| f.path().extension().is_some_and(|e| e == "glsl"))
        .filter_map(|f| Some((f.path().file_stem()?.to_string_lossy().to_string(), f.contents_utf8()?)))
        .collect();
    list.sort_by(|a, b| a.0.cmp(&b.0));
    list
}

fn entry(kind: Kind, id: &str, source: &str) -> Result<Entry, String> {
    let params = params(id, source)?;
    if params.iter().any(|p| matches!(p.name.as_str(), "intensity" | "progress" | "ratio" | "time")) {
        return Err(format!("{id}: `progress`, `ratio`, `intensity` and `time` are the look's own, a setting cannot have those names."));
    }
    let wgsl = to_wgsl(&vulkan_glsl(&params, source)).map_err(|e| format!("{id}: {e}"))?;
    let mut entry = Entry::new(
        id.to_string(),
        header(source, "Author").to_string(),
        header(source, "License").to_string(),
        header(source, "Description").to_string(),
        header(source, "Tags").split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect(),
        Origin::Original,
        kind,
        params,
        Vec::new(),
        format!("{PRELUDE}\n{source}"),
        wgsl,
    );
    entry.category = header(source, "Category").to_string();
    Ok(entry)
}

/// The effects or the filters the app has. One that fails to translate is reported on stderr and left out (a test names every one that must be there).
pub fn entries(kind: Kind) -> Vec<Entry> {
    sources(kind)
        .into_iter()
        .filter_map(|(id, source)| match entry(kind, &id, source) {
            Ok(entry) => Some(entry),
            Err(error) => {
                eprintln!("[VRGDG Transitions] {error}");
                None
            }
        })
        .collect()
}

/// Every effect or filter the app has, translation errors included, for the test that checks them.
pub fn try_entries(kind: Kind) -> Vec<Result<Entry, String>> {
    sources(kind).into_iter().map(|(id, source)| entry(kind, &id, source)).collect()
}
