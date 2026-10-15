mod assets;
mod bootstrap;
mod camera;
mod mesh;
mod renderer;
mod ui;

use std::sync::Arc;
use std::time::{Duration, Instant};

use camera::Camera;
use core_sim::{
    BuildingLogistics, Buildings, Footprint, Job, Jobs, PlacementRules, Schedule, TerrainEdit,
    WorkerActivity, World, WorldPos, TICK_DT,
};
use glam::{Mat4, Vec3, Vec4};
use renderer::{AgentInstance, Renderer};
use ui::{GameUi, HudData, UiAction};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

/// Cap on frame time fed to the accumulator to avoid a spiral of death.
const MAX_FRAME: Duration = Duration::from_millis(250);
/// Terrain is meshed within this many chunks of the camera target.
const VIEW_RADIUS_CHUNKS: i32 = 7;
const AGENT_CULL_RADIUS: f32 = 1.0;

fn agent_in_frustum(position: WorldPos, view_proj: Mat4) -> bool {
    let center = Vec3::new(position.x, position.y + 0.9, position.z);
    let x = Vec4::new(
        view_proj.x_axis.x,
        view_proj.y_axis.x,
        view_proj.z_axis.x,
        view_proj.w_axis.x,
    );
    let y = Vec4::new(
        view_proj.x_axis.y,
        view_proj.y_axis.y,
        view_proj.z_axis.y,
        view_proj.w_axis.y,
    );
    let z = Vec4::new(
        view_proj.x_axis.z,
        view_proj.y_axis.z,
        view_proj.z_axis.z,
        view_proj.w_axis.z,
    );
    let w = Vec4::new(
        view_proj.x_axis.w,
        view_proj.y_axis.w,
        view_proj.z_axis.w,
        view_proj.w_axis.w,
    );
    [w + x, w - x, w + y, w - y, z, w - z].iter().all(|plane| {
        let normal = plane.truncate();
        normal.dot(center) + plane.w >= -AGENT_CULL_RADIUS * normal.length()
    })
}

fn select_players_in_rect(
    world: &World,
    view_proj: Mat4,
    width: f32,
    height: f32,
    start: (f32, f32),
    end: (f32, f32),
) -> Vec<core_sim::Entity> {
    let left = start.0.min(end.0);
    let right = start.0.max(end.0);
    let top = start.1.min(end.1);
    let bottom = start.1.max(end.1);
    world
        .player_characters()
        .filter(|&entity| {
            let position = world.position(entity);
            let clip = view_proj * Vec3::new(position.x, position.y + 0.9, position.z).extend(1.0);
            if clip.w <= 0.0 {
                return false;
            }
            let ndc = clip.truncate() / clip.w;
            if !(0.0..=1.0).contains(&ndc.z) {
                return false;
            }
            let screen_x = (ndc.x + 1.0) * 0.5 * width;
            let screen_y = (1.0 - ndc.y) * 0.5 * height;
            (left..=right).contains(&screen_x) && (top..=bottom).contains(&screen_y)
        })
        .collect()
}

#[derive(Default)]
struct Input {
    left: bool,
    right: bool,
    forward: bool,
    back: bool,
    orbiting: bool,
    ui_pointer_captured: bool,
    last_cursor: Option<(f64, f64)>,
    left_press: Option<(f64, f64)>,
    selection_rect: Option<[f32; 4]>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TerrainTool {
    Raise,
    Lower,
    Flatten,
    Smooth,
    Slope,
}

/// Terrain editor mode: while active, left clicks reshape the terrain instead of selecting.
struct TerrainEditor {
    active: bool,
    tool: TerrainTool,
    radius: i32,
    strength_mm: i32,
    slope_start: Option<(i32, i32)>,
}

impl TerrainEditor {
    fn status(&self) -> String {
        let tool = match self.tool {
            TerrainTool::Raise => "Raise",
            TerrainTool::Lower => "Lower",
            TerrainTool::Flatten => "Flatten",
            TerrainTool::Smooth => "Smooth",
            TerrainTool::Slope => "Slope (click start, then end)",
        };
        format!(
            "Terrain editor: {tool}, radius {} m, strength {:.1} m. F1-F5 tool, [ ] radius, - = strength, T exits.",
            self.radius,
            self.strength_mm as f32 / 1000.0
        )
    }
}

struct Stats {
    window_start: Instant,
    frames: u32,
    ticks: u32,
}

struct App {
    world: World,
    selected_entities: Vec<core_sim::Entity>,
    buildings: Buildings,
    jobs: Jobs,
    building_defs: Vec<content::BuildingDef>,
    schedule: Schedule,
    build_mode: Option<String>,
    build_status: Option<String>,
    editor: TerrainEditor,
    camera: Camera,
    input: Input,
    accumulator: f32,
    last: Instant,
    stats: Stats,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    ui: Option<GameUi>,
    agent_instances: Vec<AgentInstance>,
}

impl App {
    fn new(world: World, building_defs: Vec<content::BuildingDef>, schedule: Schedule) -> Self {
        let now = Instant::now();
        let selected_entities = world.selected_character().into_iter().collect();
        Self {
            world,
            selected_entities,
            buildings: Buildings::default(),
            jobs: Jobs::new(),
            building_defs,
            schedule,
            build_mode: None,
            build_status: None,
            editor: TerrainEditor {
                active: false,
                tool: TerrainTool::Raise,
                radius: 6,
                strength_mm: 1000,
                slope_start: None,
            },
            camera: Camera::default(),
            input: Input::default(),
            accumulator: 0.0,
            last: now,
            stats: Stats {
                window_start: now,
                frames: 0,
                ticks: 0,
            },
            window: None,
            renderer: None,
            ui: None,
            agent_instances: Vec::new(),
        }
    }

    /// Queue the active terrain tool at lattice point `at`, unless it would leave an
    /// existing building on a slope too steep to stand on.
    fn apply_terrain_tool(&mut self, at: (i32, i32)) {
        let editor = &mut self.editor;
        let edit = match editor.tool {
            TerrainTool::Raise => TerrainEdit::Raise {
                center: at,
                radius: editor.radius,
                amount_mm: editor.strength_mm,
            },
            TerrainTool::Lower => TerrainEdit::Lower {
                center: at,
                radius: editor.radius,
                amount_mm: editor.strength_mm,
            },
            TerrainTool::Flatten => {
                let target =
                    self.world
                        .terrain_edits()
                        .height(self.world.seed, at.0 as f32, at.1 as f32);
                TerrainEdit::Flatten {
                    center: at,
                    radius: editor.radius,
                    target_mm: (target * 1000.0).round() as i32,
                }
            }
            TerrainTool::Smooth => TerrainEdit::Smooth {
                center: at,
                radius: editor.radius,
            },
            TerrainTool::Slope => match editor.slope_start.take() {
                None => {
                    editor.slope_start = Some(at);
                    self.build_status = Some("Slope start set; click the end point.".into());
                    return;
                }
                Some(from) => TerrainEdit::Slope {
                    from,
                    to: at,
                    half_width: editor.radius,
                },
            },
        };
        if self
            .buildings
            .terrain_edit_is_allowed(self.world.seed, self.world.terrain_edits(), edit)
        {
            self.world.queue_terrain_edit(edit);
            self.build_status = Some(self.editor.status());
        } else {
            self.build_status = Some("Edit would leave a building on too steep a slope.".into());
        }
    }

    fn handle_world_click(&mut self, screen_x: f32, screen_y: f32) {
        let Some(window) = &self.window else {
            return;
        };
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        let Some(ground) = self.camera.ground_intersection(
            screen_x,
            screen_y,
            size.width as f32,
            size.height as f32,
            self.camera.target.y,
        ) else {
            return;
        };

        if self.editor.active {
            let mut hit = ground;
            for _ in 0..3 {
                let y = self
                    .world
                    .terrain_edits()
                    .height(self.world.seed, hit.x, hit.z);
                let (width, height) = (size.width as f32, size.height as f32);
                match self
                    .camera
                    .ground_intersection(screen_x, screen_y, width, height, y)
                {
                    Some(refined) => hit = refined,
                    None => break,
                }
            }
            self.apply_terrain_tool((hit.x.round() as i32, hit.z.round() as i32));
            return;
        }

        if let Some(definition_id) = self.build_mode.clone() {
            let Some(definition) = self
                .building_defs
                .iter()
                .find(|definition| definition.id == definition_id)
                .cloned()
            else {
                return;
            };
            let footprint = Footprint {
                x: ground.x.floor() as i32 - definition.width / 2,
                z: ground.z.floor() as i32 - definition.depth / 2,
                width: definition.width,
                depth: definition.depth,
            };
            let rules = PlacementRules {
                requires_resource: definition.requires_resource.clone(),
                minimum_resource_richness: definition.min_resource_richness,
                minimum_fertility: definition.min_fertility,
                building_margin: 0,
            };
            let logistics = BuildingLogistics {
                worker_slots: definition.worker_slots,
                input_slots: definition.input_slots,
                output_slots: definition.output_slots,
                stack_limit: definition.stack_limit,
                input_resources: definition.input_resources.clone(),
                output_resources: definition.output_resources.clone(),
                is_storage: definition.storage,
                is_mine: definition.mine,
                required_inputs: definition.required_inputs.clone(),
            };
            match self.buildings.place_with_logistics(
                self.world.seed,
                self.world.terrain_edits(),
                &definition.id,
                footprint,
                &rules,
                logistics,
            ) {
                Ok(_) => {
                    self.build_status = Some(format!("Placed {}.", definition.name));
                    self.build_mode = None;
                    if let Some(ui) = &mut self.ui {
                        ui.set_selected_building(None);
                    }
                }
                Err(error) => {
                    self.build_status = Some(format!("Can't place here: {error:?}."));
                }
            }
            return;
        }

        let clicked_entity = self.world.entities().find(|&entity| {
            let position = self.world.position(entity);
            ((position.x - ground.x).powi(2) + (position.z - ground.z).powi(2)).sqrt() <= 1.5
        });
        if let Some(entity) = clicked_entity {
            if self.world.is_player_controlled(entity) {
                let _ = self.world.select_character(entity);
                self.selected_entities = vec![entity];
            } else if self.world.is_recruitable(entity) {
                if let Some(recruiter) = self.world.selected_character() {
                    if self.world.can_recruit_with(recruiter, entity)
                        && self.world.recruit_with(recruiter, entity).is_ok()
                    {
                        self.selected_entities.push(entity);
                    }
                }
            }
            return;
        }

        let selection = if self.selected_entities.is_empty() {
            self.world
                .selected_character()
                .into_iter()
                .collect::<Vec<_>>()
        } else {
            self.selected_entities.clone()
        };
        if let Some(&anchor) = selection.first() {
            let anchor_position = self.world.position(anchor);
            for entity in selection {
                let position = self.world.position(entity);
                let offset_x = position.x - anchor_position.x;
                let offset_z = position.z - anchor_position.z;
                let _ = self.jobs.player_move(
                    &mut self.world,
                    &mut self.buildings,
                    entity,
                    WorldPos::new(ground.x + offset_x, 0.0, ground.z + offset_z),
                );
            }
        }
    }

    fn frame(&mut self) {
        if let Some(renderer) = &self.renderer {
            self.camera.aspect = renderer.aspect();
        }
        let actions = match (&self.window, &mut self.ui) {
            (Some(window), Some(ui)) => ui.run(
                window,
                HudData {
                    world: &self.world,
                    selected_entities: &self.selected_entities,
                    buildings: &self.buildings,
                    jobs: &self.jobs,
                    building_defs: &self.building_defs,
                    build_status: self.build_status.as_deref(),
                    seed: self.world.seed,
                    camera: &self.camera,
                    selection_rect: self.input.selection_rect,
                },
            ),
            _ => Vec::new(),
        };
        for action in actions {
            match action {
                UiAction::Select(entity) => {
                    let _ = self.world.select_character(entity);
                    self.selected_entities.clear();
                    self.selected_entities.push(entity);
                }
                UiAction::Recruit(entity) => {
                    if let Some(recruiter) = self.world.selected_character() {
                        if self.world.recruit_with(recruiter, entity).is_ok() {
                            self.selected_entities.push(entity);
                        }
                    }
                }
                UiAction::SelectBuilding(id) => {
                    self.editor.active = false;
                    self.build_mode = Some(id.clone());
                    self.build_status = None;
                    if let Some(ui) = &mut self.ui {
                        ui.set_selected_building(Some(id));
                    }
                }
                UiAction::AddJob { worker, building } => {
                    self.jobs.add_job(worker, Job::Work { building });
                }
                UiAction::RemoveJob { worker, index } => {
                    self.jobs
                        .remove_job(worker, index, &mut self.buildings, &mut self.world);
                }
                UiAction::MoveJobUp { worker, index } => self.jobs.move_up(worker, index),
                UiAction::MoveJobDown { worker, index } => self.jobs.move_down(worker, index),
                UiAction::ResumeJobs(worker) => self.jobs.resume(worker),
                UiAction::CancelBuilding => {
                    self.build_mode = None;
                    self.build_status = None;
                    if let Some(ui) = &mut self.ui {
                        ui.set_selected_building(None);
                    }
                }
            }
        }

        let now = Instant::now();
        let dt = (now - self.last).min(MAX_FRAME);
        self.last = now;
        self.accumulator += dt.as_secs_f32();
        while self.accumulator >= TICK_DT {
            self.schedule.run(&mut self.world);
            self.jobs.update(&mut self.world, &mut self.buildings);
            self.buildings.tick_production(TICK_DT);
            self.accumulator -= TICK_DT;
            self.stats.ticks += 1;
        }

        let axis = |pos: bool, neg: bool| f32::from(pos) - f32::from(neg);
        let speed = self.camera.distance * 0.9 * dt.as_secs_f32();
        self.camera.pan(
            axis(self.input.right, self.input.left) * speed,
            axis(self.input.forward, self.input.back) * speed,
        );
        let seed = self.world.seed;
        self.camera.target.y =
            self.world
                .terrain_edits()
                .height(seed, self.camera.target.x, self.camera.target.z);
        self.world.set_focus_points(vec![core_sim::WorldPos::new(
            self.camera.target.x,
            0.0,
            self.camera.target.z,
        )]);

        if let (Some(r), Some(ui)) = (&mut self.renderer, &mut self.ui) {
            self.camera.aspect = r.aspect();
            let t = self.camera.target;
            let center = core_sim::WorldPos::new(t.x, 0.0, t.z).chunk();
            r.update_chunks(center, VIEW_RADIUS_CHUNKS, self.world.terrain_edits());
            let view_proj = self.camera.view_proj();
            self.agent_instances.clear();
            for e in self.world.entities() {
                if !self.world.is_renderable(e) {
                    continue;
                }
                let position = self.world.position(e);
                if agent_in_frustum(position, view_proj) {
                    let c = if self.world.selected_character() == Some(e) {
                        [0.2, 0.95, 0.35, 1.0]
                    } else if self.selected_entities.contains(&e) {
                        [0.9, 0.82, 0.35, 1.0]
                    } else {
                        [1.0; 4]
                    };
                    // Placeholder work animation: a small vertical bob while working.
                    let bob = if self.jobs.activity(e) == WorkerActivity::Working {
                        0.15 * (self.world.tick as f32 * 0.3).sin().abs()
                    } else {
                        0.0
                    };
                    self.agent_instances.push(AgentInstance {
                        pos: [position.x, position.y + bob, position.z, 0.0],
                        color: c,
                    });
                }
            }
            let pixels_per_point = self
                .window
                .as_ref()
                .map_or(1.0, |window| window.scale_factor() as f32);
            match r.render(
                &self.camera,
                &self.agent_instances,
                &self.buildings.buildings,
                self.world.terrain_edits(),
                ui,
                pixels_per_point,
            ) {
                Ok(()) => {}
                Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                    if let Some(w) = &self.window {
                        let s = w.inner_size();
                        r.resize(s.width, s.height);
                    }
                }
                Err(wgpu::SurfaceError::OutOfMemory) => std::process::exit(1),
                Err(e) => eprintln!("render error: {e}"),
            }
        }

        self.stats.frames += 1;
        let elapsed = now - self.stats.window_start;
        if elapsed >= Duration::from_secs(1) {
            let secs = elapsed.as_secs_f32();
            let line = format!(
                "frame {:.2} ms | sim {:.1} ticks/s | entities {} | player characters {} | chunks {}",
                1000.0 * secs / self.stats.frames as f32,
                self.stats.ticks as f32 / secs,
                self.world.entity_count(),
                self.world.player_character_count(),
                self.renderer.as_ref().map_or(0, Renderer::loaded_chunks),
            );
            println!("{line}");
            if let Some(w) = &self.window {
                w.set_title(&format!("Ashen World - {line}"));
            }
            self.stats.window_start = now;
            self.stats.frames = 0;
            self.stats.ticks = 0;
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes().with_title("Ashen World");
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                eprintln!("failed to create window: {e}");
                event_loop.exit();
                return;
            }
        };
        match Renderer::new(window.clone(), self.world.seed) {
            Ok(r) => {
                self.ui = Some(r.create_ui(&window));
                self.renderer = Some(r);
            }
            Err(e) => {
                eprintln!("failed to initialise GPU renderer: {e}");
                event_loop.exit();
                return;
            }
        }
        window.request_redraw();
        self.window = Some(window);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let ui_consumed = match (&self.window, &mut self.ui) {
            (Some(window), Some(ui)) => ui.on_window_event(window, &event),
            _ => false,
        };
        let is_ui_input = matches!(
            event,
            WindowEvent::KeyboardInput { .. }
                | WindowEvent::MouseInput { .. }
                | WindowEvent::CursorMoved { .. }
                | WindowEvent::MouseWheel { .. }
        );
        if self.input.ui_pointer_captured
            && matches!(
                event,
                WindowEvent::MouseInput {
                    state: ElementState::Released,
                    button: MouseButton::Left,
                    ..
                }
            )
        {
            self.input.ui_pointer_captured = false;
        }
        if ui_consumed || self.input.ui_pointer_captured {
            if matches!(
                event,
                WindowEvent::MouseInput {
                    state: ElementState::Released,
                    button: MouseButton::Left,
                    ..
                }
            ) {
                self.input.left_press = None;
                self.input.selection_rect = None;
            }
            if matches!(
                event,
                WindowEvent::MouseInput {
                    state: ElementState::Pressed,
                    button: MouseButton::Left,
                    ..
                }
            ) {
                self.input.ui_pointer_captured = true;
            }
            if matches!(
                event,
                WindowEvent::MouseInput {
                    state: ElementState::Released,
                    button: MouseButton::Right | MouseButton::Middle,
                    ..
                }
            ) {
                self.input.orbiting = false;
            }
            if is_ui_input {
                return;
            }
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(s) => {
                if let Some(r) = &mut self.renderer {
                    r.resize(s.width, s.height);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let down = event.state == ElementState::Pressed;
                if let PhysicalKey::Code(code) = event.physical_key {
                    match code {
                        KeyCode::Escape if down => {
                            if self.editor.active {
                                self.editor.active = false;
                                self.editor.slope_start = None;
                                self.build_status = None;
                            } else if self.build_mode.take().is_some() {
                                if let Some(ui) = &mut self.ui {
                                    ui.set_selected_building(None);
                                }
                            } else {
                                event_loop.exit();
                            }
                        }
                        KeyCode::KeyT if down => {
                            self.editor.active = !self.editor.active;
                            self.editor.slope_start = None;
                            if self.editor.active {
                                self.build_mode = None;
                                if let Some(ui) = &mut self.ui {
                                    ui.set_selected_building(None);
                                }
                                self.build_status = Some(self.editor.status());
                            } else {
                                self.build_status = None;
                            }
                        }
                        KeyCode::F1 | KeyCode::F2 | KeyCode::F3 | KeyCode::F4 | KeyCode::F5
                            if down && self.editor.active =>
                        {
                            self.editor.tool = match code {
                                KeyCode::F1 => TerrainTool::Raise,
                                KeyCode::F2 => TerrainTool::Lower,
                                KeyCode::F3 => TerrainTool::Flatten,
                                KeyCode::F4 => TerrainTool::Smooth,
                                _ => TerrainTool::Slope,
                            };
                            self.editor.slope_start = None;
                            self.build_status = Some(self.editor.status());
                        }
                        KeyCode::BracketLeft | KeyCode::BracketRight
                            if down && self.editor.active =>
                        {
                            let step = if code == KeyCode::BracketLeft { -1 } else { 1 };
                            self.editor.radius = (self.editor.radius + step)
                                .clamp(1, core_sim::terrain::MAX_BRUSH_RADIUS);
                            self.build_status = Some(self.editor.status());
                        }
                        KeyCode::Minus | KeyCode::Equal if down && self.editor.active => {
                            let step = if code == KeyCode::Minus { -250 } else { 250 };
                            self.editor.strength_mm =
                                (self.editor.strength_mm + step).clamp(250, 10_000);
                            self.build_status = Some(self.editor.status());
                        }
                        KeyCode::KeyA | KeyCode::ArrowLeft => self.input.left = down,
                        KeyCode::KeyD | KeyCode::ArrowRight => self.input.right = down,
                        KeyCode::KeyW | KeyCode::ArrowUp => self.input.forward = down,
                        KeyCode::KeyS | KeyCode::ArrowDown => self.input.back = down,
                        KeyCode::KeyQ if down => self.camera.orbit(-0.2, 0.0),
                        KeyCode::KeyE if down => self.camera.orbit(0.2, 0.0),
                        KeyCode::Digit1 if down => {
                            let players: Vec<_> = self.world.player_characters().collect();
                            if let Some(&p) = players.first() {
                                let _ = self.world.select_character(p);
                                self.selected_entities = vec![p];
                            }
                        }
                        KeyCode::Digit2 if down => {
                            let players: Vec<_> = self.world.player_characters().collect();
                            if let Some(&p) = players.get(1) {
                                let _ = self.world.select_character(p);
                                self.selected_entities = vec![p];
                            }
                        }
                        KeyCode::Digit3 if down => {
                            let players: Vec<_> = self.world.player_characters().collect();
                            if let Some(&p) = players.get(2) {
                                let _ = self.world.select_character(p);
                                self.selected_entities = vec![p];
                            }
                        }
                        KeyCode::Space if down => {
                            if let Some(selected) = self.world.selected_character() {
                                let p = self.world.position(selected);
                                self.camera.target = Vec3::new(
                                    p.x,
                                    self.world.terrain_edits().height(self.world.seed, p.x, p.z),
                                    p.z,
                                );
                            }
                        }
                        KeyCode::KeyR if down => {
                            if let Some(recruiter) = self.world.selected_character() {
                                let rp = self.world.position(recruiter);
                                let mut nearest: Option<(core_sim::Entity, f32)> = None;
                                for e in self.world.entities() {
                                    if self.world.is_recruitable(e) {
                                        let ep = self.world.position(e);
                                        let dist_sq = (rp.x - ep.x).powi(2) + (rp.z - ep.z).powi(2);
                                        if dist_sq <= core_sim::RECRUIT_MAX_DISTANCE.powi(2)
                                            && nearest.is_none_or(|(_, d)| dist_sq < d)
                                        {
                                            nearest = Some((e, dist_sq));
                                        }
                                    }
                                }
                                if let Some((target, _)) = nearest {
                                    let _ = self.world.recruit_with(recruiter, target);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if button == MouseButton::Right || button == MouseButton::Middle {
                    self.input.orbiting = state == ElementState::Pressed;
                    self.input.last_cursor = None;
                } else if button == MouseButton::Left && state == ElementState::Pressed {
                    self.input.left_press = self.input.last_cursor;
                    self.input.selection_rect = None;
                } else if button == MouseButton::Left && state == ElementState::Released {
                    let start = self.input.left_press.take();
                    let end = self.input.last_cursor;
                    self.input.selection_rect = None;
                    if let (Some(start), Some(end)) = (start, end) {
                        let dx = (end.0 - start.0) as f32;
                        let dy = (end.1 - start.1) as f32;
                        if dx * dx + dy * dy >= 64.0 {
                            if self.build_mode.is_none() && !self.editor.active {
                                if let Some(window) = &self.window {
                                    let size = window.inner_size();
                                    let view_proj = self.camera.view_proj();
                                    let selected = select_players_in_rect(
                                        &self.world,
                                        view_proj,
                                        size.width as f32,
                                        size.height as f32,
                                        (start.0 as f32, start.1 as f32),
                                        (end.0 as f32, end.1 as f32),
                                    );
                                    if let Some(&primary) = selected.first() {
                                        let _ = self.world.select_character(primary);
                                        self.selected_entities = selected;
                                    }
                                }
                            }
                        } else {
                            self.handle_world_click(end.0 as f32, end.1 as f32);
                        }
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if self.input.orbiting {
                    if let Some((lx, ly)) = self.input.last_cursor {
                        let (dx, dy) = (position.x - lx, position.y - ly);
                        self.camera.orbit(-dx as f32 * 0.005, dy as f32 * 0.005);
                    }
                }
                self.input.last_cursor = Some((position.x, position.y));
                if self.build_mode.is_none() {
                    if let Some(start) = self.input.left_press {
                        let dx = position.x - start.0;
                        let dy = position.y - start.1;
                        if dx * dx + dy * dy >= 64.0 {
                            self.input.selection_rect = Some([
                                start.0 as f32,
                                start.1 as f32,
                                position.x as f32,
                                position.y as f32,
                            ]);
                        }
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let steps = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / 40.0,
                };
                self.camera.zoom(steps);
            }
            WindowEvent::RedrawRequested => {
                self.frame();
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            _ => {}
        }
    }
}

fn main() {
    let startup = bootstrap::bootstrap(
        &assets::root(),
        std::env::var("ASHEN_SCENARIO").ok().as_deref(),
    );
    println!("{}", startup.summary);
    let (config, building_defs, schedule) =
        (startup.config, startup.building_defs, startup.schedule);
    let world = World::generate(&config);
    let event_loop = match EventLoop::new() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("cannot create event loop (no display?): {e}");
            std::process::exit(1);
        }
    };
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::new(world, building_defs, schedule);
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("event loop error: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_sim::WorldConfig;

    #[test]
    fn agent_frustum_culling_keeps_visible_and_discards_distant_agents() {
        let camera = Camera::default();
        let view_proj = camera.view_proj();
        let target = camera.target;
        assert!(agent_in_frustum(
            WorldPos::new(target.x, target.y, target.z),
            view_proj
        ));
        assert!(!agent_in_frustum(
            WorldPos::new(target.x + 100_000.0, target.y, target.z),
            view_proj
        ));
    }

    #[test]
    fn drag_selection_includes_only_player_characters_inside_screen_rect() {
        use core_sim::Entity;

        let camera = Camera::default();
        let target = camera.target;
        let mut world = World::generate(&WorldConfig {
            seed: 123,
            agent_count: 4,
            starting_player_characters: 3,
            ..Default::default()
        });
        world.set_position(Entity(0), WorldPos::new(target.x - 1.0, target.y, target.z));
        world.set_position(Entity(1), WorldPos::new(target.x + 1.0, target.y, target.z));
        world.set_position(
            Entity(2),
            WorldPos::new(target.x + 100.0, target.y, target.z),
        );

        let project = |entity| {
            let position = world.position(entity);
            let clip = camera.view_proj()
                * Vec3::new(position.x, position.y + 0.9, position.z).extend(1.0);
            let ndc = clip.truncate() / clip.w;
            ((ndc.x + 1.0) * 400.0, (1.0 - ndc.y) * 300.0)
        };
        let first = project(Entity(0));
        let second = project(Entity(1));
        let start = (first.0.min(second.0) - 10.0, first.1.min(second.1) - 10.0);
        let end = (first.0.max(second.0) + 10.0, first.1.max(second.1) + 10.0);
        let selected = select_players_in_rect(&world, camera.view_proj(), 800.0, 600.0, start, end);

        assert_eq!(selected, vec![Entity(0), Entity(1)]);
    }

    #[test]
    fn rendering_includes_reduced_and_excludes_dormant_and_culled_agents() {
        use core_sim::{Entity, SimulationTier};

        let camera = Camera::default();
        let view_proj = camera.view_proj();
        let target = camera.target;

        let mut world = World::generate(&WorldConfig {
            seed: 123,
            agent_count: 4,
            ..Default::default()
        });

        let e_active = Entity(0);
        let e_reduced = Entity(1);
        let e_dormant = Entity(2);
        let e_culled = Entity(3);

        world.set_tier(e_active, SimulationTier::Active);
        world.set_position(e_active, WorldPos::new(target.x, target.y, target.z));

        // Reduced tier: simulated less often, but STILL rendered when visible
        world.set_tier(e_reduced, SimulationTier::Reduced);
        world.set_position(e_reduced, WorldPos::new(target.x + 1.0, target.y, target.z));

        // Dormant tier: not rendered until promoted back to higher tier
        world.set_tier(e_dormant, SimulationTier::Dormant);
        world.set_position(e_dormant, WorldPos::new(target.x + 2.0, target.y, target.z));

        // Active tier, but outside camera frustum
        world.set_tier(e_culled, SimulationTier::Active);
        world.set_position(
            e_culled,
            WorldPos::new(target.x + 50_000.0, target.y, target.z),
        );

        let mut rendered_entities = Vec::new();
        for e in world.entities() {
            if !world.is_renderable(e) {
                continue;
            }
            let pos = world.position(e);
            if agent_in_frustum(pos, view_proj) {
                rendered_entities.push(e);
            }
        }

        assert!(rendered_entities.contains(&e_active));
        assert!(rendered_entities.contains(&e_reduced));
        assert!(!rendered_entities.contains(&e_dormant));
        assert!(!rendered_entities.contains(&e_culled));
    }

    #[test]
    fn textured_render_shader_validates() {
        let module = naga::front::wgsl::parse_str(include_str!("shader.wgsl")).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
    }
}
