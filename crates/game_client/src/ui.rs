use crate::context_menu::ContextEntry;
use crate::panel::{
    key_row, panel_frame, popover_frame, section_header, state_line, status_line, MovablePanel,
    ACCENT, DIVIDER, PANEL_FILL,
};
use core_sim::{fertility_at, resource_richness_at, Buildings, Entity, Jobs, World};
use egui::{ClippedPrimitive, Color32, Id, LayerId, Order, Pos2, Stroke, TexturesDelta};
use glam::{Mat4, Vec3};
use std::sync::Arc;
use winit::event::WindowEvent;
use winit::window::Window;

#[derive(Debug, Clone, PartialEq)]
pub enum UiAction {
    Select(Entity),
    /// Add or remove a survivor from the current selection (ctrl/shift-click).
    ToggleSelect(Entity),
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
    /// Move the selected squad so its anchor stands at this ground point.
    MoveSelected {
        x: f32,
        z: f32,
    },
    InspectBuilding(usize),
    ToggleTerrainEditor,
    SetTerrainTool(crate::terrain_editor::TerrainTool),
    ToggleTerrainPanel,
    QuickSave,
    QuickLoad,
    AdjustBrushRadius(i32),
    AdjustBrushStrength(i32),
}

/// An open right-click menu: physical-pixel cursor position plus its entries.
struct OpenContextMenu {
    pos: (f32, f32),
    title: String,
    entries: Vec<ContextEntry>,
    /// Skip outside-click dismissal on the frame the menu opens.
    fresh: bool,
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
    pub terrain_editor: &'a crate::terrain_editor::TerrainEditor,
}

pub struct GameUi {
    context: egui::Context,
    state: egui_winit::State,
    renderer: egui_wgpu::Renderer,
    paint_jobs: Vec<ClippedPrimitive>,
    textures_delta: TexturesDelta,
    selected_building: Option<String>,
    context_menu: Option<OpenContextMenu>,
    terrain_panel_visible: bool,
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
            context_menu: None,
            terrain_panel_visible: true,
        }
    }

    pub fn open_context_menu(
        &mut self,
        pos: (f32, f32),
        title: String,
        entries: Vec<ContextEntry>,
    ) {
        self.context_menu = Some(OpenContextMenu {
            pos,
            title,
            entries,
            fresh: true,
        });
    }

    /// Closes the context menu; returns whether one was open.
    pub fn close_context_menu(&mut self) -> bool {
        self.context_menu.take().is_some()
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
            terrain_editor,
        } = data;
        let input = self.state.take_egui_input(window);
        let mut actions = Vec::new();
        let selected_building = &mut self.selected_building;
        let context_menu = &mut self.context_menu;
        let terrain_panel_visible = &mut self.terrain_panel_visible;
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
                    crate::side_panel::show(
                        ui,
                        &mut crate::side_panel::PanelCtx {
                            world,
                            selected_entities,
                            buildings,
                            jobs,
                            building_defs,
                            build_status,
                            selected_building: &mut *selected_building,
                            actions: &mut actions,
                        },
                    );
                });

            terrain_tool_panel(
                ctx,
                terrain_panel_visible,
                terrain_editor,
                screen_height,
                &mut actions,
            );

            if let Some(menu) = context_menu.as_mut() {
                let mut close = false;
                let response = egui::Area::new(Id::new("context_menu"))
                    .order(Order::Foreground)
                    .fixed_pos(Pos2::new(menu.pos.0 / scale, menu.pos.1 / scale))
                    .constrain(true)
                    .show(ctx, |ui| {
                        popover_frame().show(ui, |ui| {
                            ui.set_min_width(170.0);
                            section_header(ui, &menu.title);
                            ui.separator();
                            for entry in &menu.entries {
                                let text = match entry.hotkey {
                                    Some(key) => format!("{}  [{key}]", entry.label),
                                    None => entry.label.clone(),
                                };
                                if ui.selectable_label(false, text).clicked() {
                                    if let Some(action) = &entry.action {
                                        actions.push(action.clone());
                                    }
                                    close = true;
                                }
                            }
                        });
                    });
                if !menu.fresh && ctx.input(|i| i.pointer.any_pressed()) {
                    let outside = ctx
                        .input(|i| i.pointer.interact_pos())
                        .is_some_and(|p| !response.response.rect.contains(p));
                    close |= outside;
                }
                menu.fresh = false;
                if close {
                    *context_menu = None;
                }
            }
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

    pub fn toggle_terrain_panel(&mut self) {
        self.terrain_panel_visible = !self.terrain_panel_visible;
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

fn terrain_tool_panel(
    context: &egui::Context,
    visible: &mut bool,
    editor: &crate::terrain_editor::TerrainEditor,
    screen_height: f32,
    actions: &mut Vec<UiAction>,
) {
    MovablePanel::new("Terrain Tool", "terrain_tool_panel", Pos2::new(12.0, 12.0)).show(
        context,
        visible,
        |ui| {
            state_line(
                ui,
                "Terrain editing:",
                if editor.active { "On" } else { "Off" },
                editor.active,
            );
            if ui
                .button(if editor.active {
                    "Stop editing (T)"
                } else {
                    "Start editing (T)"
                })
                .clicked()
            {
                actions.push(UiAction::ToggleTerrainEditor);
            }
            ui.horizontal_wrapped(|ui| {
                for key in crate::input::TERRAIN_TOOL_KEYS {
                    let label = format!("{} ({})", terrain_tool_name(key.tool), key.key);
                    if ui
                        .add_enabled(
                            editor.active,
                            egui::SelectableLabel::new(editor.tool == key.tool, label),
                        )
                        .clicked()
                    {
                        actions.push(UiAction::SetTerrainTool(key.tool));
                    }
                }
            });
            status_line(
                ui,
                format!(
                    "Brush: {} m · Strength: {:.1} m",
                    editor.radius,
                    editor.strength_mm as f32 / 1000.0
                ),
            );
            if editor.slope_start.is_some() {
                status_line(ui, "Slope start set; click the end point.");
            }
            ui.separator();

            egui::ScrollArea::vertical()
                .max_height((screen_height - 100.0).max(120.0))
                .show(ui, |ui| {
                    for group in crate::input::BindingGroup::ALL {
                        let mut bindings = crate::input::BINDINGS
                            .iter()
                            .filter(|binding| binding.group == group)
                            .peekable();
                        if bindings.peek().is_none() {
                            continue;
                        }
                        section_header(ui, group.title());
                        for binding in bindings {
                            key_row(
                                ui,
                                binding.key,
                                binding.description,
                                editor.active && binding.terrain_relevant,
                            );
                        }
                        ui.add_space(4.0);
                    }
                });
        },
    );
}

fn terrain_tool_name(tool: crate::terrain_editor::TerrainTool) -> &'static str {
    match tool {
        crate::terrain_editor::TerrainTool::Raise => "Raise",
        crate::terrain_editor::TerrainTool::Lower => "Lower",
        crate::terrain_editor::TerrainTool::Flatten => "Flatten",
        crate::terrain_editor::TerrainTool::Smooth => "Smooth",
        crate::terrain_editor::TerrainTool::Slope => "Slope",
    }
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
