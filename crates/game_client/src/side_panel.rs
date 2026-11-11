use crate::actions::{Action, PlacementPreview};
use crate::panel::{BAD, GOOD};
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
    pub actions: &'a mut Vec<Action>,
    pub placement_preview: Option<&'a PlacementPreview>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Squad,
    Build,
    Jobs,
}

impl Tab {
    const ALL: [(Tab, &'static str); 3] = [
        (Tab::Squad, "Squad"),
        (Tab::Build, "Build"),
        (Tab::Jobs, "Jobs"),
    ];
}

pub(crate) fn show(ui: &mut egui::Ui, p: &mut PanelCtx<'_>) {
    ui.heading("ASHEN WORLD");
    ui.label(format!("{} survivors", p.world.player_character_count()));
    ui.separator();
    selected_summary(ui, p);
    if let Some(preview) = p.placement_preview {
        if let Some(reason) = &preview.reason {
            ui.colored_label(BAD, format!("Invalid placement: {reason}."));
        } else {
            ui.colored_label(GOOD, "Valid placement");
        }
    }
    ui.separator();

    let tab_id = egui::Id::new("side_panel_tab");
    let mut tab = ui.data(|d| d.get_temp::<Tab>(tab_id)).unwrap_or(Tab::Squad);
    ui.horizontal(|ui| {
        for (value, title) in Tab::ALL {
            ui.selectable_value(&mut tab, value, title);
        }
    });
    ui.data_mut(|d| d.insert_temp(tab_id, tab));
    ui.separator();

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| match tab {
            Tab::Squad => {
                squad_section(ui, p);
                ui.separator();
                ui.label(egui::RichText::new("Nearby recruits").strong());
                recruit_section(ui, p);
            }
            Tab::Build => build_section(ui, p),
            Tab::Jobs => jobs_tab(ui, p),
        });
}

fn squad_section(ui: &mut egui::Ui, p: &mut PanelCtx<'_>) {
    ui.small(format!(
        "{} selected · Ctrl/Shift-click to multi-select",
        p.selected_entities.len()
    ));
    for character in p.world.player_characters() {
        let selected = p.selected_entities.contains(&character);
        let (activity, color) = activity(p.jobs, p.buildings, character);
        let label = egui::RichText::new(format!(
            "{}{}",
            if selected { "● " } else { "○ " },
            character_label(character)
        ));
        ui.horizontal(|ui| {
            let response = ui.selectable_label(selected, label);
            ui.label(egui::RichText::new(activity).small().color(color));
            if response.clicked() {
                let multi = ui.input(|i| i.modifiers.command || i.modifiers.shift);
                p.actions.push(if multi {
                    Action::ToggleSelect(character)
                } else {
                    Action::Select(character)
                });
            }
        });
    }
}

/// Short activity text and colour for a squad row, from the worker's top job.
fn activity(jobs: &Jobs, buildings: &Buildings, worker: Entity) -> (&'static str, egui::Color32) {
    use crate::panel::{BAD, GOOD, IDLE, WARN};
    if jobs.is_suspended(worker) {
        return ("ordered", WARN);
    }
    match jobs.job_status(worker, 0, buildings) {
        None if jobs.auto_build_site(worker).is_some() => ("building", GOOD),
        None => ("idle", IDLE),
        Some(JobStatus::Working) => ("working", GOOD),
        Some(JobStatus::Moving) => ("moving", WARN),
        Some(JobStatus::Hauling) => ("hauling", GOOD),
        Some(JobStatus::Queued) => ("waiting", IDLE),
        Some(JobStatus::Suspended) => ("ordered", WARN),
        Some(JobStatus::Blocked(_)) => ("blocked", BAD),
        Some(JobStatus::NoFreeSlot) => ("no slot", BAD),
        Some(JobStatus::Invalid) => ("invalid", BAD),
    }
}

/// Always-visible details for the primary selected survivor.
fn selected_summary(ui: &mut egui::Ui, p: &mut PanelCtx<'_>) {
    let Some(selected) = p.world.selected_character() else {
        ui.label("No character selected");
        return;
    };
    let position = p.world.position(selected);
    ui.label(egui::RichText::new(character_label(selected)).strong());
    ui.small(format!("Position  {:.0}, {:.0}", position.x, position.z));
    if let Some(inventory) = p.world.inventory(selected) {
        if inventory.is_empty() {
            ui.small("Inventory: empty");
        } else {
            let items = inventory
                .items()
                .iter()
                .map(|stack| format!("{} ×{}", stack.item, stack.count))
                .collect::<Vec<_>>()
                .join(", ");
            ui.small(format!("Inventory: {items}"));
        }
    }
    ui.small("Left-click open terrain to move.");
}

fn jobs_tab(ui: &mut egui::Ui, p: &mut PanelCtx<'_>) {
    let Some(selected) = p.world.selected_character() else {
        ui.small("Select a survivor to manage jobs.");
        return;
    };
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
        if ui
            .button(Action::CancelBuilding.descriptor().label)
            .clicked()
        {
            *p.selected_building = None;
            p.actions.push(Action::CancelBuilding);
        }
        ui.small("Click terrain to place. Esc cancels.");
    } else {
        ui.small("Choose a building, then click the terrain.");
    }
    if let Some(status) = p.build_status {
        ui.small(status);
    }

    ui.small("Costed buildings become sites; idle survivors fetch the materials and build them.");

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
                    .push(Action::SelectBuilding(definition.id.clone()));
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
                if ui
                    .small_button(Action::Recruit(entity).descriptor().label)
                    .clicked()
                {
                    p.actions.push(Action::Recruit(entity));
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
    actions: &mut Vec<Action>,
) {
    let list = jobs.jobs(worker);
    if list.is_empty() {
        ui.small("No jobs. Add a building below.");
    }
    if jobs.is_suspended(worker) {
        ui.small("Jobs suspended by a move order.");
        if ui.small_button("Resume jobs").clicked() {
            actions.push(Action::ResumeJobs(worker));
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
                actions.push(Action::MoveJobUp { worker, index });
            }
            if ui.small_button("▼").clicked() {
                actions.push(Action::MoveJobDown { worker, index });
            }
            if ui.small_button("✕").clicked() {
                actions.push(Action::RemoveJob { worker, index });
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
                        actions.push(Action::AddJob {
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
                        actions.push(Action::AddBuild { worker, site, from });
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
                        actions.push(Action::AddHaul { worker, from, to });
                    }
                }
            });
    }
    for (i, site) in buildings.sites.iter().enumerate() {
        let state = if site.completed.is_some() {
            "done".to_string()
        } else {
            let remaining = site.required.saturating_sub(site.delivered);
            let activity = if remaining > 0 && jobs.site_delivery_en_route(i) > 0 {
                "delivery en route"
            } else if remaining > 0 {
                "waiting for supply"
            } else if site.worked {
                "building"
            } else if site.progress > 0.0 {
                "construction paused"
            } else if site.builder.is_some() {
                "builder on the way"
            } else {
                "waiting for builder"
            };
            format!(
                "{}/{} {} delivered, {} remaining · {:.0}% built · {activity}",
                site.delivered,
                site.required,
                site.material.id(),
                remaining,
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
