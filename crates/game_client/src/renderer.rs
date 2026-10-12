use std::collections::HashMap;
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use core_sim::ChunkCoord;
use wgpu::util::DeviceExt;
use winit::window::Window;

use crate::camera::Camera;
use crate::mesh::{capsule_mesh, terrain_chunk_mesh, Vertex};

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// Linear-space haze colour; also used as the clear colour so distant terrain fades into it.
const FOG_COLOR: [f32; 3] = [0.26, 0.19, 0.14];
const FOG_DENSITY: f32 = 1.0 / 340.0;
const SUN_DIR: [f32; 3] = [0.55, 0.45, 0.45];
/// Terrain meshes built per frame, so streaming never causes a long hitch.
const CHUNK_BUILDS_PER_FRAME: usize = 6;

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct Globals {
    view_proj: [[f32; 4]; 4],
    cam_pos: [f32; 4],
    sun_dir: [f32; 4],
    fog: [f32; 4],
}

/// Per-agent GPU instance data.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct AgentInstance {
    /// World position of the agent's feet (w unused).
    pub pos: [f32; 4],
    /// Linear-ish sRGB tint (a unused).
    pub color: [f32; 4],
}

struct GpuMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
}

impl GpuMesh {
    fn new(device: &wgpu::Device, label: &str, v: &[Vertex], i: &[u32]) -> Self {
        Self {
            vertices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytemuck::cast_slice(v),
                usage: wgpu::BufferUsages::VERTEX,
            }),
            indices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytemuck::cast_slice(i),
                usage: wgpu::BufferUsages::INDEX,
            }),
            index_count: i.len() as u32,
        }
    }
}

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    sample_count: u32,
    depth_view: wgpu::TextureView,
    msaa_view: Option<wgpu::TextureView>,
    globals: wgpu::Buffer,
    globals_bind: wgpu::BindGroup,
    terrain_pipeline: wgpu::RenderPipeline,
    agent_pipeline: wgpu::RenderPipeline,
    seed: u64,
    chunks: HashMap<ChunkCoord, GpuMesh>,
    agent_mesh: GpuMesh,
    instances: wgpu::Buffer,
    instance_capacity: usize,
    instance_count: u32,
}

impl Renderer {
    pub fn new(window: Arc<Window>, seed: u64) -> Result<Self, String> {
        let size = window.inner_size();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
        let surface = instance
            .create_surface(window)
            .map_err(|e| format!("create surface: {e}"))?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .ok_or("no suitable GPU adapter found")?;
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("ashen-world device"),
                required_features: wgpu::Features::empty(),
                required_limits:
                    wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
                memory_hints: wgpu::MemoryHints::default(),
            },
            None,
        ))
        .map_err(|e| format!("request device: {e}"))?;

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&device, &config);

        let supports_4x = adapter
            .get_texture_format_features(format)
            .flags
            .sample_count_supported(4)
            && adapter
                .get_texture_format_features(DEPTH_FORMAT)
                .flags
                .sample_count_supported(4);
        let sample_count = if supports_4x { 4 } else { 1 };

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals layout"),
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
        let globals_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals bind"),
            layout: &bind_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pipeline layout"),
            bind_group_layouts: &[&bind_layout],
            push_constant_ranges: &[],
        });

        let vertex_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3];
        let instance_attrs = wgpu::vertex_attr_array![2 => Float32x4, 3 => Float32x4];
        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &vertex_attrs,
        };
        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<AgentInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &instance_attrs,
        };
        let make_pipeline =
            |label: &str, vs: &str, fs: &str, buffers: &[wgpu::VertexBufferLayout]| {
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(label),
                    layout: Some(&layout),
                    vertex: wgpu::VertexState {
                        module: &shader,
                        entry_point: vs,
                        compilation_options: Default::default(),
                        buffers,
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &shader,
                        entry_point: fs,
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format,
                            blend: Some(wgpu::BlendState::REPLACE),
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    primitive: wgpu::PrimitiveState {
                        topology: wgpu::PrimitiveTopology::TriangleList,
                        cull_mode: Some(wgpu::Face::Back),
                        ..Default::default()
                    },
                    depth_stencil: Some(wgpu::DepthStencilState {
                        format: DEPTH_FORMAT,
                        depth_write_enabled: true,
                        depth_compare: wgpu::CompareFunction::Less,
                        stencil: wgpu::StencilState::default(),
                        bias: wgpu::DepthBiasState::default(),
                    }),
                    multisample: wgpu::MultisampleState {
                        count: sample_count,
                        mask: !0,
                        alpha_to_coverage_enabled: false,
                    },
                    multiview: None,
                    cache: None,
                })
            };
        let terrain_pipeline = make_pipeline(
            "terrain",
            "vs_terrain",
            "fs_terrain",
            std::slice::from_ref(&vertex_layout),
        );
        let agent_pipeline = make_pipeline(
            "agents",
            "vs_agent",
            "fs_agent",
            &[vertex_layout.clone(), instance_layout],
        );

        let (cv, ci) = capsule_mesh();
        let agent_mesh = GpuMesh::new(&device, "capsule", &cv, &ci);
        let instance_capacity = 1024;
        let instances = Self::make_instance_buffer(&device, instance_capacity);
        let (depth_view, msaa_view) = Self::make_targets(&device, &config, sample_count);

        Ok(Self {
            surface,
            device,
            queue,
            config,
            sample_count,
            depth_view,
            msaa_view,
            globals,
            globals_bind,
            terrain_pipeline,
            agent_pipeline,
            seed,
            chunks: HashMap::new(),
            agent_mesh,
            instances,
            instance_capacity,
            instance_count: 0,
        })
    }

    fn make_instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("agent instances"),
            size: (capacity * std::mem::size_of::<AgentInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    fn make_targets(
        device: &wgpu::Device,
        config: &wgpu::SurfaceConfiguration,
        sample_count: u32,
    ) -> (wgpu::TextureView, Option<wgpu::TextureView>) {
        let extent = wgpu::Extent3d {
            width: config.width,
            height: config.height,
            depth_or_array_layers: 1,
        };
        let depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size: extent,
            mip_level_count: 1,
            sample_count,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let msaa = (sample_count > 1).then(|| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("msaa color"),
                    size: extent,
                    mip_level_count: 1,
                    sample_count,
                    dimension: wgpu::TextureDimension::D2,
                    format: config.format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        });
        (
            depth.create_view(&wgpu::TextureViewDescriptor::default()),
            msaa,
        )
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        let (d, m) = Self::make_targets(&self.device, &self.config, self.sample_count);
        self.depth_view = d;
        self.msaa_view = m;
    }

    pub fn aspect(&self) -> f32 {
        self.config.width as f32 / self.config.height as f32
    }

    pub fn loaded_chunks(&self) -> usize {
        self.chunks.len()
    }

    /// Keep terrain meshes loaded for chunks within `radius` of `center`: build a bounded
    /// number of missing ones (nearest first) and drop those that are out of range.
    pub fn update_chunks(&mut self, center: ChunkCoord, radius: i32) {
        self.chunks.retain(|c, _| {
            (c.x - center.x).abs() <= radius + 1 && (c.z - center.z).abs() <= radius + 1
        });
        let mut missing: Vec<ChunkCoord> = (center.z - radius..=center.z + radius)
            .flat_map(|z| {
                (center.x - radius..=center.x + radius).map(move |x| ChunkCoord::new(x, z))
            })
            .filter(|c| !self.chunks.contains_key(c))
            .collect();
        missing.sort_by_key(|c| (c.x - center.x).pow(2) + (c.z - center.z).pow(2));
        for c in missing.into_iter().take(CHUNK_BUILDS_PER_FRAME) {
            let (v, i) = terrain_chunk_mesh(self.seed, c);
            self.chunks
                .insert(c, GpuMesh::new(&self.device, "terrain chunk", &v, &i));
        }
    }

    pub fn render(
        &mut self,
        camera: &Camera,
        agents: &[AgentInstance],
    ) -> Result<(), wgpu::SurfaceError> {
        if agents.len() > self.instance_capacity {
            self.instance_capacity = agents.len().next_power_of_two();
            self.instances = Self::make_instance_buffer(&self.device, self.instance_capacity);
        }
        self.queue
            .write_buffer(&self.instances, 0, bytemuck::cast_slice(agents));
        self.instance_count = agents.len() as u32;

        let eye = camera.eye();
        let globals = Globals {
            view_proj: camera.view_proj().to_cols_array_2d(),
            cam_pos: [eye.x, eye.y, eye.z, 1.0],
            sun_dir: [SUN_DIR[0], SUN_DIR[1], SUN_DIR[2], 0.0],
            fog: [FOG_COLOR[0], FOG_COLOR[1], FOG_COLOR[2], FOG_DENSITY],
        };
        self.queue
            .write_buffer(&self.globals, 0, bytemuck::bytes_of(&globals));

        let frame = self.surface.get_current_texture()?;
        let frame_view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let (view, resolve) = match &self.msaa_view {
            Some(m) => (m, Some(&frame_view)),
            None => (&frame_view, None),
        };
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: resolve,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: FOG_COLOR[0] as f64,
                            g: FOG_COLOR[1] as f64,
                            b: FOG_COLOR[2] as f64,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_bind_group(0, &self.globals_bind, &[]);
            pass.set_pipeline(&self.terrain_pipeline);
            for mesh in self.chunks.values() {
                pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.index_count, 0, 0..1);
            }
            if self.instance_count > 0 {
                pass.set_pipeline(&self.agent_pipeline);
                pass.set_vertex_buffer(0, self.agent_mesh.vertices.slice(..));
                pass.set_vertex_buffer(1, self.instances.slice(..));
                pass.set_index_buffer(self.agent_mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..self.agent_mesh.index_count, 0, 0..self.instance_count);
            }
        }
        self.queue.submit(Some(encoder.finish()));
        frame.present();
        Ok(())
    }
}
