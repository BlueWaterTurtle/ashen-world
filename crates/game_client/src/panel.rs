//! Shared HUD palette, frames, the movable panel shell, and small widgets (section headers,
//! key rows, badges, status lines) so every panel and menu renders state the same way.

use egui::{Color32, Id, Pos2, Stroke};

pub(crate) const PANEL_FILL: Color32 = Color32::from_rgba_premultiplied(22, 24, 28, 235);
pub(crate) const ACCENT: Color32 = Color32::from_rgb(225, 190, 120);
pub(crate) const DIVIDER: Color32 = Color32::from_rgb(62, 66, 74);
pub(crate) const IDLE: Color32 = Color32::from_rgb(140, 144, 152);
pub(crate) const GOOD: Color32 = Color32::from_rgb(112, 224, 174);
pub(crate) const WARN: Color32 = ACCENT;
pub(crate) const BAD: Color32 = Color32::from_rgb(224, 112, 112);

/// Dark, framed panel style with an amber accent line on the inner edge.
pub(crate) fn panel_frame() -> egui::Frame {
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

/// Small rounded popover used by the right-click context menu.
pub(crate) fn popover_frame() -> egui::Frame {
    egui::Frame::none()
        .fill(PANEL_FILL)
        .stroke(Stroke::new(1.0_f32, ACCENT))
        .rounding(egui::Rounding::same(6.0))
        .inner_margin(egui::Margin::same(8.0))
        .shadow(egui::epaint::Shadow {
            offset: egui::vec2(2.0, 3.0),
            blur: 10.0,
            spread: 0.0,
            color: Color32::from_black_alpha(120),
        })
}

/// A draggable, closable HUD window with the shared frame and a default position.
pub(crate) struct MovablePanel {
    title: &'static str,
    id: &'static str,
    default_pos: Pos2,
    default_width: f32,
}

impl MovablePanel {
    pub(crate) fn new(title: &'static str, id: &'static str, default_pos: Pos2) -> Self {
        Self {
            title,
            id,
            default_pos,
            default_width: 290.0,
        }
    }

    pub(crate) fn show(
        self,
        context: &egui::Context,
        visible: &mut bool,
        add_contents: impl FnOnce(&mut egui::Ui),
    ) {
        egui::Window::new(self.title)
            .id(Id::new(self.id))
            .default_pos(self.default_pos)
            .default_width(self.default_width)
            .resizable(true)
            .constrain(true)
            .frame(panel_frame())
            .open(visible)
            .show(context, add_contents);
    }
}

/// Accent-coloured bold heading for a group of rows.
pub(crate) fn section_header(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).strong().color(ACCENT));
}

/// Coloured state label such as "On" / "blocked".
pub(crate) fn badge(ui: &mut egui::Ui, text: &str, color: Color32) {
    ui.label(egui::RichText::new(text).strong().color(color));
}

/// "Label  [badge]" line; `active` picks the good/idle colour.
pub(crate) fn state_line(ui: &mut egui::Ui, label: &str, text: &str, active: bool) {
    ui.horizontal(|ui| {
        ui.label(label);
        badge(ui, text, if active { GOOD } else { IDLE });
    });
}

/// Small secondary line of status text.
pub(crate) fn status_line(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.small(text.into());
}

/// A hotkey followed by its description; `highlight` marks currently relevant keys.
pub(crate) fn key_row(ui: &mut egui::Ui, key: &str, description: &str, highlight: bool) {
    ui.horizontal(|ui| {
        let key = egui::RichText::new(key).strong();
        ui.label(if highlight { key.color(ACCENT) } else { key });
        ui.label(description);
    });
}
