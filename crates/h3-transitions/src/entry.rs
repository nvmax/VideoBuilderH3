//! One shader as the rest of the app sees it: a transition (from the gl-transitions collection or one of the app's own), or one of the app's effects
//! and filters. Its metadata, its settings, the two shader sources (GLSL ES 3.00 for the preview, WGSL for the final render) and the layout of the
//! settings in the uniform buffer. `Params` holds the values of the settings and writes that buffer.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamType {
    Float,
    Int,
    Bool,
    Vec2,
    Vec3,
    Vec4,
    IVec2,
    IVec3,
    IVec4,
}

impl ParamType {
    pub fn glsl_name(self) -> &'static str {
        match self {
            Self::Float => "float",
            Self::Int => "int",
            Self::Bool => "bool",
            Self::Vec2 => "vec2",
            Self::Vec3 => "vec3",
            Self::Vec4 => "vec4",
            Self::IVec2 => "ivec2",
            Self::IVec3 => "ivec3",
            Self::IVec4 => "ivec4",
        }
    }

    pub fn from_glsl_name(name: &str) -> Option<Self> {
        [Self::Float, Self::Int, Self::Bool, Self::Vec2, Self::Vec3, Self::Vec4, Self::IVec2, Self::IVec3, Self::IVec4].into_iter().find(|t| t.glsl_name() == name)
    }

    pub fn components(self) -> usize {
        match self {
            Self::Float | Self::Int | Self::Bool => 1,
            Self::Vec2 | Self::IVec2 => 2,
            Self::Vec3 | Self::IVec3 => 3,
            Self::Vec4 | Self::IVec4 => 4,
        }
    }

    /// `(size, alignment)` in a std140 uniform block, which is also what a WGSL uniform struct uses for these types.
    fn std140(self) -> (usize, usize) {
        match self.components() {
            1 => (4, 4),
            2 => (8, 8),
            3 => (12, 16),
            _ => (16, 16),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value {
    Float(f32),
    Int(i32),
    Bool(bool),
    Vec2([f32; 2]),
    Vec3([f32; 3]),
    Vec4([f32; 4]),
    IVec2([i32; 2]),
    IVec3([i32; 3]),
    IVec4([i32; 4]),
}

impl Value {
    pub fn ty(&self) -> ParamType {
        match self {
            Self::Float(_) => ParamType::Float,
            Self::Int(_) => ParamType::Int,
            Self::Bool(_) => ParamType::Bool,
            Self::Vec2(_) => ParamType::Vec2,
            Self::Vec3(_) => ParamType::Vec3,
            Self::Vec4(_) => ParamType::Vec4,
            Self::IVec2(_) => ParamType::IVec2,
            Self::IVec3(_) => ParamType::IVec3,
            Self::IVec4(_) => ParamType::IVec4,
        }
    }

    /// The value as the shader reads it: 4 bytes a component, little endian, a bool as an int.
    fn write(&self, out: &mut Vec<u8>) {
        let floats = |out: &mut Vec<u8>, v: &[f32]| v.iter().for_each(|c| out.extend(c.to_le_bytes()));
        let ints = |out: &mut Vec<u8>, v: &[i32]| v.iter().for_each(|c| out.extend(c.to_le_bytes()));
        match self {
            Self::Float(v) => floats(out, &[*v]),
            Self::Int(v) => ints(out, &[*v]),
            Self::Bool(v) => ints(out, &[i32::from(*v)]),
            Self::Vec2(v) => floats(out, v),
            Self::Vec3(v) => floats(out, v),
            Self::Vec4(v) => floats(out, v),
            Self::IVec2(v) => ints(out, v),
            Self::IVec3(v) => ints(out, v),
            Self::IVec4(v) => ints(out, v),
        }
    }
}

/// A setting of a transition, with its default and the hints a settings form needs.
#[derive(Clone, Debug)]
pub struct ParamDef {
    pub name: String,
    pub ty: ParamType,
    pub default: Value,
    pub min: Option<f32>,
    pub max: Option<f32>,
    pub step: Option<f32>,
    /// The value is a colour (RGB or RGBA in 0..1).
    pub color: bool,
    pub description: Option<String>,
}

/// What a shader does. A transition blends two pictures. An effect and a filter change one picture: an effect adds or bends something (a glitch, a blur,
/// a flash), a filter grades its colours. All three are written the same way, and the uniform block starts with `progress` and `ratio`. A look (an
/// effect or a filter) also has `intensity`, how much of the change is mixed in, and `time`, the seconds since its block started.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Transition,
    Effect,
    Filter,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Transition => "transition",
            Self::Effect => "effect",
            Self::Filter => "filter",
        }
    }

    /// The bytes of the uniform block before the settings: `progress` and `ratio`, and for a look `intensity` and `time` too.
    pub fn header_bytes(self) -> usize {
        if self == Self::Transition {
            8
        } else {
            16
        }
    }

    pub fn is_look(self) -> bool {
        self != Self::Transition
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// From the gl-transitions collection.
    Library,
    /// One of the app's own, in `transitions/`.
    Original,
}

impl Origin {
    pub fn name(self) -> &'static str {
        match self {
            Self::Library => "library",
            Self::Original => "original",
        }
    }
}

#[derive(Debug)]
pub struct Entry {
    pub id: String,
    pub author: String,
    pub license: String,
    pub description: String,
    pub tags: Vec<String>,
    pub origin: Origin,
    pub kind: Kind,
    /// The group a look is listed under in the picker (`Glitch`, `Cinematic`). Empty for a transition.
    pub category: String,
    pub params: Vec<ParamDef>,
    /// Extra pictures the shader samples besides the two it blends (`luma`, `displacementMap`), bound after them in this order.
    pub textures: Vec<String>,
    /// The source the WebGL preview compiles: `vec4 transition(vec2 uv)` and the uniforms it declares.
    pub glsl3: String,
    /// A complete WGSL fragment shader for the final render, with the bindings `render.rs` sets up.
    pub wgsl: String,
    offsets: Vec<usize>,
    size: usize,
}

impl Entry {
    pub fn new(
        id: String,
        author: String,
        license: String,
        description: String,
        tags: Vec<String>,
        origin: Origin,
        kind: Kind,
        params: Vec<ParamDef>,
        textures: Vec<String>,
        glsl3: String,
        wgsl: String,
    ) -> Self {
        // The header (`progress` and `ratio`, for a look `intensity` and `time` too) comes first, then the settings in declaration order.
        let mut cursor: usize = kind.header_bytes();
        let mut offsets = Vec::with_capacity(params.len());
        for param in &params {
            let (size, align) = param.ty.std140();
            cursor = cursor.next_multiple_of(align);
            offsets.push(cursor);
            cursor += size;
        }
        Self { id, author, license, description, tags, origin, kind, category: String::new(), params, textures, glsl3, wgsl, offsets, size: cursor.next_multiple_of(16) }
    }

    pub fn param(&self, name: &str) -> Option<&ParamDef> {
        self.params.iter().find(|p| p.name == name)
    }

    /// Size of the uniform buffer in bytes. Some backends refuse a binding smaller than 16.
    pub fn uniform_size(&self) -> usize {
        self.size.max(16)
    }

    /// The offset of each setting in the uniform buffer, in declaration order. `progress` is at 0 and `ratio` at 4 (a look has `intensity` at 8 and `time` at 12).
    pub fn offsets(&self) -> &[usize] {
        &self.offsets
    }
}

/// The values of every setting of one transition, starting from the defaults.
#[derive(Clone, Debug)]
pub struct Params {
    entry: &'static Entry,
    values: Vec<Value>,
}

impl Params {
    pub fn new(entry: &'static Entry) -> Self {
        Self { entry, values: entry.params.iter().map(|p| p.default).collect() }
    }

    pub fn entry(&self) -> &'static Entry {
        self.entry
    }

    pub fn get(&self, name: &str) -> Option<Value> {
        self.entry.params.iter().position(|p| p.name == name).map(|i| self.values[i])
    }

    /// Sets a setting. The value must have the setting's type.
    pub fn set(&mut self, name: &str, value: Value) -> Result<(), String> {
        let index = self.entry.params.iter().position(|p| p.name == name).ok_or_else(|| format!("`{}` has no parameter `{name}`.", self.entry.id))?;
        let param = &self.entry.params[index];
        if value.ty() != param.ty {
            return Err(format!("`{name}` is a {}, not a {}.", param.ty.glsl_name(), value.ty().glsl_name()));
        }
        self.values[index] = value;
        Ok(())
    }

    /// Every setting with its current value, in declaration order.
    pub fn iter(&self) -> impl Iterator<Item = (&'static ParamDef, Value)> + '_ {
        self.entry.params.iter().zip(self.values.iter().copied())
    }

    /// The uniform buffer of a transition: `progress`, `ratio`, then the settings at their offsets.
    pub fn uniform_buffer(&self, progress: f32, ratio: f32) -> Vec<u8> {
        let mut out = vec![0u8; self.entry.uniform_size()];
        out[0..4].copy_from_slice(&progress.to_le_bytes());
        out[4..8].copy_from_slice(&ratio.to_le_bytes());
        self.write_settings(&mut out);
        out
    }

    /// The uniform buffer of an effect or a filter: `progress` (0 to 1 over its block), `ratio`, `intensity` (0 to 1) and `time` (seconds since its block
    /// started), then the settings at their offsets.
    pub fn look_buffer(&self, progress: f32, ratio: f32, intensity: f32, time: f32) -> Vec<u8> {
        let mut out = vec![0u8; self.entry.uniform_size()];
        for (at, value) in [progress, ratio, intensity, time].into_iter().enumerate() {
            out[at * 4..at * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
        self.write_settings(&mut out);
        out
    }

    fn write_settings(&self, out: &mut [u8]) {
        let mut bytes = Vec::with_capacity(16);
        for (offset, value) in self.entry.offsets.iter().zip(&self.values) {
            bytes.clear();
            value.write(&mut bytes);
            out[*offset..*offset + bytes.len()].copy_from_slice(&bytes);
        }
    }
}
