use core_sim::{
    fertility_at, resource_richness_at, Buildings, Entity, Job, JobStatus, Jobs, ResourceKind,
    WorkStatus, World, RECRUIT_MAX_DISTANCE,
};
use egui::{ClippedPrimitive, Color32, Id, LayerId, Order, Pos2, Stroke, TexturesDelta};
use glam::{Mat4, Vec3};
use std::sync::Arc;
use winit::event::WindowEvent;
use winit::window::Window;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiAction {
    Select(Entity),
    Recruit(Entity),
    SelectBuilding(String),
    CancelBuilding,
    AddJob {
        worker: Entity,
        building: usize,
    },
    AddBuild {
        worker: Entity,
        site: usize,
        from: usize,
    },
    AddHaul {
        worker: Entity,
        from: usize,
        to: usize,
    },
    RemoveJob {
        worker: Entity,
        index: usize,
    },
    MoveJobUp {
        worker: Entity,
        index: usize,
    },
    MoveJobDown {
        worker: Entity,
        index: usize,
    },
    ResumeJobs(Entity),
}

pub struct HudData<'a> {
    pub world: &'a World,
    pub selected_entities: &'a [Entity],
    pub buildings: &'a Buildings,
    pub jobs: &'a Jobs,
    pub building_defs: &'a [content::BuildingDef],
    pub build_status: Option<&'a str>,
    pub seed: u64,
    pub camera: &'a crate::camera::Camera,
    pub selection_rect: Option<[f32; 4]>,
}

pub struct GameUi {
    context: egui::Context,
    state: egui_winit::State,
    renderer: egui_wgpu::Renderer,
    paint_jobs: Vec<ClippedPrimitive>,
    textures_delta: TexturesDelta,
    selected_building: Option<String>,
}

impl GameUi {
    pub fn new(window: &Arc<Window>, device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let context = egui::Context::default();
        let state = egui_winit::State::new(
            context.clone(),
            egui::ViewportId::ROOT,
            window.as_ref(),
            Some(window.scale_factor() as f32),
            window.theme(),
            None,
        );
        let renderer = egui_wgpu::Renderer::new(device, format, None, 1, false);
        apply_theme(&context);
        Self {
            context,
            state,
            renderer,
            paint_jobs: Vec::new(),
            textures_delta: TexturesDelta::default(),
            selected_building: None,
        }
    }

    pub fn on_window_event(&mut self, window: &Window, event: &WindowEvent) -> bool {
        self.state.on_window_event(window, event).consumed
    }

    pub fn run(&mut self, window: &Window, data: HudData<'_>) -> Vec<UiAction> {
        let HudData {
            world,
            selected_entities,
            buildings,
            jobs,
            building_defs,
            build_status,
            seed,
            camera,
            selection_rect,
        } = data;
        let input = self.state.take_egui_input(window);
        let mut actions = Vec::new();
        let selected_building = &mut self.selected_building;
        let output = self.context.run(input, |ctx| {
            let size = window.inner_size();
            let scale = window.scale_factor() as f32;
            let screen_width = size.width as f32 / scale;
            let screen_height = size.height as f32 / scale;
            let painter =
                ctx.layer_painter(LayerId::new(Order::Middle, Id::new("world_buildings")));
            if let Some([x0, y0, x1, y1]) = selection_rect {
                let selection = egui::Rect::from_two_pos(
                    Pos2::new(x0 / scale, y0 / scale),
                    Pos2::new(x1 / scale, y1 / scale),
                );
                ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("selection_box")))
                    .rect(
                        selection,
                        0.0,
                        Color32::from_rgba_unmultiplied(70, 180, 130, 35),
                        Stroke::new(1.5_f32, Color32::from_rgb(112, 224, 174)),
                    );
            }
            let view_proj = camera.view_proj();
            if let Some(definition) = selected_building
                .as_deref()
                .and_then(|id| building_defs.iter().find(|definition| definition.id == id))
            {
                if definition.min_resource_richness.is_some() || definition.min_fertility.is_some()
                {
                    SiteQualityOverlay {
                        painter: &painter,
                        view_proj,
                        center: (camera.target.x, camera.target.z),
                        radius: camera.distance * 0.8,
                        seed,
                        edits: world.terrain_edits(),
                        definition,
                        screen_size: (screen_width, screen_height),
                    }
                    .draw();
                }
            }
            for placed in &buildings.buildings {
                let fp = placed.footprint;
                let corners = [
                    (fp.x, fp.z),
                    (fp.x + fp.width, fp.z),
                    (fp.x + fp.width, fp.z + fp.depth),
                    (fp.x, fp.z + fp.depth),
                ];
                let points: Option<Vec<Pos2>> = corners
                    .into_iter()
                    .map(|(x, z)| {
                        let position = Vec3::new(
                            x as f32,
                            world.terrain_edits().height(seed, x as f32, z as f32) + 0.3,
                            z as f32,
                        );
                        project_to_screen(view_proj, position, screen_width, screen_height)
                    })
                    .collect();
                if let Some(points) = points {
                    painter.add(egui::Shape::closed_line(
                        points,
                        Stroke::new(2.0_f32, Color32::from_rgb(225, 190, 120)),
                    ));
                }
            }

            for site in buildings.sites.iter().filter(|s| s.completed.is_none()) {
                let fp = site.footprint;
                let corners = [
                    (fp.x, fp.z),
                    (fp.x + fp.width, fp.z),
                    (fp.x + fp.width, fp.z + fp.depth),
                    (fp.x, fp.z + fp.depth),
                ];
                let points: Option<Vec<Pos2>> = corners
                    .into_iter()
                    .map(|(x, z)| {
                        let position = Vec3::new(
                            x as f32,
                            world.terrain_edits().height(seed, x as f32, z as f32) + 0.3,
                            z as f32,
                        );
                        project_to_screen(view_proj, position, screen_width, screen_height)
                    })
                    .collect();
                if let Some(points) = points {
                    painter.add(egui::Shape::closed_line(
                        points,
                        Stroke::new(2.0_f32, Color32::from_rgb(110, 170, 225)),
                    ));
                }
            }

            egui::SidePanel::right("game_hud")
                .frame(panel_frame())
                .default_width(270.0)
                .min_width(230.0)
                .resizable(true)
                .show(ctx, |ui| {
                    ui.heading("ASHEN WORLD");
                    ui.label(format!("{} survivors", world.player_character_count()));
                    ui.separator();

                    ui.heading("Squad");
                    ui.small(format!("{} selected", selected_entities.len()));
                    egui::ScrollArea::vertical()
                        .max_height(180.0)
                        .show(ui, |ui| {
                            for character in world.player_characters() {
                                let selected = selected_entities.contains(&character);
                                let label = format!(
                                    "{}{}",
                                    if selected { "● " } else { "○ " },
                                    character_label(character)
                                );
                                if ui.selectable_label(selected, label).clicked() {
                                    actions.push(UiAction::Select(character));
                                }
                            }
                        });

                    ui.separator();
                    ui.heading("Selected");
                    if let Some(selected) = world.selected_character() {
                        let position = world.position(selected);
                        ui.label(character_label(selected));
                        ui.small(format!("Position  {:.0}, {:.0}", position.x, position.z));
                        ui.add_space(6.0);
                        ui.label("Inventory");
                        if let Some(inventory) = world.inventory(selected) {
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
                        job_panel(ui, selected, jobs, buildings, building_defs, &mut actions);
                    } else {
                        ui.label("No character selected");
                    }

                    ui.separator();
                    ui.heading("Build");
                    if let Some(active_id) = selected_building.as_deref() {
                        let name = building_defs
                            .iter()
                            .find(|definition| definition.id == active_id)
                            .map_or(active_id, |definition| definition.name.as_str());
                        ui.label(format!("Placing: {name}"));
                        if ui.button("Cancel placement").clicked() {
                            *selected_building = None;
                            actions.push(UiAction::CancelBuilding);
                        }
                        ui.small("Click terrain to place. Esc cancels.");
                    } else {
                        ui.small("Choose a building, then click the terrain.");
                    }
                    if let Some(status) = build_status {
                        ui.small(status);
                    }

                    let categories = ["Mines", "Farms", "Housing", "Industry", "Storage"];
                    for category in categories {
                        let entries: Vec<_> = building_defs
                            .iter()
                            .filter(|definition| matches_category(definition, category))
                            .collect();
                        if entries.is_empty() {
                            continue;
                        }
                        ui.label(category);
                        for definition in entries {
                            let selected = selected_building.as_deref() == Some(&definition.id);
                            let label = format!(
                                "{}  {} × {}",
                                definition.name, definition.width, definition.depth
                            );
                            if ui.selectable_label(selected, label).clicked() {
                                *selected_building = Some(definition.id.clone());
                                actions.push(UiAction::SelectBuilding(definition.id.clone()));
                            }
                            if !definition.cost.is_empty() {
                                let cost = definition
                                    .cost
                                    .iter()
                                    .map(|(item, amount)| format!("{item} {amount:.0}"))
                                    .collect::<Vec<_>>()
                                    .join(", ");
                                ui.small(format!("Cost: {cost} (not charged in prototype)"));
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
                    }

                    ui.separator();
                    ui.heading("Nearby recruits");
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
                        egui::ScrollArea::vertical()
                            .max_height(150.0)
                            .show(ui, |ui| {
                                for (entity, distance) in candidates.into_iter().take(8) {
                                    ui.horizontal(|ui| {
                                        ui.label(format!(
                                            "{}  {:.1} m",
                                            character_label(entity),
                                            distance
                                        ));
                                        if ui.small_button("Recruit").clicked() {
                                            actions.push(UiAction::Recruit(entity));
                                        }
                                    });
                                }
                            });
                    }
                });
        });
        self.state
            .handle_platform_output(window, output.platform_output);
        self.paint_jobs = self
            .context
            .tessellate(output.shapes, output.pixels_per_point);
        self.textures_delta.append(output.textures_delta);
        actions
    }

    pub fn set_selected_building(&mut self, id: Option<String>) {
        self.selected_building = id;
    }

    pub fn paint(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        size: [u32; 2],
        pixels_per_point: f32,
    ) -> Vec<wgpu::CommandBuffer> {
        for (id, delta) in &self.textures_delta.set {
            self.renderer.update_texture(device, queue, *id, delta);
        }
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: size,
            pixels_per_point,
        };
        let command_buffers =
            self.renderer
                .update_buffers(device, queue, encoder, &self.paint_jobs, &screen);
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            self.renderer
                .render(&mut pass.forget_lifetime(), &self.paint_jobs, &screen);
        }
        for id in &self.textures_delta.free {
            self.renderer.free_texture(id);
        }
        self.textures_delta = TexturesDelta::default();
        command_buffers
    }
}

const PANEL_FILL: Color32 = Color32::from_rgba_premultiplied(22, 24, 28, 235);
const ACCENT: Color32 = Color32::from_rgb(225, 190, 120);
const DIVIDER: Color32 = Color32::from_rgb(62, 66, 74);

/// Dark, framed panel style with an amber accent line on the inner edge.
fn panel_frame() -> egui::Frame {
    egui::Frame::none()
        .fill(PANEL_FILL)
        .stroke(Stroke::new(1.0_f32, ACCENT))
        .inner_margin(egui::Margin::same(12.0))
        .shadow(egui::epaint::Shadow {
            offset: egui::vec2(-2.0, 0.0),
            blur: 12.0,
            spread: 0.0,
            color: Color32::from_black_alpha(120),
        })
}

/// Applies the shared HUD theme: spacing, typography, rounded widgets and
/// distinct hover/selected/disabled states. Uses only built-in egui primitives.
fn apply_theme(context: &egui::Context) {
    context.style_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(8.0, 3.0);
        style
            .text_styles
            .insert(egui::TextStyle::Heading, egui::FontId::proportional(17.0));
        let visuals = &mut style.visuals;
        visuals.override_text_color = Some(Color32::from_rgb(222, 224, 228));
        visuals.window_fill = PANEL_FILL;
        visuals.panel_fill = PANEL_FILL;
        visuals.selection.bg_fill = Color32::from_rgba_unmultiplied(225, 190, 120, 70);
        visuals.selection.stroke = Stroke::new(1.0_f32, ACCENT);
        visuals.hyperlink_color = ACCENT;
        visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, DIVIDER);
        visuals.widgets.noninteractive.fg_stroke.color = Color32::from_rgb(200, 203, 210);
        let radius = egui::Rounding::same(4.0);
        visuals.widgets.inactive.bg_fill = Color32::from_rgb(40, 44, 52);
        visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(40, 44, 52);
        visuals.widgets.hovered.bg_fill = Color32::from_rgb(58, 64, 76);
        visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(58, 64, 76);
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT);
        visuals.widgets.active.bg_fill = Color32::from_rgb(80, 72, 52);
        visuals.widgets.active.weak_bg_fill = Color32::from_rgb(80, 72, 52);
        for widget in [
            &mut visuals.widgets.noninteractive,
            &mut visuals.widgets.inactive,
            &mut visuals.widgets.hovered,
            &mut visuals.widgets.active,
        ] {
            widget.rounding = radius;
        }
    });
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

fn project_to_screen(view_proj: Mat4, position: Vec3, width: f32, height: f32) -> Option<Pos2> {
    let clip = view_proj * position.extend(1.0);
    if clip.w <= 0.0 {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    Some(Pos2::new(
        (ndc.x + 1.0) * 0.5 * width,
        (1.0 - ndc.y) * 0.5 * height,
    ))
}

struct SiteQualityOverlay<'a> {
    painter: &'a egui::Painter,
    view_proj: Mat4,
    center: (f32, f32),
    radius: f32,
    seed: u64,
    edits: &'a core_sim::TerrainEdits,
    definition: &'a content::BuildingDef,
    screen_size: (f32, f32),
}

impl SiteQualityOverlay<'_> {
    fn draw(self) {
        const CELL_SIZE: i32 = 12;
        let radius = self.radius.ceil() as i32;
        let center_x = self.center.0.floor() as i32;
        let center_z = self.center.1.floor() as i32;
        let start_x = (center_x - radius).div_euclid(CELL_SIZE) * CELL_SIZE;
        let end_x = (center_x + radius).div_euclid(CELL_SIZE) * CELL_SIZE;
        let start_z = (center_z - radius).div_euclid(CELL_SIZE) * CELL_SIZE;
        let end_z = (center_z + radius).div_euclid(CELL_SIZE) * CELL_SIZE;

        for z in (start_z..=end_z).step_by(CELL_SIZE as usize) {
            for x in (start_x..=end_x).step_by(CELL_SIZE as usize) {
                let quality = if let Some(resource) = &self.definition.requires_resource {
                    resource_richness_at(self.seed, resource, x as f32 + 6.0, z as f32 + 6.0)
                } else {
                    fertility_at(self.seed, x as f32 + 6.0, z as f32 + 6.0)
                };
                let alpha = (20.0 + quality * 85.0) as u8;
                let fill = if self.definition.requires_resource.is_some() {
                    Color32::from_rgba_unmultiplied(174, 86, 46, alpha)
                } else {
                    Color32::from_rgba_unmultiplied(74, 158, 83, alpha)
                };
                let corners = [
                    (x, z),
                    (x + CELL_SIZE, z),
                    (x + CELL_SIZE, z + CELL_SIZE),
                    (x, z + CELL_SIZE),
                ];
                let points: Option<Vec<Pos2>> = corners
                    .into_iter()
                    .map(|(cx, cz)| {
                        let position = Vec3::new(
                            cx as f32,
                            self.edits.height(self.seed, cx as f32, cz as f32) + 0.12,
                            cz as f32,
                        );
                        project_to_screen(
                            self.view_proj,
                            position,
                            self.screen_size.0,
                            self.screen_size.1,
                        )
                    })
                    .collect();
                if let Some(points) = points {
                    self.painter
                        .add(egui::Shape::convex_polygon(points, fill, Stroke::NONE));
                }
            }
        }
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
