use crate::ui::UiAction;
use core_sim::{
    Buildings, Entity, Job, JobStatus, Jobs, ResourceKind, WorkStatus, World, RECRUIT_MAX_DISTANCE,
};

/// Everything the side panel reads, plus the outputs it writes to.
pub(crate) struct PanelCtx<'a> {
    pub world: &'a World,
    pub selected_entities: &'a [Entity],
    pub buildings: &'a Buildings,
    pub jobs: &'a Jobs,
    pub building_defs: &'a [content::BuildingDef],
    pub build_status: Option<&'a str>,
    pub selected_building: &'a mut Option<String>,
    pub actions: &'a mut Vec<UiAction>,
}

pub(crate) fn show(ui: &mut egui::Ui, p: &mut PanelCtx<'_>) {
    ui.heading("ASHEN WORLD");
    ui.label(format!("{} survivors", p.world.player_character_count()));
    ui.separator();
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            section(ui, "squad", "Squad", true, |ui| squad_section(ui, p));
            section(ui, "selected", "Selected", true, |ui| {
                selected_section(ui, p)
            });
            section(ui, "build", "Build", false, |ui| build_section(ui, p));
            section(ui, "recruits", "Nearby recruits", false, |ui| {
                recruit_section(ui, p)
            });
        });
}

/// A collapsible panel section; egui remembers its open state per id.
fn section(
    ui: &mut egui::Ui,
    id: &str,
    title: &str,
    default_open: bool,
    body: impl FnOnce(&mut egui::Ui),
) {
    egui::CollapsingHeader::new(egui::RichText::new(title).heading())
        .id_salt(id)
        .default_open(default_open)
        .show(ui, body);
}

fn squad_section(ui: &mut egui::Ui, p: &mut PanelCtx<'_>) {
    ui.small(format!("{} selected", p.selected_entities.len()));
    for character in p.world.player_characters() {
        let selected = p.selected_entities.contains(&character);
        let label = format!(
            "{}{}",
            if selected { "● " } else { "○ " },
            character_label(character)
        );
        if ui.selectable_label(selected, label).clicked() {
            p.actions.push(UiAction::Select(character));
        }
    }
}

fn selected_section(ui: &mut egui::Ui, p: &mut PanelCtx<'_>) {
    let Some(selected) = p.world.selected_character() else {
        ui.label("No character selected");
        return;
    };
    let position = p.world.position(selected);
    ui.label(character_label(selected));
    ui.small(format!("Position  {:.0}, {:.0}", position.x, position.z));
    ui.add_space(6.0);
    ui.label("Inventory");
    if let Some(inventory) = p.world.inventory(selected) {
        if inventory.is_empty() {
            ui.small("Empty");
        } else {
            for stack in inventory.items() {
                ui.small(format!("{}  ×{}", stack.item, stack.count));
            }
        }
    }

    ui.add_space(8.0);
    ui.label("Move order");
    ui.small("Left-click open terrain to move this survivor.");

    ui.separator();
    job_panel(
        ui,
        selected,
        p.jobs,
        p.buildings,
        p.building_defs,
        p.actions,
    );
}

fn build_section(ui: &mut egui::Ui, p: &mut PanelCtx<'_>) {
    if let Some(active_id) = p.selected_building.as_deref() {
        let name = p
            .building_defs
            .iter()
            .find(|definition| definition.id == active_id)
            .map_or(active_id, |definition| definition.name.as_str());
        ui.label(format!("Placing: {name}"));
        if ui.button("Cancel placement").clicked() {
            *p.selected_building = None;
            p.actions.push(UiAction::CancelBuilding);
        }
        ui.small("Click terrain to place. Esc cancels.");
    } else {
        ui.small("Choose a building, then click the terrain.");
    }
    if let Some(status) = p.build_status {
        ui.small(status);
    }

    ui.small("Costs are not charged in this prototype.");

    let categories = ["Mines", "Farms", "Housing", "Industry", "Storage"];
    for category in categories {
        let entries: Vec<_> = p
            .building_defs
            .iter()
            .filter(|definition| matches_category(definition, category))
            .collect();
        if entries.is_empty() {
            continue;
        }
        ui.label(egui::RichText::new(category).strong());
        for definition in entries {
            let selected = p.selected_building.as_deref() == Some(&definition.id);
            let label = format!(
                "{}  {} × {}",
                definition.name, definition.width, definition.depth
            );
            let response = ui
                .selectable_label(selected, label)
                .on_hover_ui(|ui| building_details(ui, definition));
            if response.clicked() {
                *p.selected_building = Some(definition.id.clone());
                p.actions
                    .push(UiAction::SelectBuilding(definition.id.clone()));
            }
            if selected {
                ui.indent(("build_details", &definition.id), |ui| {
                    building_details(ui, definition);
                });
            }
        }
    }
}

/// Cost, output, storage and placement requirements for one building.
fn building_details(ui: &mut egui::Ui, definition: &content::BuildingDef) {
    if !definition.cost.is_empty() {
        let cost = definition
            .cost
            .iter()
            .map(|(item, amount)| format!("{item} {amount:.0}"))
            .collect::<Vec<_>>()
            .join(", ");
        ui.small(format!("Cost: {cost}"));
    }
    if !definition.output_resources.is_empty() {
        ui.small(format!(
            "Output: {}",
            definition.output_resources.join(", ")
        ));
    }
    if definition.storage {
        ui.small(format!(
            "Stores: {} per resource, stack limit {}",
            definition.input_slots, definition.stack_limit
        ));
    }
    if let Some(resource) = &definition.requires_resource {
        if definition.min_resource_richness.is_some() {
            ui.small(format!("{} richness must be above zero.", resource));
        } else {
            ui.small(format!("Requires {resource} in its footprint."));
        }
    }
    if definition.min_fertility.is_some() {
        ui.small("Soil fertility must be above zero.");
    }
}

fn recruit_section(ui: &mut egui::Ui, p: &mut PanelCtx<'_>) {
    let world = p.world;
    let mut candidates: Vec<(Entity, f32)> = world
        .entities()
        .filter(|&entity| world.is_recruitable(entity))
        .filter_map(|entity| {
            let selected = world.selected_character()?;
            if !world.can_recruit_with(selected, entity) {
                return None;
            }
            let a = world.position(selected);
            let b = world.position(entity);
            let distance = ((a.x - b.x).powi(2) + (a.z - b.z).powi(2)).sqrt();
            (distance <= RECRUIT_MAX_DISTANCE).then_some((entity, distance))
        })
        .collect();
    candidates.sort_by(|a, b| a.1.total_cmp(&b.1));
    if candidates.is_empty() {
        ui.small("No eligible characters in interaction range.");
    } else {
        for (entity, distance) in candidates.into_iter().take(8) {
            ui.horizontal(|ui| {
                ui.label(format!("{}  {:.1} m", character_label(entity), distance));
                if ui.small_button("Recruit").clicked() {
                    p.actions.push(UiAction::Recruit(entity));
                }
            });
        }
    }
}

fn character_label(entity: Entity) -> String {
    format!("Survivor {:02}", entity.0 + 1)
}

fn matches_category(definition: &content::BuildingDef, category: &str) -> bool {
    match category {
        "Mines" => definition.requires_resource.is_some(),
        "Farms" => definition.crop.is_some(),
        "Housing" => definition.dwelling,
        "Industry" => {
            definition.requires_resource.is_none()
                && definition.crop.is_none()
                && !definition.dwelling
                && !definition.storage
        }
        "Storage" => definition.storage,
        _ => false,
    }
}

fn building_label(buildings: &Buildings, defs: &[content::BuildingDef], index: usize) -> String {
    let Some(building) = buildings.buildings.get(index) else {
        return format!("#{index} (removed)");
    };
    let name = defs
        .iter()
        .find(|definition| definition.id == building.def)
        .map_or(building.def.as_str(), |definition| definition.name.as_str());
    format!("{name} #{index}")
}

fn status_text(status: JobStatus) -> &'static str {
    match status {
        JobStatus::Working => "working",
        JobStatus::Moving => "moving to slot",
        JobStatus::Blocked(WorkStatus::OutputFull) => "blocked: output full",
        JobStatus::Blocked(WorkStatus::MissingInput) => "blocked: missing input",
        JobStatus::Blocked(WorkStatus::Ready) => "ready",
        JobStatus::NoFreeSlot => "no free worker slot",
        JobStatus::Queued => "waiting (higher priority first)",
        JobStatus::Suspended => "suspended",
        JobStatus::Hauling => "hauling / building",
        JobStatus::Invalid => "building missing",
    }
}

/// Job panel: the worker's prioritized jobs (top = highest priority).
fn job_panel(
    ui: &mut egui::Ui,
    worker: Entity,
    jobs: &Jobs,
    buildings: &Buildings,
    defs: &[content::BuildingDef],
    actions: &mut Vec<UiAction>,
) {
    ui.heading("Jobs");
    let list = jobs.jobs(worker);
    if list.is_empty() {
        ui.small("No jobs. Add a building below.");
    }
    if jobs.is_suspended(worker) {
        ui.small("Jobs suspended by a move order.");
        if ui.small_button("Resume jobs").clicked() {
            actions.push(UiAction::ResumeJobs(worker));
        }
    }
    for (index, job) in list.iter().enumerate() {
        let label = match *job {
            Job::Work { building } => building_label(buildings, defs, building),
            Job::Haul { from, to, item } => format!(
                "Haul {}: {} -> {}",
                item.id(),
                building_label(buildings, defs, from),
                building_label(buildings, defs, to)
            ),
            Job::Build { site, from } => format!(
                "Build {}: {} -> site #{site}",
                buildings.sites.get(site).map_or("?", |s| s.def.as_str()),
                building_label(buildings, defs, from)
            ),
        };
        ui.horizontal(|ui| {
            ui.label(format!("{}. {}", index + 1, label));
            if let Some(status) = jobs.job_status(worker, index, buildings) {
                ui.small(status_text(status));
            }
            if ui.small_button("▲").clicked() {
                actions.push(UiAction::MoveJobUp { worker, index });
            }
            if ui.small_button("▼").clicked() {
                actions.push(UiAction::MoveJobDown { worker, index });
            }
            if ui.small_button("✕").clicked() {
                actions.push(UiAction::RemoveJob { worker, index });
            }
        });
    }
    let candidates: Vec<usize> = (0..buildings.buildings.len())
        .filter(|&i| {
            let b = &buildings.buildings[i];
            !b.workers.is_empty() && !list.contains(&Job::Work { building: i })
        })
        .collect();
    if !candidates.is_empty() {
        egui::ComboBox::from_id_salt("add_job")
            .selected_text("Add job…")
            .show_ui(ui, |ui| {
                for i in candidates {
                    if ui
                        .selectable_label(false, building_label(buildings, defs, i))
                        .clicked()
                    {
                        actions.push(UiAction::AddJob {
                            worker,
                            building: i,
                        });
                    }
                }
            });
    }
    let builds: Vec<(usize, usize)> = (0..buildings.sites.len())
        .filter(|&site| buildings.sites[site].completed.is_none())
        .flat_map(|site| (0..buildings.buildings.len()).map(move |from| (site, from)))
        .filter(|&(site, from)| {
            buildings.buildings[from]
                .outputs
                .allows(buildings.sites[site].material.id())
                && !list.contains(&Job::Build { site, from })
        })
        .collect();
    if !builds.is_empty() {
        egui::ComboBox::from_id_salt("add_build")
            .selected_text("Add build job…")
            .show_ui(ui, |ui| {
                for (site, from) in builds {
                    let label = format!(
                        "{} site #{site} from {}",
                        buildings.sites[site].def,
                        building_label(buildings, defs, from)
                    );
                    if ui.selectable_label(false, label).clicked() {
                        actions.push(UiAction::AddBuild { worker, site, from });
                    }
                }
            });
    }
    let ore = ResourceKind::IronOre.id();
    let hauls: Vec<(usize, usize)> = (0..buildings.buildings.len())
        .flat_map(|from| (0..buildings.buildings.len()).map(move |to| (from, to)))
        .filter(|&(from, to)| {
            from != to
                && buildings.buildings[from].outputs.allows(ore)
                && buildings.buildings[to].inputs.allows(ore)
                && !list.contains(&Job::Haul {
                    from,
                    to,
                    item: ResourceKind::IronOre,
                })
        })
        .collect();
    if !hauls.is_empty() {
        egui::ComboBox::from_id_salt("add_haul")
            .selected_text("Add haul iron ore…")
            .show_ui(ui, |ui| {
                for (from, to) in hauls {
                    let label = format!(
                        "{} -> {}",
                        building_label(buildings, defs, from),
                        building_label(buildings, defs, to)
                    );
                    if ui.selectable_label(false, label).clicked() {
                        actions.push(UiAction::AddHaul { worker, from, to });
                    }
                }
            });
    }
    for (i, site) in buildings.sites.iter().enumerate() {
        let state = if site.completed.is_some() {
            "done".to_string()
        } else {
            format!(
                "{}/{} {}, {:.0}% built",
                site.delivered,
                site.required,
                site.material.id(),
                100.0 * site.progress / core_sim::BUILD_SECONDS
            )
        };
        ui.small(format!("Site #{i} {}: {state}", site.def));
    }
    for (i, b) in buildings.buildings.iter().enumerate() {
        let stock = |inv: &core_sim::SlotInventory| {
            inv.items()
                .map(|s| format!("{} {}", s.item, s.count))
                .collect::<Vec<_>>()
                .join(", ")
        };
        if b.inputs.items().next().is_some() || b.outputs.items().next().is_some() {
            ui.small(format!(
                "{}: in [{}] out [{}]",
                building_label(buildings, defs, i),
                stock(&b.inputs),
                stock(&b.outputs)
            ));
        }
    }
}
