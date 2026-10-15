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
            UiAction::CancelBuilding => {
                self.build_mode = None;
                self.build_status = None;
                if let Some(ui) = &mut self.ui {
                    ui.set_selected_building(None);
                }
            }
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

    /// Debug construction request: reserve a site for `definition`, keep a wood store stocked
    /// for it (wood is not produced anywhere yet), and put the selected worker on the build job.
    pub(super) fn request_construction(
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
}

#[cfg(test)]
mod tests {
    use super::toggle_selection;
    use core_sim::Entity;

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
