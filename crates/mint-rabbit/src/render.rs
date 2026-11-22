//! The Model View renderer: draws a posed project with wgpu inside the egui
//! render pass.

use std::sync::Arc;

use eframe::egui_wgpu::wgpu::util::DeviceExt as _;
use eframe::egui_wgpu::{self, CallbackResources, CallbackTrait, wgpu};
use glam::Mat4;
use mint_core::{AlphaMode, Project, Wrap};

use crate::scene::{self, GpuVertex};

/// Depth format of the egui render pass; must match `depth_buffer` in the
/// native options.
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
pub const DEPTH_BITS: u8 = 32;

const SHADER: &str = r"
struct Globals {
    view_proj: mat4x4<f32>,
    light_dir: vec4<f32>,
};
struct MaterialU {
    tint: vec4<f32>,
    cutout: vec4<f32>,
};
@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;
@group(1) @binding(2) var<uniform> material: MaterialU;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
};

@vertex
fn vs_main(@location(0) pos: vec3<f32>, @location(1) normal: vec3<f32>, @location(2) uv: vec2<f32>) -> VsOut {
    var out: VsOut;
    out.clip = globals.view_proj * vec4<f32>(pos, 1.0);
    out.normal = normal;
    out.uv = uv;
    return out;
}

@fragment
fn fs_main(in: VsOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    // The game's combiner tints color only; alpha comes from the texture.
    let texel = textureSample(tex, samp, in.uv);
    let c = vec4<f32>(texel.rgb * material.tint.rgb, texel.a);
    if material.cutout.x > 0.5 && c.a < 0.5 {
        discard;
    }
    var n = normalize(in.normal);
    if !front {
        n = -n;
    }
    let light = 0.45 + 0.55 * max(dot(n, globals.light_dir.xyz), 0.0);
    return vec4<f32>(c.rgb * light, c.a);
}
";

/// Everything the renderer draws, rebuilt when the project or the chosen
/// variants change.
pub struct SceneData {
    pub generation: u64,
    pub mesh: scene::PosedMesh,
    /// Per material: texture pixels (RGBA8), size, wrap, alpha, culling,
    /// tint.
    pub materials: Vec<MaterialData>,
}

pub struct MaterialData {
    pub pixels: Vec<u8>,
    pub size: [u32; 2],
    pub wrap: [Wrap; 2],
    pub alpha: AlphaMode,
    pub double_sided: bool,
    pub tint: [f32; 4],
}

impl SceneData {
    /// Prepares `project`, showing `variants[t]` of each texture `t`.
    pub fn new(project: &Project, variants: &[usize], generation: u64) -> Self {
        let materials = project
            .materials()
            .iter()
            .map(|m| {
                let texture = project.texture(m.texture);
                let pixels = scene::texture_pixels(project, texture, variants[m.texture.index()]);
                // The tint is a full RGBA8 register, not a palette color.
                let tint = m.tint;
                MaterialData {
                    pixels: pixels.iter().flat_map(|c| c.to_array()).collect(),
                    size: texture.size().to_array(),
                    wrap: m.wrap,
                    alpha: m.alpha,
                    double_sided: m.double_sided,
                    tint: tint.to_array().map(|c| f32::from(c) / 255.0),
                }
            })
            .collect();
        Self {
            generation,
            mesh: scene::posed_mesh(project),
            materials,
        }
    }
}

/// One frame's draw request.
pub struct ModelCallback {
    pub scene: Arc<SceneData>,
    pub view_proj: Mat4,
    pub linear_filter: bool,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    view_proj: [[f32; 4]; 4],
    light_dir: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct MaterialUniform {
    tint: [f32; 4],
    cutout: [f32; 4],
}

struct GpuMaterial {
    bind_group: wgpu::BindGroup,
    pipeline: usize,
}

struct GpuScene {
    generation: u64,
    linear_filter: bool,
    vertices: wgpu::Buffer,
    materials: Vec<GpuMaterial>,
    /// (first, count, material) in draw order: opaque before translucent.
    draws: Vec<(u32, u32, usize)>,
}

/// Long-lived GPU state, stored in egui's callback resources.
pub struct Renderer {
    /// Indexed by `pipeline_index(alpha, double_sided)`.
    pipelines: Vec<wgpu::RenderPipeline>,
    globals: wgpu::Buffer,
    globals_bind_group: wgpu::BindGroup,
    material_layout: wgpu::BindGroupLayout,
    srgb_textures: bool,
    scene: Option<GpuScene>,
}

fn pipeline_index(alpha: AlphaMode, double_sided: bool) -> usize {
    usize::from(alpha == AlphaMode::Translucent) * 2 + usize::from(double_sided)
}

fn address(w: Wrap) -> wgpu::AddressMode {
    match w {
        Wrap::Clamp => wgpu::AddressMode::ClampToEdge,
        Wrap::Repeat => wgpu::AddressMode::Repeat,
        Wrap::Mirror => wgpu::AddressMode::MirrorRepeat,
    }
}

impl Renderer {
    /// Creates the renderer and stores it in `render_state`'s callback
    /// resources.
    pub fn install(render_state: &egui_wgpu::RenderState) {
        let renderer = Self::new(&render_state.device, render_state.target_format);
        render_state
            .renderer
            .write()
            .callback_resources
            .insert(renderer);
    }

    /// Creates a renderer drawing into `target_format` with a
    /// [`DEPTH_FORMAT`] depth buffer.
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("model view"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("material"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("model view"),
            bind_group_layouts: &[Some(&globals_layout), Some(&material_layout)],
            immediate_size: 0,
        });
        let pipelines = (0..4)
            .map(|i| {
                let translucent = i >= 2;
                let double_sided = i % 2 == 1;
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("model view"),
                    layout: Some(&layout),
                    vertex: wgpu::VertexState {
                        module: &shader,
                        entry_point: Some("vs_main"),
                        compilation_options: wgpu::PipelineCompilationOptions::default(),
                        buffers: &[Some(wgpu::VertexBufferLayout {
                            array_stride: std::mem::size_of::<GpuVertex>() as u64,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2],
                        })],
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &shader,
                        entry_point: Some("fs_main"),
                        compilation_options: wgpu::PipelineCompilationOptions::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: target_format,
                            blend: translucent.then_some(wgpu::BlendState::ALPHA_BLENDING),
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    primitive: wgpu::PrimitiveState {
                        cull_mode: (!double_sided).then_some(wgpu::Face::Back),
                        ..wgpu::PrimitiveState::default()
                    },
                    depth_stencil: Some(wgpu::DepthStencilState {
                        format: DEPTH_FORMAT,
                        depth_write_enabled: Some(!translucent),
                        depth_compare: Some(wgpu::CompareFunction::Less),
                        stencil: wgpu::StencilState::default(),
                        bias: wgpu::DepthBiasState::default(),
                    }),
                    multisample: wgpu::MultisampleState::default(),
                    multiview_mask: None,
                    cache: None,
                })
            })
            .collect();
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            }],
        });
        Self {
            pipelines,
            globals,
            globals_bind_group,
            material_layout,
            srgb_textures: target_format.is_srgb(),
            scene: None,
        }
    }

    /// Uploads the scene if it changed and writes this frame's uniforms.
    pub fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, frame: &ModelCallback) {
        let stale = self.scene.as_ref().is_none_or(|s| {
            s.generation != frame.scene.generation || s.linear_filter != frame.linear_filter
        });
        if stale {
            self.upload(device, queue, &frame.scene, frame.linear_filter);
        }
        let light = glam::Vec3::new(0.4, 0.8, 0.6).normalize();
        let globals = Globals {
            view_proj: frame.view_proj.to_cols_array_2d(),
            light_dir: light.extend(0.0).to_array(),
        };
        queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&globals));
    }

    /// Draws the prepared scene into `pass`.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        let Some(scene) = &self.scene else { return };
        pass.set_bind_group(0, &self.globals_bind_group, &[]);
        pass.set_vertex_buffer(0, scene.vertices.slice(..));
        for &(first, count, m) in &scene.draws {
            let material = &scene.materials[m];
            pass.set_pipeline(&self.pipelines[material.pipeline]);
            pass.set_bind_group(1, &material.bind_group, &[]);
            pass.draw(first..first + count, 0..1);
        }
    }

    fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        data: &SceneData,
        linear_filter: bool,
    ) {
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("model vertices"),
            contents: bytemuck::cast_slice(&data.mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let filter = if linear_filter {
            wgpu::FilterMode::Linear
        } else {
            wgpu::FilterMode::Nearest
        };
        let format = if self.srgb_textures {
            wgpu::TextureFormat::Rgba8UnormSrgb
        } else {
            wgpu::TextureFormat::Rgba8Unorm
        };
        let materials = data
            .materials
            .iter()
            .map(|m| {
                let texture = device.create_texture_with_data(
                    queue,
                    &wgpu::TextureDescriptor {
                        label: Some("material texture"),
                        size: wgpu::Extent3d {
                            width: m.size[0],
                            height: m.size[1],
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage: wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    },
                    wgpu::util::TextureDataOrder::LayerMajor,
                    &m.pixels,
                );
                let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                    label: Some("material sampler"),
                    address_mode_u: address(m.wrap[0]),
                    address_mode_v: address(m.wrap[1]),
                    mag_filter: filter,
                    min_filter: filter,
                    ..wgpu::SamplerDescriptor::default()
                });
                let uniform = MaterialUniform {
                    tint: m.tint,
                    cutout: [
                        f32::from(u8::from(m.alpha == AlphaMode::Cutout)),
                        0.0,
                        0.0,
                        0.0,
                    ],
                };
                let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("material uniform"),
                    contents: bytemuck::bytes_of(&uniform),
                    usage: wgpu::BufferUsages::UNIFORM,
                });
                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("material"),
                    layout: &self.material_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&sampler),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: buffer.as_entire_binding(),
                        },
                    ],
                });
                GpuMaterial {
                    bind_group,
                    pipeline: pipeline_index(m.alpha, m.double_sided),
                }
            })
            .collect();
        let mut draws: Vec<(u32, u32, usize)> = data
            .mesh
            .batches
            .iter()
            .map(|b| (b.first, b.count, b.material.index()))
            .collect();
        draws.sort_by_key(|&(_, _, m)| data.materials[m].alpha == AlphaMode::Translucent);
        self.scene = Some(GpuScene {
            generation: data.generation,
            linear_filter,
            vertices,
            materials,
            draws,
        });
    }
}

impl CallbackTrait for ModelCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(r) = resources.get_mut::<Renderer>() {
            r.prepare(device, queue, self);
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: eframe::egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &CallbackResources,
    ) {
        if let Some(r) = resources.get::<Renderer>() {
            r.draw(pass);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::Orbit;

    const SIZE: u32 = 512;

    /// Imports the first villager of the decomp in `MINT_AC_DECOMP`, renders
    /// it offscreen with the Model View pipeline, and writes the image to
    /// `MINT_SNAPSHOT` (default `model-view.png` in the crate directory).
    #[test]
    #[ignore = "needs a GPU adapter and a decomp in MINT_AC_DECOMP"]
    fn model_view_snapshot() {
        let root = std::env::var("MINT_AC_DECOMP").expect("MINT_AC_DECOMP");
        let out = std::env::var("MINT_SNAPSHOT").unwrap_or_else(|_| "model-view.png".into());
        let decomp = mint_ac::Decomp::open(&root, "GAFE01_00").unwrap();
        let villager = decomp.villagers().unwrap().remove(0);
        let project = decomp.import(&villager).unwrap();
        let scene = Arc::new(SceneData::new(
            &project,
            &vec![0; project.textures().len()],
            1,
        ));
        let camera = Orbit::framing(scene.mesh.bounds_min, scene.mesh.bounds_max);

        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
                .unwrap();
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let target = |format, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width: SIZE,
                    height: SIZE,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let color = target(
            format,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let depth = target(DEPTH_FORMAT, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let color_view = color.create_view(&wgpu::TextureViewDescriptor::default());
        let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());

        let mut renderer = Renderer::new(&device, format);
        let frame = ModelCallback {
            scene,
            view_proj: camera.view_proj(1.0),
            linear_filter: true,
        };
        renderer.prepare(&device, &queue, &frame);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.1,
                            g: 0.1,
                            b: 0.12,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                ..wgpu::RenderPassDescriptor::default()
            });
            renderer.draw(&mut pass);
        }
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(SIZE * SIZE * 4),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            color.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(SIZE * 4),
                    rows_per_image: Some(SIZE),
                },
            },
            color.size(),
        );
        queue.submit([encoder.finish()]);
        readback.map_async(wgpu::MapMode::Read, .., |r| r.unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let pixels = readback.get_mapped_range(..).unwrap().to_vec();

        let file = std::fs::File::create(&out).unwrap();
        let mut png = png::Encoder::new(std::io::BufWriter::new(file), SIZE, SIZE);
        png.set_color(png::ColorType::Rgba);
        png.write_header()
            .unwrap()
            .write_image_data(&pixels)
            .unwrap();
        eprintln!("wrote {out}");
        // Something besides the clear color was drawn.
        assert!(pixels.chunks(4).any(|p| p[0] > 60), "the render is blank");
    }
}
