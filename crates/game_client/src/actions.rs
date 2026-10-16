//! Executes player intents (`UiAction`s from the HUD and context menu) against the `App` state.

use core_sim::{BuildingLogistics, Footprint, Job, PlacementRules, ResourceKind, WorldPos};

use crate::ui::UiAction;
use crate::App;

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
    pub(super) fn apply_ui_action(&mut self, action: UiAction) {
        match action {
            UiAction::Select(entity) => {
                let _ = self.world.select_character(entity);
                self.selected_entities.clear();
                self.selected_entities.push(entity);
            }
            UiAction::ToggleSelect(entity) => {
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
            UiAction::ToggleTerrainEditor => self.toggle_terrain_editor(),
            UiAction::SetTerrainTool(tool) => {
                if self.editor.active {
                    self.editor.set_tool(tool);
                    self.build_status = Some(self.editor.status());
                }
            }
            UiAction::ToggleTerrainPanel => {
                if let Some(ui) = &mut self.ui {
                    ui.toggle_terrain_panel();
                }
            }
            UiAction::QuickSave => self.quick_save(),
            UiAction::QuickLoad => self.quick_load(),
            UiAction::AdjustBrushRadius(step) => {
                if self.editor.active {
                    self.editor.adjust_radius(step);
                    self.build_status = Some(self.editor.status());
                }
            }
            UiAction::AdjustBrushStrength(step_mm) => {
                if self.editor.active {
                    self.editor.adjust_strength(step_mm);
                    self.build_status = Some(self.editor.status());
                }
            }
            UiAction::CancelBuilding => {
                self.build_mode = None;
                self.build_status = None;
                if let Some(ui) = &mut self.ui {
                    ui.set_selected_building(None);
                }
            }
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
    use super::{construction_cost, toggle_selection};
    use core_sim::{Entity, ResourceKind};

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
