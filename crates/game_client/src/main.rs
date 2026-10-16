mod actions;
mod assets;
mod bootstrap;
mod camera;
mod context_menu;
mod input;
mod key_bindings;
mod mesh;
mod panel;
mod picking;
mod renderer;
mod save;
mod side_panel;
mod terrain_editor;
mod ui;

use std::sync::Arc;
use std::time::{Duration, Instant};

use camera::Camera;
use core_sim::{Buildings, Jobs, Schedule, WorkerActivity, World, TICK_DT};
use glam::Vec3;
use input::Input;
use picking::agent_in_frustum;
use renderer::{AgentInstance, Renderer};
use save::{SaveEnvelope, SaveMetadata, QUICK_SAVE_PATH};
use terrain_editor::TerrainEditor;
use ui::{GameUi, HudData};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

/// Cap on frame time fed to the accumulator to avoid a spiral of death.
const MAX_FRAME: Duration = Duration::from_millis(250);
/// Terrain is meshed within this many chunks of the camera target.
const VIEW_RADIUS_CHUNKS: i32 = 7;

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
    save_metadata: SaveMetadata,
    /// Set by `Action::Escape` when nothing is left to cancel; the event loop then exits.
    quit_requested: bool,
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
            build_status: None,
            editor: TerrainEditor::default(),
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
            quit_requested: false,
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
        let (world, buildings, jobs) = match envelope.restore(&self.save_metadata) {
            Ok(state) => state,
            Err(error) => {
                self.build_status = Some(format!("Load failed: {error}"));
                return;
            }
        };
        if let Some(point) = world.focus_points().first() {
            self.camera.target = Vec3::new(point.x, point.y, point.z);
        }
        self.world = world;
        self.buildings = buildings;
        self.jobs = jobs;
        self.selected_entities = self.world.selected_character().into_iter().collect();
        self.accumulator = 0.0;
        self.build_status = Some(format!("Loaded from {QUICK_SAVE_PATH}."));
    }

    /// Queue the active terrain tool at lattice point `at`, unless it would leave an
    /// existing building on a slope too steep to stand on.
    fn apply_terrain_tool(&mut self, at: (i32, i32)) {
        self.build_status = Some(self.editor.apply(at, &mut self.world, &self.buildings));
    }

    fn frame(&mut self) {
        if let Some(renderer) = &self.renderer {
            self.camera.aspect = renderer.aspect();
        }
        self.run_hud();

        let now = Instant::now();
        let dt = (now - self.last).min(MAX_FRAME);
        self.last = now;
        self.step_simulation(dt);
        self.update_camera(dt);
        self.render_scene();
        self.report_stats(now);
    }

    /// Run the HUD and apply any actions the player triggered through it.
    fn run_hud(&mut self) {
        let placement_preview = self.placement_preview();
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
                    terrain_editor: &self.editor,
                    placement_preview: placement_preview.as_ref(),
                },
            ),
            _ => Vec::new(),
        };
        for action in actions {
            self.dispatch_action(action);
        }
    }

    /// Advance the fixed-step simulation by as many ticks as `dt` allows.
    fn step_simulation(&mut self, dt: Duration) {
        self.accumulator += dt.as_secs_f32();
        while self.accumulator >= TICK_DT {
            self.schedule.run(&mut self.world);
            self.jobs.update(&mut self.world, &mut self.buildings);
            self.buildings.tick_production(TICK_DT);
            self.accumulator -= TICK_DT;
            self.stats.ticks += 1;
        }
    }

    /// Apply keyboard panning, keep the camera on the terrain, and publish it as the LOD focus.
    fn update_camera(&mut self, dt: Duration) {
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
    }

    fn render_scene(&mut self) {
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
    }

    /// Count the frame and, once a second, print and show frame/sim statistics.
    fn report_stats(&mut self, now: Instant) {
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
        if self.consume_ui_event(&event) {
            return;
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(s) => {
                if let Some(r) = &mut self.renderer {
                    r.resize(s.width, s.height);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => self.handle_key(event_loop, &event),
            WindowEvent::MouseInput { state, button, .. } => {
                self.handle_mouse_button(state, button);
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.handle_cursor_moved(position.x, position.y);
            }
            WindowEvent::MouseWheel { delta, .. } => self.handle_mouse_wheel(delta),
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
