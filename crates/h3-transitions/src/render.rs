//! Draws one frame of a transition on the GPU.
//!
//! Every transition of the collection is a complete WGSL fragment shader (entry point `main`, uv in `[0, 1]` with the origin at the
//! bottom left). This file adds the vertex stage (one triangle over the picture), the bindings the shaders document (0 the uniform
//! buffer, 1 a linear clamp sampler, 2 the `from` picture, 3 the `to` picture, then the extra pictures a few transitions sample)
//! and the read back of the result.

use std::collections::HashMap;
use std::sync::mpsc;

use crate::entry::{Entry, Kind, Params};
use crate::TransitionError;

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const EXTRA_SIZE: u32 = 256;

const VERTEX: &str = "
struct Out { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex
fn vs(@builtin(vertex_index) i: u32) -> Out {
    var corners = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    let p = corners[i];
    var out: Out;
    out.position = vec4<f32>(p, 0.0, 1.0);
    out.uv = p * 0.5 + vec2<f32>(0.5, 0.5);
    return out;
}
";

struct Pipeline {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniforms: wgpu::Buffer,
    extras: Vec<wgpu::TextureView>,
}

struct Targets {
    width: u32,
    height: u32,
    from: wgpu::Texture,
    to: wgpu::Texture,
    output: wgpu::Texture,
    readback: wgpu::Buffer,
    padded_row: u32,
}

pub struct TransitionRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    sampler: wgpu::Sampler,
    vertex: wgpu::ShaderModule,
    pipelines: HashMap<(Kind, &'static str), Pipeline>,
    targets: Option<Targets>,
    adapter_name: String,
}

fn gpu(error: impl std::fmt::Display) -> TransitionError {
    TransitionError::Gpu(error.to_string())
}

impl TransitionRenderer {
    /// Opens the best adapter, or the software one when the computer has no usable GPU.
    pub fn new() -> Result<Self, TransitionError> {
        let instance = wgpu::Instance::default();
        let adapter = [false, true]
            .into_iter()
            .find_map(|fallback| {
                pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    force_fallback_adapter: fallback,
                    compatible_surface: None,
                    ..Default::default()
                }))
                .ok()
            })
            .ok_or_else(|| TransitionError::NoAdapter("neither a GPU nor a software adapter was found".into()))?;
        let adapter_name = adapter.get_info().name;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).map_err(gpu)?;
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let vertex = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("transition vertex"), source: wgpu::ShaderSource::Wgsl(VERTEX.into()) });
        Ok(Self { device, queue, sampler, vertex, pipelines: HashMap::new(), targets: None, adapter_name })
    }

    /// Name of the adapter the frames are drawn on.
    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    fn picture_texture(&self, width: u32, height: u32, usage: wgpu::TextureUsages) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage,
            view_formats: &[],
        })
    }

    fn upload(&self, texture: &wgpu::Texture, width: u32, height: u32, rgba: &[u8]) {
        self.queue.write_texture(
            texture.as_image_copy(),
            rgba,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(width * 4), rows_per_image: Some(height) },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
    }

    fn make_targets(&self, width: u32, height: u32) -> Targets {
        let padded_row = (width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        Targets {
            width,
            height,
            from: self.picture_texture(width, height, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST),
            to: self.picture_texture(width, height, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST),
            output: self.picture_texture(width, height, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC),
            readback: self.device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: u64::from(padded_row) * u64::from(height),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }),
            padded_row,
        }
    }

    fn make_pipeline(&self, t: &'static Entry) -> Result<Pipeline, TransitionError> {
        let name = t.id.as_str();
        let fragment = self.device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(name), source: wgpu::ShaderSource::Wgsl(t.wgsl.as_str().into()) });

        let texture_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
            count: None,
        };
        let mut entries = vec![
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            },
            wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            texture_entry(2),
            texture_entry(3),
        ];
        let mut extras = Vec::new();
        for (i, name) in t.textures.iter().enumerate() {
            entries.push(texture_entry(4 + i as u32));
            let texture = self.picture_texture(EXTRA_SIZE, EXTRA_SIZE, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST);
            self.upload(&texture, EXTRA_SIZE, EXTRA_SIZE, &extra_picture(name));
            extras.push(texture.create_view(&wgpu::TextureViewDescriptor::default()));
        }
        let layout = self.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some(name), entries: &entries });
        let pipeline_layout = self.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some(name), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let pipeline = self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(name),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &self.vertex, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState {
                module: &fragment,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: FORMAT, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let uniforms = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(name),
            size: t.uniform_size() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Ok(Pipeline { pipeline, layout, uniforms, extras })
    }

    /// Draws the frame at `progress` (0 shows `from`, 1 shows `to`) into `out` as `width x height` RGBA rows with no padding.
    /// `from` and `to` are RGBA pictures of that size.
    pub fn render(&mut self, params: &Params, width: u32, height: u32, from: &[u8], to: &[u8], progress: f32, out: &mut Vec<u8>) -> Result<(), TransitionError> {
        let buffer = params.uniform_buffer(progress, width as f32 / height as f32);
        self.draw(params.entry(), width, height, from, Some(to), buffer, out)
    }

    /// Draws an effect or a filter on one RGBA picture of `width x height` into `out`. `progress` is how far the look's block has run (0 to 1),
    /// `intensity` how much of the change is mixed in (0 to 1) and `time` the seconds since the block started.
    #[allow(clippy::too_many_arguments)]
    pub fn render_look(&mut self, params: &Params, width: u32, height: u32, picture: &[u8], progress: f32, intensity: f32, time: f32, out: &mut Vec<u8>) -> Result<(), TransitionError> {
        let buffer = params.look_buffer(progress, width as f32 / height as f32, intensity, time);
        self.draw(params.entry(), width, height, picture, None, buffer, out)
    }

    #[allow(clippy::too_many_arguments)]
    fn draw(&mut self, t: &'static Entry, width: u32, height: u32, from: &[u8], to: Option<&[u8]>, buffer: Vec<u8>, out: &mut Vec<u8>) -> Result<(), TransitionError> {
        let expected = width as usize * height as usize * 4;
        for picture in std::iter::once(from).chain(to) {
            if picture.len() != expected {
                return Err(TransitionError::PictureSize { expected, got: picture.len() });
            }
        }
        let key = (t.kind, t.id.as_str());
        if !self.pipelines.contains_key(&key) {
            let pipeline = self.make_pipeline(t)?;
            self.pipelines.insert(key, pipeline);
        }
        if self.targets.as_ref().is_none_or(|x| x.width != width || x.height != height) {
            self.targets = Some(self.make_targets(width, height));
        }
        let targets = self.targets.as_ref().ok_or_else(|| gpu("no render targets"))?;
        let pipeline = &self.pipelines[&key];

        self.upload(&targets.from, width, height, from);
        if let Some(to) = to {
            self.upload(&targets.to, width, height, to);
        }
        self.queue.write_buffer(&pipeline.uniforms, 0, &buffer);

        let from_view = targets.from.create_view(&wgpu::TextureViewDescriptor::default());
        // A look has one picture: the second binding of the layout gets the same view.
        let to_view = if to.is_some() { targets.to.create_view(&wgpu::TextureViewDescriptor::default()) } else { targets.from.create_view(&wgpu::TextureViewDescriptor::default()) };
        let mut entries = vec![
            wgpu::BindGroupEntry { binding: 0, resource: pipeline.uniforms.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&from_view) },
            wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&to_view) },
        ];
        for (i, view) in pipeline.extras.iter().enumerate() {
            entries.push(wgpu::BindGroupEntry { binding: 4 + i as u32, resource: wgpu::BindingResource::TextureView(view) });
        }
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &pipeline.layout, entries: &entries });

        let output_view = targets.output.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &output_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&pipeline.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        encoder.copy_texture_to_buffer(
            targets.output.as_image_copy(),
            wgpu::TexelCopyBufferInfo { buffer: &targets.readback, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(targets.padded_row), rows_per_image: Some(height) } },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        self.queue.submit([encoder.finish()]);

        let slice = targets.readback.slice(..);
        let (tx, rx) = mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        self.device.poll(wgpu::PollType::wait_indefinitely()).map_err(gpu)?;
        rx.recv().map_err(gpu)?.map_err(gpu)?;
        {
            let mapped = slice.get_mapped_range().map_err(gpu)?;
            out.clear();
            out.reserve(expected);
            let row = width as usize * 4;
            for y in 0..height as usize {
                let start = y * targets.padded_row as usize;
                out.extend_from_slice(&mapped[start..start + row]);
            }
        }
        targets.readback.unmap();
        Ok(())
    }
}

/// The pictures a few transitions sample besides the two they blend. They are made here so no file ships with the app: `luma` is a
/// soft diagonal ramp, `displacementMap` smooth noise.
fn extra_picture(name: &str) -> Vec<u8> {
    let size = EXTRA_SIZE as usize;
    let mut out = Vec::with_capacity(size * size * 4);
    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = (x as f32 / (size - 1) as f32, y as f32 / (size - 1) as f32);
            let value = if name == "luma" { (fx + fy) * 0.5 } else { smooth_noise(fx * 8.0, fy * 8.0) };
            let byte = (value.clamp(0.0, 1.0) * 255.0).round() as u8;
            out.extend_from_slice(&[byte, byte, byte, 255]);
        }
    }
    out
}

fn lattice(x: i32, y: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x9E37_79B1) ^ (y as u32).wrapping_mul(0x85EB_CA6B);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    (h & 0xFFFF) as f32 / 65535.0
}

fn smooth_noise(x: f32, y: f32) -> f32 {
    let (x0, y0) = (x.floor(), y.floor());
    let (tx, ty) = (x - x0, y - y0);
    let (sx, sy) = (tx * tx * (3.0 - 2.0 * tx), ty * ty * (3.0 - 2.0 * ty));
    let (ix, iy) = (x0 as i32, y0 as i32);
    let top = lattice(ix, iy) * (1.0 - sx) + lattice(ix + 1, iy) * sx;
    let bottom = lattice(ix, iy + 1) * (1.0 - sx) + lattice(ix + 1, iy + 1) * sx;
    top * (1.0 - sy) + bottom * sy
}
