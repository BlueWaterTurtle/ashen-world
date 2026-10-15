mod assets;
mod bootstrap;
mod camera;
mod context_menu;
mod mesh;
mod picking;
mod renderer;
mod save;
mod ui;

use std::sync::Arc;
use std::time::{Duration, Instant};

use camera::Camera;
use context_menu::ContextTarget;
use core_sim::{
    BuildingLogistics, Buildings, Footprint, Job, Jobs, PlacementRules, ResourceKind, Schedule,
    TerrainEdit, WorkerActivity, World, WorldPos, TICK_DT,
};
use glam::Vec3;
use picking::{agent_in_frustum, select_players_in_rect};
use renderer::{AgentInstance, Renderer};
use save::{SaveEnvelope, SaveMetadata, QUICK_SAVE_PATH};
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
    right_press: Option<(f64, f64)>,
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
    /// Debug: placing a selected building requests construction instead of placing it instantly.
    construct_mode: bool,
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
    save_metadata: SaveMetadata,
}

impl App {
    fn new(
        world: World,
        building_defs: Vec<content::BuildingDef>,
        schedule: Schedule,
        buildings: Buildings,
        jobs: Jobs,
        save_metadata: SaveMetadata,
    ) -> Self {
        let now = Instant::now();
        let selected_entities = world.selected_character().into_iter().collect();
        let mut camera = Camera::default();
        if let Some(point) = world.focus_points().first() {
            camera.target = Vec3::new(point.x, point.y, point.z);
        }
        Self {
            world,
            selected_entities,
            buildings,
            jobs,
            building_defs,
            schedule,
            build_mode: None,
            construct_mode: false,
            build_status: None,
            editor: TerrainEditor {
                active: false,
                tool: TerrainTool::Raise,
                radius: 6,
                strength_mm: 1000,
                slope_start: None,
            },
            camera,
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
            save_metadata,
        }
    }

    fn quick_save(&mut self) {
        let save = SaveEnvelope::capture(
            self.save_metadata.clone(),
            &self.world,
            &self.buildings,
            &self.jobs,
        );
        match save::write_save(QUICK_SAVE_PATH, &save) {
            Ok(()) => self.build_status = Some(format!("Saved to {QUICK_SAVE_PATH}.")),
            Err(error) => {
                eprintln!("{error}");
                self.build_status = Some(format!("Save failed: {error}"));
            }
        }
    }

    fn quick_load(&mut self) {
        let envelope = match save::read_save(QUICK_SAVE_PATH) {
            Ok(envelope) => envelope,
            Err(error) => {
                self.build_status = Some(format!("Load failed: {error}"));
                return;
            }
        };
        let differences = envelope.metadata.differences(&self.save_metadata);
        if !differences.is_empty() {
            self.build_status = Some(format!(
                "Load failed: save composition is incompatible: {}",
                differences.join("; ")
            ));
            return;
        }
        let world = match World::from_snapshot(envelope.world) {
            Ok(world) => world,
            Err(error) => {
                self.build_status = Some(format!("Load failed: invalid world snapshot: {error}"));
                return;
            }
        };
        if let Some(point) = world.focus_points().first() {
            self.camera.target = Vec3::new(point.x, point.y, point.z);
        }
        self.world = world;
        self.buildings = envelope.buildings;
        self.jobs = envelope.jobs;
        self.selected_entities = self.world.selected_character().into_iter().collect();
        self.accumulator = 0.0;
        self.build_status = Some(format!("Loaded from {QUICK_SAVE_PATH}."));
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
                is_smelter: definition.smelter,
                required_inputs: definition.required_inputs.clone(),
            };
            if self.construct_mode {
                self.request_construction(&definition, footprint, &rules, logistics);
                return;
            }
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

        self.move_selected_to(ground.x, ground.z);
    }

    /// Open the right-click context menu for whatever lies under the cursor, for the selected worker.
    fn handle_context_click(&mut self, screen_x: f32, screen_y: f32) {
        if self.editor.active || self.build_mode.is_some() {
            return;
        }
        let Some(worker) = self.world.selected_character() else {
            return;
        };
        let Some(window) = &self.window else {
            return;
        };
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        let (width, height) = (size.width as f32, size.height as f32);
        let Some(mut hit) = self.camera.ground_intersection(
            screen_x,
            screen_y,
            width,
            height,
            self.camera.target.y,
        ) else {
            return;
        };
        for _ in 0..3 {
            let y = self
                .world
                .terrain_edits()
                .height(self.world.seed, hit.x, hit.z);
            match self
                .camera
                .ground_intersection(screen_x, screen_y, width, height, y)
            {
                Some(refined) => hit = refined,
                None => break,
            }
        }
        let cell = (hit.x.floor() as i32, hit.z.floor() as i32);
        let target = if let Some(index) = self
            .buildings
            .buildings
            .iter()
            .position(|b| b.footprint.contains(cell))
        {
            ContextTarget::Building(index)
        } else if let Some(site) = self
            .buildings
            .sites
            .iter()
            .position(|s| s.completed.is_none() && s.footprint.contains(cell))
        {
            ContextTarget::Site(site)
        } else {
            ContextTarget::Ground { x: hit.x, z: hit.z }
        };
        let entries = context_menu::available_actions(
            target,
            worker,
            &self.jobs,
            &self.buildings,
            &self.building_defs,
        );
        let title = context_menu::title(target, &self.buildings, &self.building_defs);
        if let Some(ui) = &mut self.ui {
            ui.open_context_menu((screen_x, screen_y), title, entries);
        }
    }

    fn inspect_building(&mut self, index: usize) {
        let Some(building) = self.buildings.buildings.get(index) else {
            return;
        };
        let name = self
            .building_defs
            .iter()
            .find(|definition| definition.id == building.def)
            .map_or(building.def.as_str(), |definition| definition.name.as_str());
        let staffed = building.workers.iter().flatten().count();
        self.build_status = Some(format!(
            "{name} #{index}: {staffed}/{} workers, {:?}.",
            building.workers.len(),
            building.work_status()
        ));
    }

    fn move_selected_to(&mut self, x: f32, z: f32) {
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
                    WorldPos::new(x + offset_x, 0.0, z + offset_z),
                );
            }
        }
    }

    /// Debug construction request: reserve a site for `definition`, keep a wood store stocked
    /// for it (wood is not produced anywhere yet), and put the selected worker on the build job.
    fn request_construction(
        &mut self,
        definition: &content::BuildingDef,
        footprint: Footprint,
        rules: &PlacementRules,
        logistics: BuildingLogistics,
    ) {
        let wood = ResourceKind::Wood;
        let required = definition
            .cost
            .get(wood.id())
            .map_or(0, |amount| amount.max(0.0).round() as u32);
        let Some(store) = (0..self.buildings.buildings.len()).find(|&i| {
            let b = &self.buildings.buildings[i];
            b.is_storage && b.outputs.allows(wood.id())
        }) else {
            self.build_status = Some("Construction needs a wood store: place a Wood Store.".into());
            return;
        };
        match self.buildings.request_construction(
            self.world.seed,
            self.world.terrain_edits(),
            &definition.id,
            footprint,
            rules,
            logistics,
            wood,
            required,
        ) {
            Ok(site) => {
                self.buildings.buildings[store]
                    .outputs
                    .insert_resource(wood.id(), required);
                let mut status = format!("Construction of {} requested.", definition.name);
                if let Some(worker) = self.world.selected_character() {
                    self.jobs.add_job(worker, Job::Build { site, from: store });
                } else {
                    status.push_str(" Select a worker and add the build job.");
                }
                self.build_status = Some(status);
                self.build_mode = None;
                self.construct_mode = false;
                if let Some(ui) = &mut self.ui {
                    ui.set_selected_building(None);
                }
            }
            Err(error) => {
                self.build_status = Some(format!("Can't build here: {error:?}."));
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
                UiAction::AddBuild { worker, site, from } => {
                    self.jobs.add_job(worker, Job::Build { site, from });
                }
                UiAction::AddHaul { worker, from, to } => {
                    self.jobs.add_job(
                        worker,
                        Job::Haul {
                            from,
                            to,
                            item: ResourceKind::IronOre,
                        },
                    );
                }
                UiAction::RemoveJob { worker, index } => {
                    self.jobs
                        .remove_job(worker, index, &mut self.buildings, &mut self.world);
                }
                UiAction::MoveJobUp { worker, index } => self.jobs.move_up(worker, index),
                UiAction::MoveJobDown { worker, index } => self.jobs.move_down(worker, index),
                UiAction::ResumeJobs(worker) => self.jobs.resume(worker),
                UiAction::MoveSelected { x, z } => self.move_selected_to(x, z),
                UiAction::InspectBuilding(index) => self.inspect_building(index),
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
                            let menu_closed =
                                self.ui.as_mut().is_some_and(|ui| ui.close_context_menu());
                            if menu_closed {
                                // Escape only dismissed the context menu.
                            } else if self.editor.active {
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
                        KeyCode::F5 if down => self.quick_save(),
                        KeyCode::F9 if down => self.quick_load(),
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
                        KeyCode::KeyC if down => {
                            self.construct_mode = !self.construct_mode;
                            self.build_status = Some(if self.construct_mode {
                                "Construction mode: pick a building, click to request it.".into()
                            } else {
                                "Construction mode off.".into()
                            });
                        }
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
                    if button == MouseButton::Right {
                        if state == ElementState::Pressed {
                            self.input.right_press = self.input.last_cursor;
                        } else if let (Some(start), Some(end)) =
                            (self.input.right_press.take(), self.input.last_cursor)
                        {
                            let dx = (end.0 - start.0) as f32;
                            let dy = (end.1 - start.1) as f32;
                            if dx * dx + dy * dy < 64.0 {
                                self.handle_context_click(end.0 as f32, end.1 as f32);
                            }
                        }
                    }
                    if button != MouseButton::Right || state == ElementState::Released {
                        self.input.last_cursor = None;
                    }
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
    let startup = bootstrap::bootstrap_with_load(
        &assets::root(),
        std::env::var("ASHEN_SCENARIO").ok().as_deref(),
        std::env::var("ASHEN_LOAD").ok().as_deref(),
    );
    println!("{}", startup.summary);
    let save_metadata = SaveMetadata::from_summary(&startup.summary);
    let (config, building_defs, schedule, loaded_state) = (
        startup.config,
        startup.building_defs,
        startup.schedule,
        startup.loaded_state,
    );
    let (world, buildings, jobs) = match loaded_state {
        Some(state) => (state.world, state.buildings, state.jobs),
        None => (World::generate(&config), Buildings::default(), Jobs::new()),
    };
    let event_loop = match EventLoop::new() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("cannot create event loop (no display?): {e}");
            std::process::exit(1);
        }
    };
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::new(
        world,
        building_defs,
        schedule,
        buildings,
        jobs,
        save_metadata,
    );
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("event loop error: {e}");
    }
}

#[cfg(test)]
mod tests {
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
