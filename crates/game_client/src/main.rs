mod camera;
mod mesh;
mod renderer;

use std::sync::Arc;
use std::time::{Duration, Instant};

use camera::Camera;
use core_sim::{height_at, Behavior, Entity, World, WorldConfig, TICK_DT};
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

        if let Some(r) = &mut self.renderer {
            self.camera.aspect = r.aspect();
            let t = self.camera.target;
            let center = core_sim::WorldPos::new(t.x, 0.0, t.z).chunk();
            r.update_chunks(center, VIEW_RADIUS_CHUNKS);
            let agents: Vec<AgentInstance> = self
                .world
                .entities()
                .map(|e: Entity| {
                    let p = self.world.position(e);
                    let c = PALETTE[self.world.behavior_of(e) % PALETTE.len()];
                    AgentInstance {
                        pos: [p.x, height_at(seed, p.x, p.z), p.z, 0.0],
                        color: c,
                    }
                })
                .collect();
            match r.render(&self.camera, &agents) {
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
                "frame {:.2} ms | sim {:.1} ticks/s | entities {} | chunks {}",
                1000.0 * secs / self.stats.frames as f32,
                self.stats.ticks as f32 / secs,
                self.world.entity_count(),
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
                        _ => {}
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if button == MouseButton::Right || button == MouseButton::Middle {
                    self.input.orbiting = state == ElementState::Pressed;
                    self.input.last_cursor = None;
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
/// its speed scaled from the archetype's `move_speed`.
fn behaviors_from_content(c: &content::Content) -> Vec<Behavior> {
    let behaviors: Vec<Behavior> = c
        .units
        .iter()
        .filter_map(|u| {
            let b = c.behaviors.iter().find(|b| b.id == u.behavior)?;
            Some(Behavior::from_seconds(
                b.wander_range,
                b.pause_seconds_min,
                b.pause_seconds_max,
                u.move_speed * b.speed_multiplier,
            ))
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
