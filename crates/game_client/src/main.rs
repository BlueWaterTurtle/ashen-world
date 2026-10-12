mod camera;

use camera::Camera;
use core_sim::{ChunkStreamer, World, WorldConfig, WorldPos, CHUNK_SIZE, TICK_DT};
use std::num::NonZeroU32;
use std::rc::Rc;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

/// Cap on frame time fed to the accumulator to avoid a spiral of death.
const MAX_FRAME: Duration = Duration::from_millis(250);

#[derive(Default)]
struct Input {
    left: bool,
    right: bool,
    forward: bool,
    back: bool,
    up: bool,
    down: bool,
}

struct Stats {
    window_start: Instant,
    frames: u32,
    ticks: u32,
}

struct App {
    world: World,
    terrain: ChunkStreamer,
    camera: Camera,
    input: Input,
    accumulator: f32,
    last: Instant,
    stats: Stats,
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
}

impl App {
    fn new(world: World) -> Self {
        let now = Instant::now();
        Self {
            terrain: ChunkStreamer::new(world.seed, 3),
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
            surface: None,
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
        let i = &self.input;
        let axis = |pos: bool, neg: bool| f32::from(pos) - f32::from(neg);
        let speed = 60.0 * dt.as_secs_f32();
        self.camera.pan(
            axis(i.right, i.left) * speed,
            axis(i.forward, i.back) * speed,
            axis(i.up, i.down) * speed,
        );
        let cam = WorldPos::new(self.camera.position[0], 0.0, self.camera.position[2]);
        self.terrain.update(cam.chunk());
        self.render();
        self.stats.frames += 1;
        let elapsed = now - self.stats.window_start;
        if elapsed >= Duration::from_secs(1) {
            let secs = elapsed.as_secs_f32();
            println!(
                "frame {:.2} ms | sim {:.1} ticks/s | entities {}",
                1000.0 * secs / self.stats.frames as f32,
                self.stats.ticks as f32 / secs,
                self.world.entity_count()
            );
            self.stats.window_start = now;
            self.stats.frames = 0;
            self.stats.ticks = 0;
        }
    }

    /// Placeholder renderer: top-down dots for each entity relative to the camera.
    fn render(&mut self) {
        let (Some(window), Some(surface)) = (&self.window, &mut self.surface) else {
            return;
        };
        let size = window.inner_size();
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            return;
        };
        if surface.resize(w, h).is_err() {
            return;
        }
        let Ok(mut buf) = surface.buffer_mut() else {
            return;
        };
        let terrain = &self.terrain;
        self.camera.aspect = size.width as f32 / size.height as f32;
        let px_per_unit = size.height as f32 / (CHUNK_SIZE * 4.0);
        let (cx, cz) = (self.camera.position[0], self.camera.position[2]);
        for py in 0..size.height as usize {
            for px in 0..size.width as usize {
                let wx = (px as f32 - size.width as f32 / 2.0) / px_per_unit + cx;
                let wz = (py as f32 - size.height as f32 / 2.0) / px_per_unit + cz;
                let wp = WorldPos::new(wx, 0.0, wz);
                let l = wp.local();
                let shade = terrain
                    .get(wp.chunk())
                    .map(|t| t.height(l.x as usize, l.z as usize))
                    .map_or(0.0, |h| h / 40.0);
                let v = (24.0 + shade * 70.0) as u32;
                buf[py * size.width as usize + px] = (v << 16) | ((v * 9 / 10) << 8) | (v * 8 / 10);
            }
        }
        for p in &self.world.positions {
            let sx = (p.x - cx) * px_per_unit + size.width as f32 / 2.0;
            let sy = (p.z - cz) * px_per_unit + size.height as f32 / 2.0;
            if sx >= 0.0 && sy >= 0.0 && (sx as u32) < size.width && (sy as u32) < size.height {
                buf[sy as usize * size.width as usize + sx as usize] = 0x00d9_a441;
            }
        }
        let _ = buf.present();
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes().with_title("Ashen World");
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Rc::new(w),
            Err(e) => {
                eprintln!("failed to create window: {e}");
                event_loop.exit();
                return;
            }
        };
        match softbuffer::Context::new(window.clone())
            .and_then(|ctx| softbuffer::Surface::new(&ctx, window.clone()))
        {
            Ok(s) => self.surface = Some(s),
            Err(e) => eprintln!("failed to create surface: {e}"),
        }
        self.window = Some(window);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::KeyboardInput { event, .. } => {
                let down = event.state == ElementState::Pressed;
                if let PhysicalKey::Code(code) = event.physical_key {
                    match code {
                        KeyCode::Escape => event_loop.exit(),
                        KeyCode::KeyA => self.input.left = down,
                        KeyCode::KeyD => self.input.right = down,
                        KeyCode::KeyW => self.input.forward = down,
                        KeyCode::KeyS => self.input.back = down,
                        KeyCode::KeyE => self.input.up = down,
                        KeyCode::KeyQ => self.input.down = down,
                        _ => {}
                    }
                }
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
    let content_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
    match content::load_content(&content_dir) {
        Ok(c) => println!(
            "content: {} factions, {} units, {} biomes",
            c.factions.len(),
            c.units.len(),
            c.biomes.len()
        ),
        Err(e) => eprintln!("content load failed: {e}"),
    }
    let world = World::generate(&WorldConfig::default());
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
