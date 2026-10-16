//! Keyboard and mouse handling: turns window events into camera, selection, build and
//! terrain-editing changes on `App`.

use core_sim::{BuildingLogistics, Footprint, PlacementRules};
use glam::Vec3;
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{KeyCode, PhysicalKey};

use crate::context_menu::{self, ContextTarget};
use crate::picking::select_players_in_rect;
use crate::terrain_editor::TerrainTool;
use crate::ui::UiAction;
use crate::App;

/// Squared cursor travel (in pixels) above which a left press becomes a drag selection
/// and a right press stops counting as a context-menu click.
const DRAG_THRESHOLD_SQ: f64 = 64.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum BindingGroup {
    Terrain,
    Camera,
    Build,
    SaveLoad,
    General,
}

impl BindingGroup {
    /// Display order in the key list.
    pub(crate) const ALL: [BindingGroup; 5] = [
        BindingGroup::Terrain,
        BindingGroup::Camera,
        BindingGroup::Build,
        BindingGroup::SaveLoad,
        BindingGroup::General,
    ];

    pub(crate) fn title(self) -> &'static str {
        match self {
            BindingGroup::Terrain => "Terrain",
            BindingGroup::Camera => "Camera",
            BindingGroup::Build => "Build",
            BindingGroup::SaveLoad => "Save / Load",
            BindingGroup::General => "General",
        }
    }
}

/// Terrain tool hotkeys: the canonical source for both key handling and the panel buttons.
pub(crate) struct TerrainToolKey {
    pub(crate) code: KeyCode,
    pub(crate) key: &'static str,
    pub(crate) tool: TerrainTool,
}

pub(crate) const TERRAIN_TOOL_KEYS: [TerrainToolKey; 5] = [
    TerrainToolKey {
        code: KeyCode::F1,
        key: "F1",
        tool: TerrainTool::Raise,
    },
    TerrainToolKey {
        code: KeyCode::F2,
        key: "F2",
        tool: TerrainTool::Lower,
    },
    TerrainToolKey {
        code: KeyCode::F3,
        key: "F3",
        tool: TerrainTool::Flatten,
    },
    TerrainToolKey {
        code: KeyCode::F4,
        key: "F4",
        tool: TerrainTool::Smooth,
    },
    TerrainToolKey {
        code: KeyCode::F5,
        key: "F5",
        tool: TerrainTool::Slope,
    },
];

/// Parses a key press into a shared command, or `None` if the key is handled elsewhere
/// (camera movement, Escape, slot selection). Execution lives in `App::apply_ui_action`.
pub(crate) fn key_command(code: KeyCode, editor_active: bool, repeat: bool) -> Option<UiAction> {
    if editor_active {
        if let Some(key) = TERRAIN_TOOL_KEYS.iter().find(|key| key.code == code) {
            return Some(UiAction::SetTerrainTool(key.tool));
        }
    }
    match code {
        KeyCode::KeyT => Some(UiAction::ToggleTerrainEditor),
        KeyCode::F5 => Some(UiAction::QuickSave),
        KeyCode::F9 => Some(UiAction::QuickLoad),
        KeyCode::F10 if !repeat => Some(UiAction::ToggleTerrainPanel),
        KeyCode::BracketLeft if editor_active => Some(UiAction::AdjustBrushRadius(-1)),
        KeyCode::BracketRight if editor_active => Some(UiAction::AdjustBrushRadius(1)),
        KeyCode::Minus if editor_active => Some(UiAction::AdjustBrushStrength(-250)),
        KeyCode::Equal if editor_active => Some(UiAction::AdjustBrushStrength(250)),
        _ => None,
    }
}

pub(crate) struct Binding {
    pub(crate) key: &'static str,
    pub(crate) description: &'static str,
    pub(crate) group: BindingGroup,
    pub(crate) terrain_relevant: bool,
}

// Keep this display table alongside handle_key and handle_mouse_button when bindings change.
pub(crate) const BINDINGS: &[Binding] = &[
    Binding {
        key: "T",
        description: "Toggle terrain editing",
        group: BindingGroup::Terrain,
        terrain_relevant: false,
    },
    Binding {
        key: "F1–F4",
        description: "Raise, lower, flatten, or smooth terrain while editing",
        group: BindingGroup::Terrain,
        terrain_relevant: true,
    },
    Binding {
        key: "F5",
        description: "Slope tool while editing; quick-save otherwise",
        group: BindingGroup::Terrain,
        terrain_relevant: true,
    },
    Binding {
        key: "[ / ]",
        description: "Decrease / increase brush radius while editing",
        group: BindingGroup::Terrain,
        terrain_relevant: true,
    },
    Binding {
        key: "- / =",
        description: "Decrease / increase brush strength while editing",
        group: BindingGroup::Terrain,
        terrain_relevant: true,
    },
    Binding {
        key: "Left mouse",
        description: "Apply terrain tool; otherwise select, move, place a building, or drag-select",
        group: BindingGroup::Build,
        terrain_relevant: true,
    },
    Binding {
        key: "A / ←",
        description: "Pan camera left",
        group: BindingGroup::Camera,
        terrain_relevant: false,
    },
    Binding {
        key: "D / →",
        description: "Pan camera right",
        group: BindingGroup::Camera,
        terrain_relevant: false,
    },
    Binding {
        key: "W / ↑",
        description: "Pan camera forward",
        group: BindingGroup::Camera,
        terrain_relevant: false,
    },
    Binding {
        key: "S / ↓",
        description: "Pan camera back",
        group: BindingGroup::Camera,
        terrain_relevant: false,
    },
    Binding {
        key: "Q / E",
        description: "Orbit camera left / right",
        group: BindingGroup::Camera,
        terrain_relevant: false,
    },
    Binding {
        key: "Right / middle mouse drag",
        description: "Orbit camera",
        group: BindingGroup::Camera,
        terrain_relevant: false,
    },
    Binding {
        key: "Mouse wheel",
        description: "Zoom camera",
        group: BindingGroup::Camera,
        terrain_relevant: false,
    },
    Binding {
        key: "F9",
        description: "Quick-load",
        group: BindingGroup::SaveLoad,
        terrain_relevant: false,
    },
    Binding {
        key: "Esc",
        description: "Close context menu, exit terrain editing, cancel placement, or quit",
        group: BindingGroup::General,
        terrain_relevant: false,
    },
    Binding {
        key: "1 / 2 / 3",
        description: "Select survivor slot",
        group: BindingGroup::General,
        terrain_relevant: false,
    },
    Binding {
        key: "Space",
        description: "Center camera on selected survivor",
        group: BindingGroup::General,
        terrain_relevant: false,
    },
    Binding {
        key: "R",
        description: "Recruit nearest eligible survivor",
        group: BindingGroup::General,
        terrain_relevant: false,
    },
    Binding {
        key: "Right click",
        description: "Open selected worker's context menu; drag to orbit",
        group: BindingGroup::General,
        terrain_relevant: false,
    },
    Binding {
        key: "F10",
        description: "Show or hide this panel",
        group: BindingGroup::General,
        terrain_relevant: false,
    },
];

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
            if let Some(command) = key_command(code, self.editor.active, event.repeat) {
                self.apply_ui_action(command);
                return;
            }
            if code == KeyCode::F10 {
                return;
            }
        }
        match code {
            KeyCode::Escape if down => self.handle_escape(event_loop),
            KeyCode::KeyA | KeyCode::ArrowLeft => self.input.left = down,
            KeyCode::KeyD | KeyCode::ArrowRight => self.input.right = down,
            KeyCode::KeyW | KeyCode::ArrowUp => self.input.forward = down,
            KeyCode::KeyS | KeyCode::ArrowDown => self.input.back = down,
            KeyCode::KeyQ if down => self.camera.orbit(-0.2, 0.0),
            KeyCode::KeyE if down => self.camera.orbit(0.2, 0.0),
            KeyCode::Digit1 if down => self.select_player_slot(0),
            KeyCode::Digit2 if down => self.select_player_slot(1),
            KeyCode::Digit3 if down => self.select_player_slot(2),
            KeyCode::Space if down => self.center_camera_on_selected(),
            KeyCode::KeyR if down => self.recruit_nearest(),
            _ => {}
        }
    }

    /// Escape peels back one layer: context menu, terrain editor, building placement, then quit.
    fn handle_escape(&mut self, event_loop: &ActiveEventLoop) {
        if self.ui.as_mut().is_some_and(|ui| ui.close_context_menu()) {
            return;
        }
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

    fn select_player_slot(&mut self, slot: usize) {
        let player = self.world.player_characters().nth(slot);
        if let Some(player) = player {
            let _ = self.world.select_character(player);
            self.selected_entities = vec![player];
        }
    }

    fn center_camera_on_selected(&mut self) {
        if let Some(selected) = self.world.selected_character() {
            let p = self.world.position(selected);
            self.camera.target = Vec3::new(
                p.x,
                self.world.terrain_edits().height(self.world.seed, p.x, p.z),
                p.z,
            );
        }
    }

    fn recruit_nearest(&mut self) {
        let Some(recruiter) = self.world.selected_character() else {
            return;
        };
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
    use std::collections::HashSet;

    #[test]
    fn drag_distance_is_squared_euclidean() {
        assert_eq!(drag_distance_sq((0.0, 0.0), (3.0, 4.0)), 25.0);
        assert!(drag_distance_sq((10.0, 10.0), (12.0, 12.0)) < DRAG_THRESHOLD_SQ);
        assert!(drag_distance_sq((0.0, 0.0), (8.0, 0.0)) >= DRAG_THRESHOLD_SQ);
    }

    #[test]
    fn terrain_keys_map_to_commands_only_while_editing() {
        assert_eq!(
            key_command(KeyCode::F1, true, false),
            Some(UiAction::SetTerrainTool(TerrainTool::Raise))
        );
        assert_eq!(
            key_command(KeyCode::F5, true, false),
            Some(UiAction::SetTerrainTool(TerrainTool::Slope))
        );
        assert_eq!(
            key_command(KeyCode::F5, false, false),
            Some(UiAction::QuickSave)
        );
        assert_eq!(key_command(KeyCode::F1, false, false), None);
        assert_eq!(key_command(KeyCode::BracketLeft, false, false), None);
        assert_eq!(
            key_command(KeyCode::Equal, true, false),
            Some(UiAction::AdjustBrushStrength(250))
        );
        assert_eq!(
            key_command(KeyCode::KeyT, false, false),
            Some(UiAction::ToggleTerrainEditor)
        );
        assert_eq!(key_command(KeyCode::F10, false, true), None);
        assert_eq!(key_command(KeyCode::KeyW, false, false), None);
    }

    #[test]
    fn every_terrain_tool_key_is_listed_in_the_binding_table() {
        // The table lists the range "F1–F4" plus "F5", so check the endpoints.
        for key in ["F1", "F4", "F5"] {
            assert!(TERRAIN_TOOL_KEYS.iter().any(|k| k.key == key));
            assert!(BINDINGS.iter().any(|b| b.key.contains(key)));
        }
        for group in BindingGroup::ALL {
            assert!(!group.title().is_empty());
            assert!(BINDINGS.iter().any(|b| b.group == group));
        }
    }

    #[test]
    fn binding_table_has_unique_keys_and_nonempty_text() {
        let mut keys = HashSet::new();
        for binding in BINDINGS {
            assert!(
                keys.insert(binding.key),
                "duplicate binding key: {}",
                binding.key
            );
            assert!(!binding.key.trim().is_empty());
            assert!(!binding.description.trim().is_empty());
        }
    }
}
