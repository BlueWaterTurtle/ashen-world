//! Rows for the keys panel. Keyboard rows are derived from the action descriptors so they
//! cannot drift from the real bindings; only input that is not a discrete `Action` is listed
//! here by hand.

use crate::actions::{ActionGroup, Availability, DESCRIPTORS};

pub(crate) struct KeyRow {
    pub(crate) key: String,
    pub(crate) description: &'static str,
    pub(crate) group: ActionGroup,
    /// Highlighted while the terrain editor is active.
    pub(crate) terrain_relevant: bool,
}

struct StaticRow {
    key: &'static str,
    description: &'static str,
    group: ActionGroup,
    terrain_relevant: bool,
}

// Static supplement: continuous camera input and mouse gestures. These are raw input handled
// in `input.rs` (held keys, drags, wheel, clicks whose meaning depends on the target), not
// discrete actions, so they have no descriptor. Keep in sync with `handle_key` and
// `handle_mouse_button`.
const STATIC_ROWS: &[StaticRow] = &[
    StaticRow {
        key: "Left mouse (editing)",
        description: "Apply the selected terrain tool at the cursor",
        group: ActionGroup::Terrain,
        terrain_relevant: true,
    },
    StaticRow {
        key: "Left mouse",
        description:
            "Place the selected building when placing; otherwise select, move, or drag-select",
        group: ActionGroup::Build,
        terrain_relevant: false,
    },
    StaticRow {
        key: "A / ←",
        description: "Pan camera left",
        group: ActionGroup::Camera,
        terrain_relevant: false,
    },
    StaticRow {
        key: "D / →",
        description: "Pan camera right",
        group: ActionGroup::Camera,
        terrain_relevant: false,
    },
    StaticRow {
        key: "W / ↑",
        description: "Pan camera forward",
        group: ActionGroup::Camera,
        terrain_relevant: false,
    },
    StaticRow {
        key: "S / ↓",
        description: "Pan camera back",
        group: ActionGroup::Camera,
        terrain_relevant: false,
    },
    StaticRow {
        key: "Q / E",
        description: "Orbit camera left / right",
        group: ActionGroup::Camera,
        terrain_relevant: false,
    },
    StaticRow {
        key: "Right / middle mouse drag",
        description: "Orbit camera",
        group: ActionGroup::Camera,
        terrain_relevant: false,
    },
    StaticRow {
        key: "Mouse wheel",
        description: "Zoom camera",
        group: ActionGroup::Camera,
        terrain_relevant: false,
    },
];

/// All rows: one per hotkeyed action, then the static supplement. A hotkey shared by actions
/// available in different contexts (F5) gets a context suffix so rows stay distinguishable.
pub(crate) fn rows() -> Vec<KeyRow> {
    let mut rows: Vec<KeyRow> = DESCRIPTORS
        .iter()
        .filter_map(|descriptor| {
            let hotkey = descriptor.hotkey?;
            let shared = DESCRIPTORS
                .iter()
                .filter(|other| other.hotkey == Some(hotkey))
                .count()
                > 1;
            Some(KeyRow {
                key: if shared {
                    format!("{hotkey}{}", descriptor.availability.tag())
                } else {
                    hotkey.to_string()
                },
                description: descriptor.description,
                group: descriptor.group,
                terrain_relevant: descriptor.availability == Availability::EditorActive,
            })
        })
        .collect();
    rows.extend(STATIC_ROWS.iter().map(|row| KeyRow {
        key: row.key.to_string(),
        description: row.description,
        group: row.group,
        terrain_relevant: row.terrain_relevant,
    }));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn rows_have_unique_keys_and_nonempty_text() {
        let mut keys = HashSet::new();
        for row in rows() {
            assert!(keys.insert(row.key.clone()), "duplicate key: {}", row.key);
            assert!(!row.key.trim().is_empty());
            assert!(!row.description.trim().is_empty());
        }
    }

    #[test]
    fn every_group_has_rows_and_a_title() {
        let rows = rows();
        for group in ActionGroup::ALL {
            assert!(!group.title().is_empty());
            assert!(rows.iter().any(|row| row.group == group));
        }
    }

    #[test]
    fn every_hotkeyed_action_is_listed() {
        let rows = rows();
        for descriptor in DESCRIPTORS {
            if let Some(hotkey) = descriptor.hotkey {
                assert!(rows.iter().any(|row| row.key.starts_with(hotkey)));
            }
        }
        let f5: Vec<_> = rows.iter().filter(|r| r.key.starts_with("F5")).collect();
        assert_eq!(f5.len(), 2);
    }
}
