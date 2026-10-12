mod camera;
mod mesh;
mod renderer;

use std::sync::Arc;
use std::time::{Duration, Instant};

use camera::Camera;
use core_sim::{height_at, Behavior, World, WorldConfig, WorldPos, TICK_DT};
use glam::{Mat4, Vec3, Vec4};
use renderer::{AgentInstance, Renderer};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

/// Cap on frame time fed to the accumulator to avoid a spiral of death.
const MAX_FRAME: Duration = Duration::from_millis(250);
/// Terrain is meshed within this many chunks of the camera target.
const VIEW_RADIUS_CHUNKS: i32 = 7;
/// Agent tints, cycled by behaviour index.
const PALETTE: [[f32; 4]; 4] = [
    [0.95, 0.76, 0.30, 1.0],
    [0.80, 0.30, 0.25, 1.0],
    [0.35, 0.65, 0.85, 1.0],
    [0.55, 0.80, 0.45, 1.0],
];
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

#[derive(Default)]
struct Input {
    left: bool,
    right: bool,
    forward: bool,
    back: bool,
    orbiting: bool,
    last_cursor: Option<(f64, f64)>,
}

struct Stats {
    window_start: Instant,
    frames: u32,
    ticks: u32,
}

struct App {
    world: World,
    camera: Camera,
    input: Input,
    accumulator: f32,
    last: Instant,
    stats: Stats,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    agent_instances: Vec<AgentInstance>,
}

impl App {
    fn new(world: World) -> Self {
        let now = Instant::now();
        Self {
            world,
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
            agent_instances: Vec::new(),
        }
    }

    fn frame(&mut self) {
        let now = Instant::now();
        let dt = (now - self.last).min(MAX_FRAME);
        self.last = now;
        self.accumulator += dt.as_secs_f32();
        while self.accumulator >= TICK_DT {
            self.world.step();
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
        self.camera.target.y = height_at(seed, self.camera.target.x, self.camera.target.z);
        self.world.set_focus_points(vec![core_sim::WorldPos::new(
            self.camera.target.x,
            0.0,
            self.camera.target.z,
        )]);

        if let Some(r) = &mut self.renderer {
            self.camera.aspect = r.aspect();
            let t = self.camera.target;
            let center = core_sim::WorldPos::new(t.x, 0.0, t.z).chunk();
            r.update_chunks(center, VIEW_RADIUS_CHUNKS);
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
                    } else if self.world.is_player_controlled(e) {
                        [0.15, 0.70, 0.30, 1.0]
                    } else {
                        PALETTE[self.world.behavior_of(e) % PALETTE.len()]
                    };
                    self.agent_instances.push(AgentInstance {
                        pos: [position.x, position.y, position.z, 0.0],
                        color: c,
                    });
                }
            }
            match r.render(&self.camera, &self.agent_instances) {
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
            Ok(r) => self.renderer = Some(r),
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
                        KeyCode::Escape => event_loop.exit(),
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
                            }
                        }
                        KeyCode::Digit2 if down => {
                            let players: Vec<_> = self.world.player_characters().collect();
                            if let Some(&p) = players.get(1) {
                                let _ = self.world.select_character(p);
                            }
                        }
                        KeyCode::Digit3 if down => {
                            let players: Vec<_> = self.world.player_characters().collect();
                            if let Some(&p) = players.get(2) {
                                let _ = self.world.select_character(p);
                            }
                        }
                        KeyCode::Space if down => {
                            if let Some(selected) = self.world.selected_character() {
                                let p = self.world.position(selected);
                                self.camera.target =
                                    Vec3::new(p.x, height_at(self.world.seed, p.x, p.z), p.z);
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
                    if let Some((cx, cy)) = self.input.last_cursor {
                        if let Some(w) = &self.window {
                            let size = w.inner_size();
                            if size.width > 0 && size.height > 0 {
                                if let Some(ground) = self.camera.ground_intersection(
                                    cx as f32,
                                    cy as f32,
                                    size.width as f32,
                                    size.height as f32,
                                    self.camera.target.y,
                                ) {
                                    let mut clicked_entity = None;
                                    for e in self.world.entities() {
                                        let p = self.world.position(e);
                                        let dist = ((p.x - ground.x).powi(2)
                                            + (p.z - ground.z).powi(2))
                                        .sqrt();
                                        if dist <= 1.5 {
                                            clicked_entity = Some(e);
                                            break;
                                        }
                                    }
                                    if let Some(e) = clicked_entity {
                                        if self.world.is_player_controlled(e) {
                                            let _ = self.world.select_character(e);
                                        } else if self.world.is_recruitable(e) {
                                            if let Some(recruiter) = self.world.selected_character()
                                            {
                                                if self.world.can_recruit_with(recruiter, e) {
                                                    let _ = self.world.recruit_with(recruiter, e);
                                                }
                                            }
                                        }
                                    } else {
                                        let _ = self.world.command_move_selected(WorldPos::new(
                                            ground.x, 0.0, ground.z,
                                        ));
                                    }
                                }
                            }
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

/// One sim behaviour per unit archetype: the archetype's behaviour definition, with
/// its speed scaled from the archetype's `move_speed` and recruitability mapped from archetype.
fn behaviors_from_content(c: &content::Content) -> Vec<Behavior> {
    let behaviors: Vec<Behavior> = c
        .units
        .iter()
        .filter_map(|u| {
            let b = c.behaviors.iter().find(|b| b.id == u.behavior)?;
            Some(
                Behavior::from_seconds(
                    b.wander_range,
                    b.pause_seconds_min,
                    b.pause_seconds_max,
                    u.move_speed * b.speed_multiplier,
                )
                .with_recruitable(u.recruitable),
            )
        })
        .collect();
    if behaviors.is_empty() {
        vec![Behavior::default()]
    } else {
        behaviors
    }
}

fn main() {
    let content_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
    let config = match content::load_content(&content_dir) {
        Ok(c) => {
            println!(
                "content: {} factions, {} units, {} biomes, {} races, {} behaviors, {} buildings, {} crops",
                c.factions.len(),
                c.units.len(),
                c.biomes.len(),
                c.races.len(),
                c.behaviors.len(),
                c.buildings.len(),
                c.crops.len()
            );
            WorldConfig {
                behaviors: behaviors_from_content(&c),
                ..WorldConfig::default()
            }
        }
        Err(e) => {
            eprintln!("content load failed: {e}");
            WorldConfig::default()
        }
    };
    let world = World::generate(&config);
    let event_loop = match EventLoop::new() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("cannot create event loop (no display?): {e}");
            std::process::exit(1);
        }
    };
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::new(world);
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("event loop error: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
