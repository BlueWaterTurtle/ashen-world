//! Terrain editor state and tool application. Free of winit/wgpu so it can be tested headless.

use core_sim::terrain::MAX_BRUSH_RADIUS;
use core_sim::{Buildings, TerrainEdit, World};

const MIN_STRENGTH_MM: i32 = 250;
const MAX_STRENGTH_MM: i32 = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerrainTool {
    Raise,
    Lower,
    Flatten,
    Smooth,
    Slope,
}

/// Terrain editor mode: while active, left clicks reshape the terrain instead of selecting.
#[derive(Debug)]
pub struct TerrainEditor {
    pub active: bool,
    pub tool: TerrainTool,
    pub radius: i32,
    pub strength_mm: i32,
    pub slope_start: Option<(i32, i32)>,
}

impl Default for TerrainEditor {
    fn default() -> Self {
        Self {
            active: false,
            tool: TerrainTool::Raise,
            radius: 6,
            strength_mm: 1000,
            slope_start: None,
        }
    }
}

impl TerrainEditor {
    pub fn status(&self) -> String {
        let tool = match self.tool {
            TerrainTool::Raise => "Raise",
            TerrainTool::Lower => "Lower",
            TerrainTool::Flatten => "Flatten",
            TerrainTool::Smooth => "Smooth",
            TerrainTool::Slope => "Slope (click start, then end)",
        };
        format!(
            "Terrain editor: {tool}, radius {} m, strength {:.1} m. F1-F5 tool, [ ] radius, - = strength, T exits.",
            self.radius,
            self.strength_mm as f32 / 1000.0
        )
    }

    /// Switch tools, discarding any half-finished slope.
    pub fn set_tool(&mut self, tool: TerrainTool) {
        self.tool = tool;
        self.slope_start = None;
    }

    pub fn adjust_radius(&mut self, step: i32) {
        self.radius = (self.radius + step).clamp(1, MAX_BRUSH_RADIUS);
    }

    pub fn adjust_strength(&mut self, step_mm: i32) {
        self.strength_mm = (self.strength_mm + step_mm).clamp(MIN_STRENGTH_MM, MAX_STRENGTH_MM);
    }

    /// Apply the active tool at lattice point `at`, queueing the edit on `world` unless it
    /// would leave an existing building on a slope too steep to stand on. Returns the
    /// status message to show.
    pub fn apply(&mut self, at: (i32, i32), world: &mut World, buildings: &Buildings) -> String {
        let edit = match self.tool {
            TerrainTool::Raise => TerrainEdit::Raise {
                center: at,
                radius: self.radius,
                amount_mm: self.strength_mm,
            },
            TerrainTool::Lower => TerrainEdit::Lower {
                center: at,
                radius: self.radius,
                amount_mm: self.strength_mm,
            },
            TerrainTool::Flatten => {
                let target = world
                    .terrain_edits()
                    .height(world.seed, at.0 as f32, at.1 as f32);
                TerrainEdit::Flatten {
                    center: at,
                    radius: self.radius,
                    target_mm: (target * 1000.0).round() as i32,
                }
            }
            TerrainTool::Smooth => TerrainEdit::Smooth {
                center: at,
                radius: self.radius,
            },
            TerrainTool::Slope => match self.slope_start.take() {
                None => {
                    self.slope_start = Some(at);
                    return "Slope start set; click the end point.".into();
                }
                Some(from) => TerrainEdit::Slope {
                    from,
                    to: at,
                    half_width: self.radius,
                },
            },
        };
        if buildings.terrain_edit_is_allowed(world.seed, world.terrain_edits(), edit) {
            world.queue_terrain_edit(edit);
            self.status()
        } else {
            "Edit would leave a building on too steep a slope.".into()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_sim::WorldConfig;

    fn world() -> World {
        World::generate(&WorldConfig {
            agent_count: 1,
            ..Default::default()
        })
    }

    #[test]
    fn radius_and_strength_are_clamped() {
        let mut editor = TerrainEditor::default();
        editor.adjust_radius(-100);
        assert_eq!(editor.radius, 1);
        editor.adjust_radius(1000);
        assert_eq!(editor.radius, MAX_BRUSH_RADIUS);
        editor.adjust_strength(-100_000);
        assert_eq!(editor.strength_mm, MIN_STRENGTH_MM);
        editor.adjust_strength(100_000);
        assert_eq!(editor.strength_mm, MAX_STRENGTH_MM);
    }

    #[test]
    fn switching_tools_discards_pending_slope() {
        let mut editor = TerrainEditor {
            tool: TerrainTool::Slope,
            slope_start: Some((1, 2)),
            ..Default::default()
        };
        editor.set_tool(TerrainTool::Raise);
        assert_eq!(editor.tool, TerrainTool::Raise);
        assert_eq!(editor.slope_start, None);
    }

    #[test]
    fn slope_needs_two_clicks() {
        let mut world = world();
        let buildings = Buildings::default();
        let mut editor = TerrainEditor {
            tool: TerrainTool::Slope,
            ..Default::default()
        };
        let message = editor.apply((0, 0), &mut world, &buildings);
        assert!(message.contains("Slope start set"));
        assert_eq!(editor.slope_start, Some((0, 0)));
        let message = editor.apply((10, 0), &mut world, &buildings);
        assert_eq!(message, editor.status());
        assert_eq!(editor.slope_start, None);
    }

    #[test]
    fn raise_queues_an_edit_and_reports_status() {
        let mut world = world();
        let buildings = Buildings::default();
        let mut editor = TerrainEditor::default();
        let message = editor.apply((4, 4), &mut world, &buildings);
        assert_eq!(message, editor.status());
        assert_eq!(world.snapshot().pending_terrain_edits.len(), 1);
    }
}
