//! Table of every input binding shown by the terrain tool panel.
//!
//! This table lives next to the input handling but is not generated from it: when you add or
//! change a binding in `input.rs` (`handle_key`, `handle_mouse_button`, `handle_mouse_wheel`),
//! update `BINDINGS` together with it.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindingGroup {
    Terrain,
    Camera,
    Build,
    SaveLoad,
    General,
}

impl BindingGroup {
    pub const ALL: [BindingGroup; 5] = [
        BindingGroup::Terrain,
        BindingGroup::Camera,
        BindingGroup::Build,
        BindingGroup::SaveLoad,
        BindingGroup::General,
    ];

    pub fn title(self) -> &'static str {
        match self {
            BindingGroup::Terrain => "Terrain",
            BindingGroup::Camera => "Camera",
            BindingGroup::Build => "Build",
            BindingGroup::SaveLoad => "Save / Load",
            BindingGroup::General => "General",
        }
    }
}

pub struct Binding {
    pub key: &'static str,
    pub description: &'static str,
    pub group: BindingGroup,
}

const fn b(key: &'static str, description: &'static str, group: BindingGroup) -> Binding {
    Binding {
        key,
        description,
        group,
    }
}

/// Key that shows or hides the panel (handled in `input.rs`).
pub const TOGGLE_PANEL_KEY: &str = "H";

use BindingGroup::{Build, Camera, General, SaveLoad, Terrain};

pub const BINDINGS: &[Binding] = &[
    b("T", "Toggle terrain editor", Terrain),
    b("F1", "Raise tool (editor active)", Terrain),
    b("F2", "Lower tool (editor active)", Terrain),
    b("F3", "Flatten tool (editor active)", Terrain),
    b("F4", "Smooth tool (editor active)", Terrain),
    b("F5 (editor)", "Slope tool (editor active)", Terrain),
    b(
        "[ / ]",
        "Shrink / grow brush radius (editor active)",
        Terrain,
    ),
    b(
        "- / =",
        "Lower / raise brush strength (editor active)",
        Terrain,
    ),
    b(
        "Left click (editor)",
        "Apply the active terrain tool",
        Terrain,
    ),
    b("W A S D / Arrows", "Pan camera", Camera),
    b("Q / E", "Orbit camera left / right", Camera),
    b("Right-drag / Middle-drag", "Orbit camera", Camera),
    b("Mouse wheel", "Zoom camera", Camera),
    b("Space", "Center camera on selected character", Camera),
    b("Left click (placing)", "Place the selected building", Build),
    b("Esc (placing)", "Cancel building placement", Build),
    b("F5", "Quick-save", SaveLoad),
    b("F9", "Quick-load", SaveLoad),
    b("1 / 2 / 3", "Select player character 1 / 2 / 3", General),
    b("R", "Recruit nearest recruitable character", General),
    b("Left click", "Select / move selected squad", General),
    b("Left-drag", "Box-select characters", General),
    b("Right click", "Open context menu at the cursor", General),
    b(TOGGLE_PANEL_KEY, "Show / hide this panel", General),
    b(
        "Esc",
        "Close menu, exit editor or placement, then quit",
        General,
    ),
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn bindings_are_unique_and_described() {
        let mut seen = HashSet::new();
        for binding in BINDINGS {
            assert!(!binding.key.is_empty());
            assert!(!binding.description.trim().is_empty());
            assert!(seen.insert(binding.key), "duplicate key {}", binding.key);
        }
    }

    #[test]
    fn terrain_group_is_listed_first_and_toggle_is_present() {
        assert_eq!(BINDINGS[0].group, BindingGroup::Terrain);
        assert!(BINDINGS.iter().any(|b| b.key == TOGGLE_PANEL_KEY));
    }
}
