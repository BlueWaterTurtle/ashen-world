//! The shared action model for UI-triggered commands. Every surface (keyboard, context menu,
//! side panel, terrain panel) emits an `Action`; `App::dispatch_action` is the only place
//! that executes one. `ActionDescriptor`s are the single source for labels, hotkeys,
//! grouping and availability, including the keys panel.
//!
//! Availability and the checks in `dispatch_action` are UX prechecks only: state changes
//! still go through `core_sim`, which remains the rules authority.

use core_sim::{BuildingLogistics, Footprint, Job, PlacementRules, ResourceKind, WorldPos};
use glam::Vec3;
use winit::keyboard::KeyCode;

use crate::terrain_editor::TerrainTool;
use crate::App;

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Select(core_sim::Entity),
    /// Add or remove a survivor from the current selection (ctrl/shift-click).
    ToggleSelect(core_sim::Entity),
    Recruit(core_sim::Entity),
    SelectBuilding(String),
    CancelBuilding,
    AddJob {
        worker: core_sim::Entity,
        building: usize,
    },
    AddBuild {
        worker: core_sim::Entity,
        site: usize,
        from: usize,
    },
    AddHaul {
        worker: core_sim::Entity,
        from: usize,
        to: usize,
    },
    RemoveJob {
        worker: core_sim::Entity,
        index: usize,
    },
    MoveJobUp {
        worker: core_sim::Entity,
        index: usize,
    },
    MoveJobDown {
        worker: core_sim::Entity,
        index: usize,
    },
    ResumeJobs(core_sim::Entity),
    /// Move the selected squad so its anchor stands at this ground point.
    MoveSelected {
        x: f32,
        z: f32,
    },
    InspectBuilding(usize),
    ToggleTerrainEditor,
    SetTerrainTool(TerrainTool),
    ToggleTerrainPanel,
    QuickSave,
    QuickLoad,
    AdjustBrushRadius(i32),
    AdjustBrushStrength(i32),
    /// Select the Nth player-controlled survivor (0-based).
    SelectSlot(usize),
    CenterCamera,
    RecruitNearest,
    /// Peels back one UI layer: context menu, terrain editor, building placement, then quit.
    Escape,
}

/// Display grouping for the keys panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ActionGroup {
    Terrain,
    Camera,
    Build,
    SaveLoad,
    General,
}

impl ActionGroup {
    /// Display order in the key list.
    pub(crate) const ALL: [ActionGroup; 5] = [
        ActionGroup::Terrain,
        ActionGroup::Camera,
        ActionGroup::Build,
        ActionGroup::SaveLoad,
        ActionGroup::General,
    ];

    pub(crate) fn title(self) -> &'static str {
        match self {
            ActionGroup::Terrain => "Terrain",
            ActionGroup::Camera => "Camera",
            ActionGroup::Build => "Build",
            ActionGroup::SaveLoad => "Save / Load",
            ActionGroup::General => "General",
        }
    }
}

/// The slice of app state that decides whether an action is offered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ActionContext {
    pub(crate) editor_active: bool,
}

/// When an action can be triggered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Availability {
    Always,
    EditorActive,
    EditorInactive,
}

impl Availability {
    pub(crate) fn is_met(self, context: &ActionContext) -> bool {
        match self {
            Availability::Always => true,
            Availability::EditorActive => context.editor_active,
            Availability::EditorInactive => !context.editor_active,
        }
    }

    /// Whether some context satisfies both, i.e. a shared hotkey would be ambiguous.
    #[cfg(test)]
    pub(crate) fn overlaps(self, other: Availability) -> bool {
        !matches!(
            (self, other),
            (Availability::EditorActive, Availability::EditorInactive)
                | (Availability::EditorInactive, Availability::EditorActive)
        )
    }

    /// Suffix used to tell apart rows that share a hotkey label.
    pub(crate) fn tag(self) -> &'static str {
        match self {
            Availability::Always => "",
            Availability::EditorActive => " (editing)",
            Availability::EditorInactive => " (not editing)",
        }
    }
}

/// Text, hotkey and availability for one action.
pub(crate) struct ActionDescriptor {
    pub(crate) label: &'static str,
    /// Hotkey as shown to the player, if the action has a keyboard binding.
    pub(crate) hotkey: Option<&'static str>,
    /// Key that triggers `action`; set exactly when `hotkey` is.
    pub(crate) key: Option<KeyCode>,
    /// The concrete action a hotkey fires. `None` for actions that need a target.
    pub(crate) action: Option<Action>,
    pub(crate) group: ActionGroup,
    pub(crate) description: &'static str,
    pub(crate) availability: Availability,
    /// Whether holding the key repeats the action.
    pub(crate) repeats: bool,
}

const fn keyed(
    label: &'static str,
    hotkey: &'static str,
    key: KeyCode,
    action: Action,
    group: ActionGroup,
    description: &'static str,
    availability: Availability,
) -> ActionDescriptor {
    ActionDescriptor {
        label,
        hotkey: Some(hotkey),
        key: Some(key),
        action: Some(action),
        group,
        description,
        availability,
        repeats: true,
    }
}

const fn unkeyed(
    label: &'static str,
    group: ActionGroup,
    description: &'static str,
) -> ActionDescriptor {
    ActionDescriptor {
        label,
        hotkey: None,
        key: None,
        action: None,
        group,
        description,
        availability: Availability::Always,
        repeats: true,
    }
}

use ActionGroup as G;
use Availability as A;

static TOGGLE_EDITOR: ActionDescriptor = keyed(
    "Terrain editing",
    "T",
    KeyCode::KeyT,
    Action::ToggleTerrainEditor,
    G::Terrain,
    "Toggle terrain editing",
    A::Always,
);
static TOOL_RAISE: ActionDescriptor = keyed(
    "Raise",
    "F1",
    KeyCode::F1,
    Action::SetTerrainTool(TerrainTool::Raise),
    G::Terrain,
    "Raise terrain tool (while editing)",
    A::EditorActive,
);
static TOOL_LOWER: ActionDescriptor = keyed(
    "Lower",
    "F2",
    KeyCode::F2,
    Action::SetTerrainTool(TerrainTool::Lower),
    G::Terrain,
    "Lower terrain tool (while editing)",
    A::EditorActive,
);
static TOOL_FLATTEN: ActionDescriptor = keyed(
    "Flatten",
    "F3",
    KeyCode::F3,
    Action::SetTerrainTool(TerrainTool::Flatten),
    G::Terrain,
    "Flatten terrain tool (while editing)",
    A::EditorActive,
);
static TOOL_SMOOTH: ActionDescriptor = keyed(
    "Smooth",
    "F4",
    KeyCode::F4,
    Action::SetTerrainTool(TerrainTool::Smooth),
    G::Terrain,
    "Smooth terrain tool (while editing)",
    A::EditorActive,
);
static TOOL_SLOPE: ActionDescriptor = keyed(
    "Slope",
    "F5",
    KeyCode::F5,
    Action::SetTerrainTool(TerrainTool::Slope),
    G::Terrain,
    "Select slope tool (quick-save is F5 when not editing)",
    A::EditorActive,
);
static RADIUS_DOWN: ActionDescriptor = keyed(
    "Brush radius -",
    "[",
    KeyCode::BracketLeft,
    Action::AdjustBrushRadius(-1),
    G::Terrain,
    "Decrease brush radius while editing",
    A::EditorActive,
);
static RADIUS_UP: ActionDescriptor = keyed(
    "Brush radius +",
    "]",
    KeyCode::BracketRight,
    Action::AdjustBrushRadius(1),
    G::Terrain,
    "Increase brush radius while editing",
    A::EditorActive,
);
static STRENGTH_DOWN: ActionDescriptor = keyed(
    "Brush strength -",
    "-",
    KeyCode::Minus,
    Action::AdjustBrushStrength(-250),
    G::Terrain,
    "Decrease brush strength while editing",
    A::EditorActive,
);
static STRENGTH_UP: ActionDescriptor = keyed(
    "Brush strength +",
    "=",
    KeyCode::Equal,
    Action::AdjustBrushStrength(250),
    G::Terrain,
    "Increase brush strength while editing",
    A::EditorActive,
);
static QUICK_SAVE: ActionDescriptor = keyed(
    "Quick-save",
    "F5",
    KeyCode::F5,
    Action::QuickSave,
    G::SaveLoad,
    "Quick-save (selects the slope tool while terrain editing)",
    A::EditorInactive,
);
static QUICK_LOAD: ActionDescriptor = keyed(
    "Quick-load",
    "F9",
    KeyCode::F9,
    Action::QuickLoad,
    G::SaveLoad,
    "Quick-load",
    A::Always,
);
static TOGGLE_KEYS_PANEL: ActionDescriptor = {
    let mut descriptor = keyed(
        "Terrain tool & keys panel",
        "F10",
        KeyCode::F10,
        Action::ToggleTerrainPanel,
        G::General,
        "Show or hide the terrain tool & keys panel",
        A::Always,
    );
    descriptor.repeats = false;
    descriptor
};
static ESCAPE: ActionDescriptor = keyed(
    "Cancel",
    "Esc",
    KeyCode::Escape,
    Action::Escape,
    G::General,
    "Close context menu, exit terrain editing, cancel placement, or quit",
    A::Always,
);
static SLOT_1: ActionDescriptor = keyed(
    "Select survivor 1",
    "1",
    KeyCode::Digit1,
    Action::SelectSlot(0),
    G::General,
    "Select survivor slot 1",
    A::Always,
);
static SLOT_2: ActionDescriptor = keyed(
    "Select survivor 2",
    "2",
    KeyCode::Digit2,
    Action::SelectSlot(1),
    G::General,
    "Select survivor slot 2",
    A::Always,
);
static SLOT_3: ActionDescriptor = keyed(
    "Select survivor 3",
    "3",
    KeyCode::Digit3,
    Action::SelectSlot(2),
    G::General,
    "Select survivor slot 3",
    A::Always,
);
static CENTER_CAMERA: ActionDescriptor = keyed(
    "Center camera",
    "Space",
    KeyCode::Space,
    Action::CenterCamera,
    G::General,
    "Center camera on selected survivor",
    A::Always,
);
static RECRUIT_NEAREST: ActionDescriptor = keyed(
    "Recruit nearest",
    "R",
    KeyCode::KeyR,
    Action::RecruitNearest,
    G::General,
    "Recruit nearest eligible survivor",
    A::Always,
);

// Actions without a hotkey: their labels are used by the context menu and side panel.
static SELECT: ActionDescriptor = unkeyed("Select", G::General, "Select a survivor");
static TOGGLE_SELECT: ActionDescriptor = unkeyed(
    "Toggle selection",
    G::General,
    "Add or remove a survivor from the selection",
);
static RECRUIT: ActionDescriptor = unkeyed("Recruit", G::General, "Recruit a nearby survivor");
static SELECT_BUILDING: ActionDescriptor =
    unkeyed("Place building", G::Build, "Choose a building to place");
static CANCEL_BUILDING: ActionDescriptor =
    unkeyed("Cancel placement", G::Build, "Cancel building placement");
static ADD_JOB: ActionDescriptor = unkeyed("Work", G::General, "Queue a work job");
static ADD_BUILD: ActionDescriptor = unkeyed("Build", G::Build, "Queue a construction job");
static ADD_HAUL: ActionDescriptor = unkeyed("Haul", G::General, "Queue a haul job");
static REMOVE_JOB: ActionDescriptor = unkeyed("Remove job", G::General, "Remove a queued job");
static MOVE_JOB_UP: ActionDescriptor =
    unkeyed("Move job up", G::General, "Move a queued job earlier");
static MOVE_JOB_DOWN: ActionDescriptor =
    unkeyed("Move job down", G::General, "Move a queued job later");
static RESUME_JOBS: ActionDescriptor = unkeyed("Resume jobs", G::General, "Resume a paused queue");
static MOVE_SELECTED: ActionDescriptor = unkeyed(
    "Move here",
    G::General,
    "Move the selected squad to a point",
);
static INSPECT_BUILDING: ActionDescriptor =
    unkeyed("Inspect", G::Build, "Show a building's status");

/// Every descriptor, in keys-panel order.
pub(crate) static DESCRIPTORS: &[&ActionDescriptor] = &[
    &TOGGLE_EDITOR,
    &TOOL_RAISE,
    &TOOL_LOWER,
    &TOOL_FLATTEN,
    &TOOL_SMOOTH,
    &TOOL_SLOPE,
    &RADIUS_DOWN,
    &RADIUS_UP,
    &STRENGTH_DOWN,
    &STRENGTH_UP,
    &QUICK_SAVE,
    &QUICK_LOAD,
    &TOGGLE_KEYS_PANEL,
    &ESCAPE,
    &SLOT_1,
    &SLOT_2,
    &SLOT_3,
    &CENTER_CAMERA,
    &RECRUIT_NEAREST,
    &SELECT,
    &TOGGLE_SELECT,
    &RECRUIT,
    &SELECT_BUILDING,
    &CANCEL_BUILDING,
    &ADD_JOB,
    &ADD_BUILD,
    &ADD_HAUL,
    &REMOVE_JOB,
    &MOVE_JOB_UP,
    &MOVE_JOB_DOWN,
    &RESUME_JOBS,
    &MOVE_SELECTED,
    &INSPECT_BUILDING,
];

impl Action {
    /// Terrain tools in button order.
    pub(crate) const TERRAIN_TOOLS: [TerrainTool; 5] = [
        TerrainTool::Raise,
        TerrainTool::Lower,
        TerrainTool::Flatten,
        TerrainTool::Smooth,
        TerrainTool::Slope,
    ];

    /// The descriptor for this action. Exhaustive, so a new variant needs a descriptor.
    pub(crate) fn descriptor(&self) -> &'static ActionDescriptor {
        match self {
            Action::Select(_) => &SELECT,
            Action::ToggleSelect(_) => &TOGGLE_SELECT,
            Action::Recruit(_) => &RECRUIT,
            Action::SelectBuilding(_) => &SELECT_BUILDING,
            Action::CancelBuilding => &CANCEL_BUILDING,
            Action::AddJob { .. } => &ADD_JOB,
            Action::AddBuild { .. } => &ADD_BUILD,
            Action::AddHaul { .. } => &ADD_HAUL,
            Action::RemoveJob { .. } => &REMOVE_JOB,
            Action::MoveJobUp { .. } => &MOVE_JOB_UP,
            Action::MoveJobDown { .. } => &MOVE_JOB_DOWN,
            Action::ResumeJobs(_) => &RESUME_JOBS,
            Action::MoveSelected { .. } => &MOVE_SELECTED,
            Action::InspectBuilding(_) => &INSPECT_BUILDING,
            Action::ToggleTerrainEditor => &TOGGLE_EDITOR,
            Action::SetTerrainTool(tool) => match tool {
                TerrainTool::Raise => &TOOL_RAISE,
                TerrainTool::Lower => &TOOL_LOWER,
                TerrainTool::Flatten => &TOOL_FLATTEN,
                TerrainTool::Smooth => &TOOL_SMOOTH,
                TerrainTool::Slope => &TOOL_SLOPE,
            },
            Action::ToggleTerrainPanel => &TOGGLE_KEYS_PANEL,
            Action::QuickSave => &QUICK_SAVE,
            Action::QuickLoad => &QUICK_LOAD,
            Action::AdjustBrushRadius(step) if *step < 0 => &RADIUS_DOWN,
            Action::AdjustBrushRadius(_) => &RADIUS_UP,
            Action::AdjustBrushStrength(step) if *step < 0 => &STRENGTH_DOWN,
            Action::AdjustBrushStrength(_) => &STRENGTH_UP,
            Action::SelectSlot(0) => &SLOT_1,
            Action::SelectSlot(1) => &SLOT_2,
            Action::SelectSlot(_) => &SLOT_3,
            Action::CenterCamera => &CENTER_CAMERA,
            Action::RecruitNearest => &RECRUIT_NEAREST,
            Action::Escape => &ESCAPE,
        }
    }

    pub(crate) fn is_available(&self, context: &ActionContext) -> bool {
        self.descriptor().availability.is_met(context)
    }
}

/// Pure key-to-action mapping: the first available descriptor bound to `code` wins.
/// Camera panning/orbiting and mouse input are handled directly in `input.rs`.
pub(crate) fn key_action(code: KeyCode, context: &ActionContext, repeat: bool) -> Option<Action> {
    DESCRIPTORS
        .iter()
        .filter(|descriptor| descriptor.key == Some(code))
        .filter(|descriptor| descriptor.availability.is_met(context))
        .find(|descriptor| !repeat || descriptor.repeats)
        .and_then(|descriptor| descriptor.action.clone())
}

/// Adds `entity` to the selection, or removes it, never emptying the selection.
fn toggle_selection(selection: &mut Vec<core_sim::Entity>, entity: core_sim::Entity) {
    if let Some(index) = selection.iter().position(|&e| e == entity) {
        if selection.len() > 1 {
            selection.remove(index);
        }
    } else {
        selection.push(entity);
    }
}

impl App {
    pub(crate) fn action_context(&self) -> ActionContext {
        ActionContext {
            editor_active: self.editor.active,
        }
    }

    pub(super) fn dispatch_action(&mut self, action: Action) {
        match action {
            Action::SelectSlot(slot) => self.select_player_slot(slot),
            Action::CenterCamera => self.center_camera_on_selected(),
            Action::RecruitNearest => self.recruit_nearest(),
            Action::Escape => self.escape(),
            Action::Select(entity) => {
                let _ = self.world.select_character(entity);
                self.selected_entities.clear();
                self.selected_entities.push(entity);
            }
            Action::ToggleSelect(entity) => {
                toggle_selection(&mut self.selected_entities, entity);
                let primary_kept = self
                    .world
                    .selected_character()
                    .is_some_and(|primary| self.selected_entities.contains(&primary));
                if !primary_kept {
                    if let Some(&first) = self.selected_entities.first() {
                        let _ = self.world.select_character(first);
                    }
                }
            }
            Action::Recruit(entity) => {
                if let Some(recruiter) = self.world.selected_character() {
                    if self.world.recruit_with(recruiter, entity).is_ok() {
                        self.selected_entities.push(entity);
                    }
                }
            }
            Action::SelectBuilding(id) => {
                self.editor.active = false;
                self.build_mode = Some(id.clone());
                self.build_status = None;
                if let Some(ui) = &mut self.ui {
                    ui.set_selected_building(Some(id));
                }
            }
            Action::AddJob { worker, building } => {
                self.jobs.add_job(worker, Job::Work { building });
            }
            Action::AddBuild { worker, site, from } => {
                self.jobs.add_job(worker, Job::Build { site, from });
            }
            Action::AddHaul { worker, from, to } => {
                self.jobs.add_job(
                    worker,
                    Job::Haul {
                        from,
                        to,
                        item: ResourceKind::IronOre,
                    },
                );
            }
            Action::RemoveJob { worker, index } => {
                self.jobs
                    .remove_job(worker, index, &mut self.buildings, &mut self.world);
            }
            Action::MoveJobUp { worker, index } => self.jobs.move_up(worker, index),
            Action::MoveJobDown { worker, index } => self.jobs.move_down(worker, index),
            Action::ResumeJobs(worker) => self.jobs.resume(worker),
            Action::MoveSelected { x, z } => self.move_selected_to(x, z),
            Action::InspectBuilding(index) => self.inspect_building(index),
            Action::ToggleTerrainEditor => self.toggle_terrain_editor(),
            Action::SetTerrainTool(tool) => {
                if self.editor.active {
                    self.editor.set_tool(tool);
                    self.build_status = Some(self.editor.status());
                }
            }
            Action::ToggleTerrainPanel => {
                if let Some(ui) = &mut self.ui {
                    ui.toggle_terrain_panel();
                }
            }
            Action::QuickSave => self.quick_save(),
            Action::QuickLoad => self.quick_load(),
            Action::AdjustBrushRadius(step) => {
                if self.editor.active {
                    self.editor.adjust_radius(step);
                    self.build_status = Some(self.editor.status());
                }
            }
            Action::AdjustBrushStrength(step_mm) => {
                if self.editor.active {
                    self.editor.adjust_strength(step_mm);
                    self.build_status = Some(self.editor.status());
                }
            }
            Action::CancelBuilding => {
                self.build_mode = None;
                self.build_status = None;
                if let Some(ui) = &mut self.ui {
                    ui.set_selected_building(None);
                }
            }
        }
    }

    /// Escape peels back one layer: context menu, terrain editor, building placement, then quit.
    fn escape(&mut self) {
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
            self.quit_requested = true;
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

    fn toggle_terrain_editor(&mut self) {
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

    pub(super) fn move_selected_to(&mut self, x: f32, z: f32) {
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

    /// Place `definition`: free buildings appear at once, costed ones become
    /// construction sites that idle survivors supply and finish.
    pub(super) fn place_building(
        &mut self,
        definition: &content::BuildingDef,
        footprint: Footprint,
        rules: &PlacementRules,
        logistics: BuildingLogistics,
    ) {
        let result = match construction_cost(definition) {
            Some((material, required)) => self
                .buildings
                .request_construction(
                    self.world.seed,
                    self.world.terrain_edits(),
                    &definition.id,
                    footprint,
                    rules,
                    logistics,
                    material,
                    required,
                )
                .map(|_| {
                    format!(
                        "{} site placed: survivors will bring {} {}.",
                        definition.name,
                        required,
                        material.id()
                    )
                }),
            None => self
                .buildings
                .place_with_logistics(
                    self.world.seed,
                    self.world.terrain_edits(),
                    &definition.id,
                    footprint,
                    rules,
                    logistics,
                )
                .map(|_| format!("Placed {}.", definition.name)),
        };
        match result {
            Ok(status) => {
                self.build_status = Some(status);
                self.build_mode = None;
                if let Some(ui) = &mut self.ui {
                    ui.set_selected_building(None);
                }
            }
            Err(error) => {
                self.build_status = Some(format!("Can't build here: {error:?}."));
            }
        }
    }
}

/// The material and amount a building costs, or `None` if it is free.
pub(super) fn construction_cost(definition: &content::BuildingDef) -> Option<(ResourceKind, u32)> {
    definition.cost.iter().find_map(|(id, amount)| {
        let required = amount.max(0.0).round() as u32;
        let material = ResourceKind::from_id(id)?;
        (required > 0).then_some((material, required))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_sim::{Entity, ResourceKind};

    const EDITING: ActionContext = ActionContext {
        editor_active: true,
    };
    const IDLE: ActionContext = ActionContext {
        editor_active: false,
    };

    /// One instance of every variant; the exhaustive match below fails to compile when a
    /// variant is added without being listed here.
    fn every_variant() -> Vec<Action> {
        let e = Entity(1);
        let all = vec![
            Action::Select(e),
            Action::ToggleSelect(e),
            Action::Recruit(e),
            Action::SelectBuilding("x".into()),
            Action::CancelBuilding,
            Action::AddJob {
                worker: e,
                building: 0,
            },
            Action::AddBuild {
                worker: e,
                site: 0,
                from: 0,
            },
            Action::AddHaul {
                worker: e,
                from: 0,
                to: 1,
            },
            Action::RemoveJob {
                worker: e,
                index: 0,
            },
            Action::MoveJobUp {
                worker: e,
                index: 0,
            },
            Action::MoveJobDown {
                worker: e,
                index: 0,
            },
            Action::ResumeJobs(e),
            Action::MoveSelected { x: 0.0, z: 0.0 },
            Action::InspectBuilding(0),
            Action::ToggleTerrainEditor,
            Action::SetTerrainTool(TerrainTool::Raise),
            Action::ToggleTerrainPanel,
            Action::QuickSave,
            Action::QuickLoad,
            Action::AdjustBrushRadius(1),
            Action::AdjustBrushStrength(1),
            Action::SelectSlot(0),
            Action::CenterCamera,
            Action::RecruitNearest,
            Action::Escape,
        ];
        for action in &all {
            match action {
                Action::Select(_)
                | Action::ToggleSelect(_)
                | Action::Recruit(_)
                | Action::SelectBuilding(_)
                | Action::CancelBuilding
                | Action::AddJob { .. }
                | Action::AddBuild { .. }
                | Action::AddHaul { .. }
                | Action::RemoveJob { .. }
                | Action::MoveJobUp { .. }
                | Action::MoveJobDown { .. }
                | Action::ResumeJobs(_)
                | Action::MoveSelected { .. }
                | Action::InspectBuilding(_)
                | Action::ToggleTerrainEditor
                | Action::SetTerrainTool(_)
                | Action::ToggleTerrainPanel
                | Action::QuickSave
                | Action::QuickLoad
                | Action::AdjustBrushRadius(_)
                | Action::AdjustBrushStrength(_)
                | Action::SelectSlot(_)
                | Action::CenterCamera
                | Action::RecruitNearest
                | Action::Escape => {}
            }
        }
        all
    }

    #[test]
    fn every_variant_has_a_listed_descriptor() {
        for action in every_variant() {
            let descriptor = action.descriptor();
            assert!(
                DESCRIPTORS.iter().any(|d| std::ptr::eq(*d, descriptor)),
                "{action:?} descriptor missing from DESCRIPTORS"
            );
        }
    }

    #[test]
    fn descriptors_have_text_and_consistent_keys() {
        for d in DESCRIPTORS {
            assert!(!d.label.trim().is_empty());
            assert!(!d.description.trim().is_empty());
            assert_eq!(d.hotkey.is_some(), d.key.is_some(), "{}", d.label);
            assert_eq!(d.hotkey.is_some(), d.action.is_some(), "{}", d.label);
            if let Some(action) = &d.action {
                assert!(std::ptr::eq(action.descriptor(), *d), "{}", d.label);
            }
        }
    }

    #[test]
    fn hotkeys_are_unique_within_a_context() {
        for (i, a) in DESCRIPTORS.iter().enumerate() {
            for b in &DESCRIPTORS[i + 1..] {
                if a.hotkey.is_some() && a.hotkey == b.hotkey {
                    assert!(
                        !a.availability.overlaps(b.availability),
                        "{} and {} share a hotkey",
                        a.label,
                        b.label
                    );
                }
                if a.key.is_some() && a.key == b.key {
                    assert!(!a.availability.overlaps(b.availability));
                }
            }
        }
    }

    #[test]
    fn terrain_keys_map_to_actions_only_while_editing() {
        assert_eq!(
            key_action(KeyCode::F1, &EDITING, false),
            Some(Action::SetTerrainTool(TerrainTool::Raise))
        );
        assert_eq!(
            key_action(KeyCode::F5, &EDITING, false),
            Some(Action::SetTerrainTool(TerrainTool::Slope))
        );
        assert_eq!(
            key_action(KeyCode::F5, &IDLE, false),
            Some(Action::QuickSave)
        );
        assert_eq!(key_action(KeyCode::F1, &IDLE, false), None);
        assert_eq!(key_action(KeyCode::BracketLeft, &IDLE, false), None);
        assert_eq!(
            key_action(KeyCode::Equal, &EDITING, false),
            Some(Action::AdjustBrushStrength(250))
        );
        assert_eq!(
            key_action(KeyCode::Minus, &EDITING, false),
            Some(Action::AdjustBrushStrength(-250))
        );
        assert_eq!(
            key_action(KeyCode::BracketRight, &EDITING, false),
            Some(Action::AdjustBrushRadius(1))
        );
    }

    #[test]
    fn general_keys_map_to_actions() {
        for (code, action) in [
            (KeyCode::KeyT, Action::ToggleTerrainEditor),
            (KeyCode::F9, Action::QuickLoad),
            (KeyCode::F10, Action::ToggleTerrainPanel),
            (KeyCode::Escape, Action::Escape),
            (KeyCode::Digit1, Action::SelectSlot(0)),
            (KeyCode::Digit2, Action::SelectSlot(1)),
            (KeyCode::Digit3, Action::SelectSlot(2)),
            (KeyCode::Space, Action::CenterCamera),
            (KeyCode::KeyR, Action::RecruitNearest),
        ] {
            assert_eq!(key_action(code, &IDLE, false), Some(action.clone()));
            assert_eq!(key_action(code, &EDITING, false), Some(action));
        }
        assert_eq!(key_action(KeyCode::KeyW, &IDLE, false), None);
    }

    #[test]
    fn keys_panel_toggle_ignores_key_repeat() {
        assert_eq!(key_action(KeyCode::F10, &IDLE, true), None);
        assert_eq!(
            key_action(KeyCode::KeyR, &IDLE, true),
            Some(Action::RecruitNearest)
        );
    }

    #[test]
    fn availability_follows_editor_state() {
        let raise = Action::SetTerrainTool(TerrainTool::Raise);
        assert!(raise.is_available(&EDITING));
        assert!(!raise.is_available(&IDLE));
        assert!(Action::QuickSave.is_available(&IDLE));
        assert!(!Action::QuickSave.is_available(&EDITING));
        assert!(Action::QuickLoad.is_available(&EDITING));
    }

    #[test]
    fn cost_map_becomes_a_material_and_amount() {
        let mut def: content::BuildingDef =
            ron::from_str("(id: \"x\", name: \"X\", width: 1, depth: 1)").unwrap();
        assert_eq!(construction_cost(&def), None);
        def.cost.insert("iron_ore".into(), 2.0);
        assert_eq!(construction_cost(&def), Some((ResourceKind::IronOre, 2)));
        def.cost.clear();
        def.cost.insert("unknown".into(), 3.0);
        assert_eq!(construction_cost(&def), None);
    }

    #[test]
    fn toggle_adds_and_removes() {
        let mut selection = vec![Entity(1)];
        toggle_selection(&mut selection, Entity(2));
        assert_eq!(selection, vec![Entity(1), Entity(2)]);
        toggle_selection(&mut selection, Entity(1));
        assert_eq!(selection, vec![Entity(2)]);
    }

    #[test]
    fn toggle_keeps_last_member() {
        let mut selection = vec![Entity(3)];
        toggle_selection(&mut selection, Entity(3));
        assert_eq!(selection, vec![Entity(3)]);
    }
}
