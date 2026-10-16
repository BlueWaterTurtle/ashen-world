//! Keyboard and mouse handling: turns window events into camera, selection, build and
//! terrain-editing changes on `App`.

use core_sim::{BuildingLogistics, Footprint, PlacementRules};
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{KeyCode, PhysicalKey};

use crate::actions::key_action;
use crate::context_menu::{self, ContextTarget};
use crate::picking::select_players_in_rect;
use crate::App;

/// Squared cursor travel (in pixels) above which a left press becomes a drag selection
/// and a right press stops counting as a context-menu click.
const DRAG_THRESHOLD_SQ: f64 = 64.0;

#[derive(Default)]
pub(crate) struct Input {
    pub(crate) left: bool,
    pub(crate) right: bool,
    pub(crate) forward: bool,
    pub(crate) back: bool,
    pub(crate) orbiting: bool,
    pub(crate) ui_pointer_captured: bool,
    pub(crate) last_cursor: Option<(f64, f64)>,
    pub(crate) left_press: Option<(f64, f64)>,
    pub(crate) selection_rect: Option<[f32; 4]>,
    pub(crate) right_press: Option<(f64, f64)>,
}

fn is_left_release(event: &WindowEvent) -> bool {
    matches!(
        event,
        WindowEvent::MouseInput {
            state: ElementState::Released,
            button: MouseButton::Left,
            ..
        }
    )
}

impl App {
    /// Offer `event` to the HUD first. Returns true when the HUD (or an in-progress HUD
    /// drag) owns it, in which case the world must not react.
    pub(super) fn consume_ui_event(&mut self, event: &WindowEvent) -> bool {
        let ui_consumed = match (&self.window, &mut self.ui) {
            (Some(window), Some(ui)) => ui.on_window_event(window, event),
            _ => false,
        };
        let is_ui_input = matches!(
            event,
            WindowEvent::KeyboardInput { .. }
                | WindowEvent::MouseInput { .. }
                | WindowEvent::CursorMoved { .. }
                | WindowEvent::MouseWheel { .. }
        );
        if self.input.ui_pointer_captured && is_left_release(event) {
            self.input.ui_pointer_captured = false;
        }
        if !(ui_consumed || self.input.ui_pointer_captured) {
            return false;
        }
        if is_left_release(event) {
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
        is_ui_input
    }

    pub(super) fn handle_key(&mut self, event_loop: &ActiveEventLoop, event: &KeyEvent) {
        let down = event.state == ElementState::Pressed;
        let PhysicalKey::Code(code) = event.physical_key else {
            return;
        };
        if down {
            if let Some(command) = key_action(code, &self.action_context(), event.repeat) {
                self.dispatch_action(command);
                if self.quit_requested {
                    event_loop.exit();
                }
                return;
            }
            if code == KeyCode::F10 {
                return;
            }
        }
        // Held-state camera input stays here; it is continuous, not a discrete action.
        match code {
            KeyCode::KeyA | KeyCode::ArrowLeft => self.input.left = down,
            KeyCode::KeyD | KeyCode::ArrowRight => self.input.right = down,
            KeyCode::KeyW | KeyCode::ArrowUp => self.input.forward = down,
            KeyCode::KeyS | KeyCode::ArrowDown => self.input.back = down,
            KeyCode::KeyQ if down => self.camera.orbit(-0.2, 0.0),
            KeyCode::KeyE if down => self.camera.orbit(0.2, 0.0),
            _ => {}
        }
    }

    pub(super) fn handle_mouse_button(&mut self, state: ElementState, button: MouseButton) {
        match button {
            MouseButton::Right | MouseButton::Middle => {
                self.input.orbiting = state == ElementState::Pressed;
                if button == MouseButton::Right {
                    if state == ElementState::Pressed {
                        self.input.right_press = self.input.last_cursor;
                    } else if let (Some(start), Some(end)) =
                        (self.input.right_press.take(), self.input.last_cursor)
                    {
                        if drag_distance_sq(start, end) < DRAG_THRESHOLD_SQ {
                            self.handle_context_click(end.0 as f32, end.1 as f32);
                        }
                    }
                }
                if button != MouseButton::Right || state == ElementState::Released {
                    self.input.last_cursor = None;
                }
            }
            MouseButton::Left if state == ElementState::Pressed => {
                self.input.left_press = self.input.last_cursor;
                self.input.selection_rect = None;
            }
            MouseButton::Left => self.finish_left_press(),
            _ => {}
        }
    }

    /// A left release is either a drag selection or, if the cursor barely moved, a click.
    fn finish_left_press(&mut self) {
        let start = self.input.left_press.take();
        let end = self.input.last_cursor;
        self.input.selection_rect = None;
        let (Some(start), Some(end)) = (start, end) else {
            return;
        };
        if drag_distance_sq(start, end) < DRAG_THRESHOLD_SQ {
            self.handle_world_click(end.0 as f32, end.1 as f32);
            return;
        }
        if self.build_mode.is_some() || self.editor.active {
            return;
        }
        let Some(window) = &self.window else {
            return;
        };
        let size = window.inner_size();
        let selected = select_players_in_rect(
            &self.world,
            self.camera.view_proj(),
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

    pub(super) fn handle_cursor_moved(&mut self, x: f64, y: f64) {
        if self.input.orbiting {
            if let Some((lx, ly)) = self.input.last_cursor {
                self.camera
                    .orbit(-(x - lx) as f32 * 0.005, (y - ly) as f32 * 0.005);
            }
        }
        self.input.last_cursor = Some((x, y));
        if self.build_mode.is_none() {
            if let Some(start) = self.input.left_press {
                if drag_distance_sq(start, (x, y)) >= DRAG_THRESHOLD_SQ {
                    self.input.selection_rect =
                        Some([start.0 as f32, start.1 as f32, x as f32, y as f32]);
                }
            }
        }
    }

    pub(super) fn handle_mouse_wheel(&mut self, delta: MouseScrollDelta) {
        let steps = match delta {
            MouseScrollDelta::LineDelta(_, y) => y,
            MouseScrollDelta::PixelDelta(p) => p.y as f32 / 40.0,
        };
        self.camera.zoom(steps);
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
            self.place_building(&definition, footprint, &rules, logistics);
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
}

fn drag_distance_sq(start: (f64, f64), end: (f64, f64)) -> f64 {
    let dx = end.0 - start.0;
    let dy = end.1 - start.1;
    dx * dx + dy * dy
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_distance_is_squared_euclidean() {
        assert_eq!(drag_distance_sq((0.0, 0.0), (3.0, 4.0)), 25.0);
        assert!(drag_distance_sq((10.0, 10.0), (12.0, 12.0)) < DRAG_THRESHOLD_SQ);
        assert!(drag_distance_sq((0.0, 0.0), (8.0, 0.0)) >= DRAG_THRESHOLD_SQ);
    }
}
