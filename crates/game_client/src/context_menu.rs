//! Maps a right-clicked world target plus the selected worker to the actions the
//! context menu offers. Pure data: the UI renders entries and `main` executes the actions.
//!
//! Availability here is a UX hint only: the simulation (`Jobs`, `Buildings`) remains the
//! authority and re-validates every job when it runs.

use core_sim::{Buildings, Entity, Job, Jobs, ResourceKind};

use crate::ui::UiAction;

/// What the player right-clicked in the world.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ContextTarget {
    Building(usize),
    Site(usize),
    Ground { x: f32, z: f32 },
}

/// One selectable menu row. `action: None` just closes the menu.
#[derive(Debug, Clone, PartialEq)]
pub struct ContextEntry {
    pub label: String,
    pub action: Option<UiAction>,
    /// Hotkey shown next to the label when the action has a keyboard equivalent.
    pub hotkey: Option<&'static str>,
}

// Canonical labels for fixed context-menu rows; dynamic rows are built in `available_actions`.
const LABEL_MOVE_HERE: &str = "Move here";
const LABEL_INSPECT: &str = "Inspect";
const LABEL_CLOSE: &str = "Close";
const HOTKEY_CLOSE: &str = "Esc";

fn entry(label: impl Into<String>, action: UiAction) -> ContextEntry {
    ContextEntry {
        label: label.into(),
        action: Some(action),
        hotkey: None,
    }
}

fn building_name(buildings: &Buildings, defs: &[content::BuildingDef], index: usize) -> String {
    let Some(building) = buildings.buildings.get(index) else {
        return format!("#{index}");
    };
    let name = defs
        .iter()
        .find(|definition| definition.id == building.def)
        .map_or(building.def.as_str(), |definition| definition.name.as_str());
    format!("{name} #{index}")
}

/// Title shown at the top of the menu for `target`.
pub fn title(
    target: ContextTarget,
    buildings: &Buildings,
    defs: &[content::BuildingDef],
) -> String {
    match target {
        ContextTarget::Building(index) => building_name(buildings, defs, index),
        ContextTarget::Site(site) => format!(
            "{} site #{site}",
            buildings.sites.get(site).map_or("?", |s| s.def.as_str())
        ),
        ContextTarget::Ground { .. } => "Ground".to_string(),
    }
}

/// Actions valid for `worker` on `target`. Actions that would be invalid or duplicate an
/// existing job are omitted; a trailing "Close" entry is always present.
pub fn available_actions(
    target: ContextTarget,
    worker: Entity,
    jobs: &Jobs,
    buildings: &Buildings,
    defs: &[content::BuildingDef],
) -> Vec<ContextEntry> {
    let list = jobs.jobs(worker);
    let ore = ResourceKind::IronOre;
    let mut entries = Vec::new();
    match target {
        ContextTarget::Building(index) => {
            let Some(building) = buildings.buildings.get(index) else {
                return vec![close_entry()];
            };
            if !building.workers.is_empty() && !list.contains(&Job::Work { building: index }) {
                let verb = if building.mine.is_some() {
                    "Mine"
                } else if building.def.contains("farm") {
                    "Farm"
                } else {
                    "Work here"
                };
                entries.push(entry(
                    verb,
                    UiAction::AddJob {
                        worker,
                        building: index,
                    },
                ));
            }
            if building.outputs.allows(ore.id()) {
                for to in 0..buildings.buildings.len() {
                    let job = Job::Haul {
                        from: index,
                        to,
                        item: ore,
                    };
                    if to != index
                        && buildings.buildings[to].inputs.allows(ore.id())
                        && !list.contains(&job)
                    {
                        entries.push(entry(
                            format!("Haul iron ore to {}", building_name(buildings, defs, to)),
                            UiAction::AddHaul {
                                worker,
                                from: index,
                                to,
                            },
                        ));
                    }
                }
            }
            if building.inputs.allows(ore.id()) {
                for from in 0..buildings.buildings.len() {
                    let job = Job::Haul {
                        from,
                        to: index,
                        item: ore,
                    };
                    if from != index
                        && buildings.buildings[from].outputs.allows(ore.id())
                        && !list.contains(&job)
                    {
                        entries.push(entry(
                            format!(
                                "Haul iron ore from {}",
                                building_name(buildings, defs, from)
                            ),
                            UiAction::AddHaul {
                                worker,
                                from,
                                to: index,
                            },
                        ));
                    }
                }
            }
            for (site, s) in buildings.sites.iter().enumerate() {
                if s.completed.is_none()
                    && building.outputs.allows(s.material.id())
                    && !list.contains(&Job::Build { site, from: index })
                {
                    entries.push(entry(
                        format!("Build {} site #{site}", s.def),
                        UiAction::AddBuild {
                            worker,
                            site,
                            from: index,
                        },
                    ));
                }
            }
            entries.push(entry(LABEL_INSPECT, UiAction::InspectBuilding(index)));
        }
        ContextTarget::Site(site) => {
            if let Some(s) = buildings.sites.get(site).filter(|s| s.completed.is_none()) {
                for from in 0..buildings.buildings.len() {
                    if buildings.buildings[from].outputs.allows(s.material.id())
                        && !list.contains(&Job::Build { site, from })
                    {
                        entries.push(entry(
                            format!("Build from {}", building_name(buildings, defs, from)),
                            UiAction::AddBuild { worker, site, from },
                        ));
                    }
                }
            }
        }
        ContextTarget::Ground { x, z } => {
            entries.push(entry(LABEL_MOVE_HERE, UiAction::MoveSelected { x, z }));
        }
    }
    entries.push(close_entry());
    entries
}

fn close_entry() -> ContextEntry {
    ContextEntry {
        label: LABEL_CLOSE.to_string(),
        action: None,
        hotkey: Some(HOTKEY_CLOSE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_sim::{Building, Footprint, SlotInventory};

    fn building(def: &str, workers: usize, mine: bool, ore_out: bool, ore_in: bool) -> Building {
        let ore = || vec![ResourceKind::IronOre.id().to_string()];
        Building {
            def: def.to_string(),
            footprint: Footprint {
                x: 0,
                z: 0,
                width: 2,
                depth: 2,
            },
            planted_tick: None,
            workers: vec![None; workers],
            inputs: SlotInventory::new(
                1,
                10,
                if ore_in {
                    ore()
                } else {
                    vec!["wheat".to_string()]
                },
            ),
            outputs: SlotInventory::new(
                1,
                10,
                if ore_out {
                    ore()
                } else {
                    vec!["wheat".to_string()]
                },
            ),
            is_storage: false,
            mine: mine.then(Default::default),
            smelt: None,
            required_inputs: Vec::new(),
            worked: false,
        }
    }

    fn labels(entries: &[ContextEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.label.as_str()).collect()
    }

    #[test]
    fn building_offers_only_valid_actions() {
        let mut buildings = Buildings::default();
        buildings
            .buildings
            .push(building("iron_mine", 1, true, true, false));
        buildings
            .buildings
            .push(building("ore_storage", 0, false, true, true));
        buildings
            .buildings
            .push(building("farm", 1, false, false, false));
        let mut jobs = Jobs::default();
        let worker = Entity(0);

        let mine = available_actions(ContextTarget::Building(0), worker, &jobs, &buildings, &[]);
        assert_eq!(
            labels(&mine),
            [
                "Mine",
                "Haul iron ore to ore_storage #1",
                "Inspect",
                "Close"
            ]
        );
        let farm = available_actions(ContextTarget::Building(2), worker, &jobs, &buildings, &[]);
        assert_eq!(labels(&farm), ["Farm", "Inspect", "Close"]);
        let store = available_actions(ContextTarget::Building(1), worker, &jobs, &buildings, &[]);
        assert_eq!(
            labels(&store),
            ["Haul iron ore from iron_mine #0", "Inspect", "Close"]
        );

        jobs.add_job(worker, Job::Work { building: 0 });
        let mine = available_actions(ContextTarget::Building(0), worker, &jobs, &buildings, &[]);
        assert!(!labels(&mine).contains(&"Mine"));
    }

    #[test]
    fn entries_have_labels_and_no_duplicate_actions() {
        let mut buildings = Buildings::default();
        buildings
            .buildings
            .push(building("iron_mine", 1, true, true, false));
        buildings
            .buildings
            .push(building("ore_storage", 0, false, true, true));
        for target in [
            ContextTarget::Building(0),
            ContextTarget::Building(1),
            ContextTarget::Building(9),
            ContextTarget::Site(0),
            ContextTarget::Ground { x: 0.0, z: 0.0 },
        ] {
            let entries = available_actions(target, Entity(0), &Jobs::default(), &buildings, &[]);
            assert_eq!(entries.last().unwrap().action, None);
            assert_eq!(entries.last().unwrap().hotkey, Some("Esc"));
            for (i, e) in entries.iter().enumerate() {
                assert!(!e.label.trim().is_empty());
                assert!(entries[..i].iter().all(|o| o.label != e.label));
                if let Some(action) = &e.action {
                    assert!(entries[..i]
                        .iter()
                        .all(|o| o.action.as_ref() != Some(action)));
                }
            }
        }
    }

    #[test]
    fn ground_offers_move_and_close() {
        let entries = available_actions(
            ContextTarget::Ground { x: 1.0, z: 2.0 },
            Entity(0),
            &Jobs::default(),
            &Buildings::default(),
            &[],
        );
        assert_eq!(labels(&entries), ["Move here", "Close"]);
        assert_eq!(
            entries[0].action,
            Some(UiAction::MoveSelected { x: 1.0, z: 2.0 })
        );
    }
}
