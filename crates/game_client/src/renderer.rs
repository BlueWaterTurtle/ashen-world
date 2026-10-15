use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use core_sim::{Building, ChunkCoord, Footprint, TerrainEdits};
use wgpu::util::DeviceExt;
use winit::window::Window;

use crate::assets;
use crate::camera::Camera;
use crate::mesh::{building_mesh, humanoid_mesh, terrain_chunk_mesh, Vertex};
use crate::ui::GameUi;

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

pub type BuildingInstance = AgentInstance;

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct MaterialUniform {
    base_color: [f32; 4],
}

struct GpuMaterial {
    _texture: wgpu::Texture,
    _uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

struct GpuMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
}

struct GpuBuilding {
    mesh: GpuMesh,
    surfaces: Vec<asset_pipeline::Surface>,
    materials: Vec<GpuMaterial>,
    textured: bool,
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
    building_pipeline: wgpu::RenderPipeline,
    white_material: GpuMaterial,
    terrain_material: GpuMaterial,
    material_layout: wgpu::BindGroupLayout,
    material_sampler: wgpu::Sampler,
    seed: u64,
    model_dir: PathBuf,
    chunks: HashMap<ChunkCoord, GpuMesh>,
    chunk_revisions: HashMap<ChunkCoord, u64>,
    agent_mesh: GpuMesh,
    agent_surfaces: Vec<asset_pipeline::Surface>,
    agent_materials: Vec<GpuMaterial>,
    building_meshes: HashMap<String, GpuBuilding>,
    building_instances: wgpu::Buffer,
    building_instance_capacity: usize,
    building_instance_count: u32,
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
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("material layout"),
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
            label: Some("pipeline layout"),
            bind_group_layouts: &[&bind_layout, &material_layout],
            push_constant_ranges: &[],
        });

        let vertex_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2];
        let instance_attrs = wgpu::vertex_attr_array![3 => Float32x4, 4 => Float32x4];
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
            &[vertex_layout.clone(), instance_layout.clone()],
        );
        let building_pipeline = make_pipeline(
            "buildings",
            "vs_agent",
            "fs_agent",
            &[vertex_layout.clone(), instance_layout.clone()],
        );

        let model_dir = assets::root().join("models");
        let character_model =
            asset_pipeline::load_gltf(model_dir.join("Humanoids/Superhero_Male_FullBody.gltf"))
                .and_then(|mut model| {
                    model.fit_height(1.8)?;
                    Ok(model)
                });
        let character_model = character_model.or_else(|gltf_error| {
            eprintln!("could not load character glTF: {gltf_error}");
            asset_pipeline::load_obj(model_dir.join("humanoid.obj"))
        });
        let (cv, ci, agent_surfaces, material_definitions) = match character_model {
            Ok(model) => (
                model.vertices,
                model.indices,
                model.surfaces,
                model.materials,
            ),
            Err(error) => {
                eprintln!("could not load humanoid placeholder model: {error}");
                let (vertices, indices) = humanoid_mesh();
                let surfaces = vec![asset_pipeline::Surface {
                    first_index: 0,
                    index_count: indices.len() as u32,
                    material_index: 0,
                }];
                (vertices, indices, surfaces, Vec::new())
            }
        };
        let agent_mesh = GpuMesh::new(&device, "humanoid", &cv, &ci);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("base color sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let white_material = create_gpu_material(
            &device,
            &queue,
            &material_layout,
            &sampler,
            [1.0; 4],
            None,
            "white fallback material",
        );
        let terrain_noise =
            asset_pipeline::load_texture_file(model_dir.join("textures/T_Noise_Terrain.png"))
                .map_err(|error| {
                    eprintln!("could not load terrain noise texture: {error}");
                    error
                })
                .ok();
        let terrain_material = create_gpu_material(
            &device,
            &queue,
            &material_layout,
            &sampler,
            [1.0; 4],
            terrain_noise.as_deref(),
            "terrain noise material",
        );
        let agent_materials = material_definitions
            .iter()
            .enumerate()
            .map(|(index, material)| {
                create_gpu_material(
                    &device,
                    &queue,
                    &material_layout,
                    &sampler,
                    material.base_color,
                    material.base_color_texture.as_deref(),
                    &format!("agent material {index}"),
                )
            })
            .collect();
        let instance_capacity = 1024;
        let instances = Self::make_instance_buffer(&device, instance_capacity);
        let building_instance_capacity = 128;
        let building_instances = Self::make_instance_buffer(&device, building_instance_capacity);
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
            building_pipeline,
            white_material,
            terrain_material,
            material_layout,
            material_sampler: sampler,
            seed,
            model_dir,
            chunks: HashMap::new(),
            chunk_revisions: HashMap::new(),
            agent_mesh,
            agent_surfaces,
            agent_materials,
            building_meshes: HashMap::new(),
            building_instances,
            building_instance_capacity,
            building_instance_count: 0,
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

    pub fn create_ui(&self, window: &Arc<Window>) -> GameUi {
        GameUi::new(window, &self.device, self.config.format)
    }

    pub fn loaded_chunks(&self) -> usize {
        self.chunks.len()
    }

    /// Keep terrain meshes loaded for chunks within `radius` of `center`: build a bounded
    /// number of missing ones (nearest first) and drop those that are out of range.
    /// Chunks whose terrain edit revision changed are rebuilt through the same budget.
    pub fn update_chunks(&mut self, center: ChunkCoord, radius: i32, edits: &TerrainEdits) {
        self.chunks.retain(|c, _| {
            (c.x - center.x).abs() <= radius + 1 && (c.z - center.z).abs() <= radius + 1
        });
        self.chunk_revisions
            .retain(|c, _| self.chunks.contains_key(c));
        let mut missing: Vec<ChunkCoord> = (center.z - radius..=center.z + radius)
            .flat_map(|z| {
                (center.x - radius..=center.x + radius).map(move |x| ChunkCoord::new(x, z))
            })
            .filter(|c| {
                !self.chunks.contains_key(c)
                    || self.chunk_revisions.get(c) != Some(&edits.revision(*c))
            })
            .collect();
        missing.sort_by_key(|c| (c.x - center.x).pow(2) + (c.z - center.z).pow(2));
        for c in missing.into_iter().take(CHUNK_BUILDS_PER_FRAME) {
            let (v, i) = terrain_chunk_mesh(self.seed, edits, c);
            self.chunk_revisions.insert(c, edits.revision(c));
            self.chunks
                .insert(c, GpuMesh::new(&self.device, "terrain chunk", &v, &i));
        }
    }

    pub fn render(
        &mut self,
        camera: &Camera,
        agents: &[AgentInstance],
        buildings: &[Building],
        edits: &TerrainEdits,
        ui: &mut GameUi,
        pixels_per_point: f32,
    ) -> Result<(), wgpu::SurfaceError> {
        if agents.len() > self.instance_capacity {
            self.instance_capacity = agents.len().next_power_of_two();
            self.instances = Self::make_instance_buffer(&self.device, self.instance_capacity);
        }
        self.queue
            .write_buffer(&self.instances, 0, bytemuck::cast_slice(agents));
        self.instance_count = agents.len() as u32;

        let mut building_instances = Vec::with_capacity(buildings.len());
        for building in buildings {
            if !self.building_meshes.contains_key(&building.def) {
                let asset_root = self.model_dir.parent().unwrap_or(&self.model_dir);
                let model = load_building_model(
                    asset_root,
                    &self.model_dir,
                    building.footprint,
                    &building.def,
                );
                let surfaces = if model.surfaces.is_empty() {
                    vec![asset_pipeline::Surface {
                        first_index: 0,
                        index_count: model.indices.len() as u32,
                        material_index: 0,
                    }]
                } else {
                    model.surfaces.clone()
                };
                let textured = model
                    .materials
                    .iter()
                    .any(|material| material.base_color_texture.is_some());
                let materials = model
                    .materials
                    .iter()
                    .enumerate()
                    .map(|(index, material)| {
                        create_gpu_material(
                            &self.device,
                            &self.queue,
                            &self.material_layout,
                            &self.material_sampler,
                            material.base_color,
                            material.base_color_texture.as_deref(),
                            &format!("{} material {index}", building.def),
                        )
                    })
                    .collect();
                let mesh =
                    GpuMesh::new(&self.device, &building.def, &model.vertices, &model.indices);
                self.building_meshes.insert(
                    building.def.clone(),
                    GpuBuilding {
                        mesh,
                        surfaces,
                        materials,
                        textured,
                    },
                );
            }
            let x = building.footprint.x as f32;
            let z = building.footprint.z as f32;
            let y = edits.height(
                self.seed,
                x + building.footprint.width as f32 * 0.5,
                z + building.footprint.depth as f32 * 0.5,
            );
            let color = if self.building_meshes[&building.def].textured {
                [1.0; 4]
            } else {
                building_color(&building.def)
            };
            building_instances.push((
                building.def.as_str(),
                BuildingInstance {
                    pos: [x, y, z, 0.0],
                    color,
                },
            ));
        }
        if building_instances.len() > self.building_instance_capacity {
            self.building_instance_capacity = building_instances.len().next_power_of_two();
            self.building_instances =
                Self::make_instance_buffer(&self.device, self.building_instance_capacity);
        }
        let gpu_building_instances: Vec<BuildingInstance> = building_instances
            .iter()
            .map(|(_, instance)| *instance)
            .collect();
        self.queue.write_buffer(
            &self.building_instances,
            0,
            bytemuck::cast_slice(&gpu_building_instances),
        );
        self.building_instance_count = gpu_building_instances.len() as u32;

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
            pass.set_bind_group(1, &self.terrain_material.bind_group, &[]);
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
                for surface in &self.agent_surfaces {
                    let material = self
                        .agent_materials
                        .get(surface.material_index)
                        .unwrap_or(&self.white_material);
                    pass.set_bind_group(1, &material.bind_group, &[]);
                    pass.draw_indexed(
                        surface.first_index..surface.first_index + surface.index_count,
                        0,
                        0..self.instance_count,
                    );
                }
            }
            if self.building_instance_count > 0 {
                pass.set_pipeline(&self.building_pipeline);
                pass.set_bind_group(1, &self.white_material.bind_group, &[]);
                for (index, (definition, _)) in building_instances.iter().enumerate() {
                    let Some(building) = self.building_meshes.get(*definition) else {
                        continue;
                    };
                    pass.set_vertex_buffer(0, building.mesh.vertices.slice(..));
                    pass.set_vertex_buffer(1, self.building_instances.slice(..));
                    pass.set_index_buffer(
                        building.mesh.indices.slice(..),
                        wgpu::IndexFormat::Uint32,
                    );
                    for surface in &building.surfaces {
                        let material = building
                            .materials
                            .get(surface.material_index)
                            .unwrap_or(&self.white_material);
                        pass.set_bind_group(1, &material.bind_group, &[]);
                        pass.draw_indexed(
                            surface.first_index..surface.first_index + surface.index_count,
                            0,
                            index as u32..index as u32 + 1,
                        );
                    }
                }
            }
        }
        let ui_commands = ui.paint(
            &self.device,
            &self.queue,
            &mut encoder,
            &frame_view,
            [self.config.width, self.config.height],
            pixels_per_point,
        );
        self.queue.submit(
            ui_commands
                .into_iter()
                .chain(std::iter::once(encoder.finish())),
        );
        frame.present();
        Ok(())
    }
}

fn load_building_model(
    asset_root: &Path,
    model_dir: &Path,
    footprint: Footprint,
    definition: &str,
) -> asset_pipeline::Model {
    if !definition
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
    {
        return procedural_building_model(footprint, definition);
    }

    let prefab_path = asset_root
        .join("prefabs")
        .join("buildings")
        .join(format!("{definition}.ron"));
    if prefab_path.is_file() {
        match asset_pipeline::load_prefab(asset_root, &prefab_path) {
            Ok(model) => return model,
            Err(error) => {
                eprintln!(
                    "could not load building prefab {}: {error}",
                    prefab_path.display()
                );
            }
        }
    }

    let model_path = model_dir
        .join("buildings")
        .join(format!("{definition}.gltf"));
    let imported = asset_pipeline::load_gltf(&model_path)
        .or_else(|_| asset_pipeline::load_obj(model_path.with_extension("obj")));
    match imported {
        Ok(mut model) => match model.scale([footprint.width as f32, 1.0, footprint.depth as f32]) {
            Ok(()) => model,
            Err(error) => {
                eprintln!(
                    "could not scale building model {}: {error}",
                    model_path.display()
                );
                procedural_building_model(footprint, definition)
            }
        },
        Err(error) => {
            eprintln!(
                "could not load legacy building model {}: {error}",
                model_path.display()
            );
            procedural_building_model(footprint, definition)
        }
    }
}

fn procedural_building_model(footprint: Footprint, definition: &str) -> asset_pipeline::Model {
    let (vertices, indices) = building_mesh(footprint, definition);
    asset_pipeline::Model {
        vertices,
        indices,
        ..Default::default()
    }
}

fn create_gpu_material(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    base_color: [f32; 4],
    source: Option<&asset_pipeline::TextureData>,
    label: &str,
) -> GpuMaterial {
    let fallback = [255_u8; 4];
    let (width, height, pixels) = source.map_or((1, 1, fallback.as_slice()), |image| {
        (image.width, image.height, image.rgba8.as_ref())
    });
    let unpadded_bytes_per_row = width * 4;
    let alignment = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded_bytes_per_row = unpadded_bytes_per_row.div_ceil(alignment) * alignment;
    let mut upload = vec![0; padded_bytes_per_row as usize * height as usize];
    for row in 0..height as usize {
        let source_start = row * unpadded_bytes_per_row as usize;
        let destination_start = row * padded_bytes_per_row as usize;
        upload[destination_start..destination_start + unpadded_bytes_per_row as usize]
            .copy_from_slice(&pixels[source_start..source_start + unpadded_bytes_per_row as usize]);
    }

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::ImageCopyTexture {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &upload,
        wgpu::ImageDataLayout {
            offset: 0,
            bytes_per_row: Some(padded_bytes_per_row),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("material parameters"),
        contents: bytemuck::bytes_of(&MaterialUniform { base_color }),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: uniform.as_entire_binding(),
            },
        ],
    });
    GpuMaterial {
        _texture: texture,
        _uniform: uniform,
        bind_group,
    }
}

fn building_color(definition: &str) -> [f32; 4] {
    match definition {
        "house" => [0.62, 0.43, 0.30, 1.0],
        "farm" => [0.42, 0.60, 0.32, 1.0],
        "iron_mine" => [0.42, 0.39, 0.34, 1.0],
        "workshop" => [0.60, 0.55, 0.45, 1.0],
        "wheat_storage" => [0.72, 0.60, 0.34, 1.0],
        "ore_storage" => [0.48, 0.49, 0.50, 1.0],
        _ => [0.58, 0.47, 0.35, 1.0],
    }
}
