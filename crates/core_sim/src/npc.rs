use std::collections::BTreeMap;
use std::fmt;

use crate::chunk::WorldPos;
use serde::{Deserialize, Serialize};

/// Stable, persistent identifier for an NPC. Survives across distance and despawn/resume.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
pub struct NpcId(pub u64);

impl fmt::Display for NpcId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NpcId({})", self.0)
    }
}

/// Category of entity in the simulation world.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum EntityKind {
    /// Persistent NPC whose identity, skills, and inventory persist across distance and tiers.
    #[default]
    PersistentNpc,
    /// Disposable entity (e.g. ambient wildlife or temporary encounter traffic).
    Disposable,
}

/// Persistent skill progression for an NPC.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Skills {
    levels: BTreeMap<String, u32>,
}

impl Skills {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, name: &str) -> u32 {
        self.levels.get(name).copied().unwrap_or(0)
    }

    pub fn set(&mut self, name: impl Into<String>, level: u32) {
        self.levels.insert(name.into(), level);
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, u32)> {
        self.levels.iter().map(|(k, v)| (k.as_str(), *v))
    }

    pub fn len(&self) -> usize {
        self.levels.len()
    }

    pub fn is_empty(&self) -> bool {
        self.levels.is_empty()
    }
}

/// A stack of items in an inventory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemStack {
    pub item: String,
    pub count: u32,
}

impl ItemStack {
    pub fn new(item: impl Into<String>, count: u32) -> Self {
        Self {
            item: item.into(),
            count,
        }
    }
}

/// Persistent inventory holding items for an NPC.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Inventory {
    items: Vec<ItemStack>,
}

impl Inventory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn count_of(&self, item: &str) -> u32 {
        self.items
            .iter()
            .filter(|s| s.item == item)
            .map(|s| s.count)
            .sum()
    }

    pub fn add(&mut self, item: impl Into<String>, count: u32) {
        if count == 0 {
            return;
        }
        let item = item.into();
        if let Some(existing) = self.items.iter_mut().find(|s| s.item == item) {
            existing.count = existing.count.saturating_add(count);
        } else {
            self.items.push(ItemStack::new(item, count));
        }
    }

    pub fn remove(&mut self, item: &str, mut count: u32) -> bool {
        if count == 0 {
            return true;
        }
        if self.count_of(item) < count {
            return false;
        }
        for stack in self.items.iter_mut() {
            if stack.item == item {
                let take = stack.count.min(count);
                stack.count -= take;
                count -= take;
                if count == 0 {
                    break;
                }
            }
        }
        self.items.retain(|s| s.count > 0);
        true
    }

    pub fn items(&self) -> &[ItemStack] {
        &self.items
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// Deterministic simulation tiers for persistent NPCs.
///
/// Distinguishes full per-tick simulation from cheaper throttled simulation and
/// coarse background persistence, while keeping rendering decisions independent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
pub enum SimulationTier {
    /// Full movement, pathfinding, and behavior updates every simulation tick.
    /// Rendered when visible. Used for player characters, squad members, and nearby NPCs.
    #[default]
    Active,
    /// Throttled updates (e.g. every N ticks) with reduced movement/scheduling overhead.
    /// Rendered when visible. Used for broader-area NPCs relevant to the current region.
    Reduced,
    /// State survives persistently, but full A* pathfinding and per-tick physics are skipped.
    /// Advances toward destinations with cheap direct movement and resumes cleanly when in focus.
    /// Generally not rendered until promoted back to a higher tier.
    Dormant,
}

/// Backwards-compatible alias for [`SimulationTier`].
pub type SimTier = SimulationTier;

impl SimulationTier {
    /// Whether this NPC is in the [`SimulationTier::Active`] tier.
    #[inline]
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Active)
    }

    /// Whether this NPC is in the [`SimulationTier::Reduced`] tier.
    #[inline]
    pub const fn is_reduced(self) -> bool {
        matches!(self, Self::Reduced)
    }

    /// Whether this NPC is in the [`SimulationTier::Dormant`] tier.
    #[inline]
    pub const fn is_dormant(self) -> bool {
        matches!(self, Self::Dormant)
    }

    /// Whether this tier allows the NPC to be rendered when visible in the camera frustum.
    ///
    /// Rendering depends on visibility and renderability rather than full simulation:
    /// both `Active` and `Reduced` NPCs are rendered when in view, while `Dormant` NPCs
    /// are not rendered until promoted.
    #[inline]
    pub const fn is_renderable(self) -> bool {
        !self.is_dormant()
    }

    /// Effective update interval (in simulation ticks) for this tier.
    #[inline]
    pub const fn update_interval(self, reduced_interval: u32, dormant_interval: u32) -> u32 {
        match self {
            Self::Active => 1,
            Self::Reduced => {
                if reduced_interval > 0 {
                    reduced_interval
                } else {
                    1
                }
            }
            Self::Dormant => {
                if dormant_interval > 0 {
                    dormant_interval
                } else {
                    1
                }
            }
        }
    }
}

/// Configuration for simulation LOD tiering.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LodConfig {
    /// Whether simulation LOD tiering is active.
    pub enabled: bool,
    /// Distance from closest focus point (meters) for Active tier.
    pub active_radius: f32,
    /// Distance from closest focus point (meters) for Reduced tier.
    /// Entities beyond this distance are Dormant.
    pub reduced_radius: f32,
    /// Ticks between updates for Reduced-tier entities.
    pub reduced_tick_interval: u32,
    /// Ticks between updates for Dormant-tier entities.
    pub dormant_tick_interval: u32,
    /// Whether player-controlled characters automatically act as focus points.
    pub track_player_focus: bool,
}

impl Default for LodConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            active_radius: 64.0,
            reduced_radius: 160.0,
            reduced_tick_interval: 4,
            dormant_tick_interval: 10,
            track_player_focus: true,
        }
    }
}

impl LodConfig {
    /// Create an enabled LodConfig with default radii.
    pub fn enabled() -> Self {
        Self {
            enabled: true,
            ..Default::default()
        }
    }

    /// Create an enabled LodConfig with custom radii.
    pub fn with_radii(active_radius: f32, reduced_radius: f32) -> Self {
        Self {
            enabled: true,
            active_radius,
            reduced_radius,
            ..Default::default()
        }
    }
}

/// Target destination for navigation or abstract movement.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct NavGoal(pub Option<WorldPos>);

/// A complete snapshot of a persistent NPC, suitable for long-term storage or despawn/resume.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistentNpcRecord {
    pub id: NpcId,
    pub kind: EntityKind,
    pub position: WorldPos,
    pub velocity: WorldPos,
    pub goal: Option<WorldPos>,
    pub behavior_id: usize,
    pub pause_ticks: u32,
    pub player_controlled: bool,
    pub recruitable: bool,
    pub tier: SimulationTier,
    pub skills: Skills,
    pub inventory: Inventory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DespawnError {
    NotFound,
    AlreadyDespawned,
}

impl fmt::Display for DespawnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => write!(f, "entity not found"),
            Self::AlreadyDespawned => write!(f, "entity is already despawned"),
        }
    }
}

impl std::error::Error for DespawnError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skills_management() {
        let mut skills = Skills::new();
        assert_eq!(skills.get("mining"), 0);
        skills.set("mining", 5);
        skills.set("athletics", 12);
        assert_eq!(skills.get("mining"), 5);
        assert_eq!(skills.get("athletics"), 12);
        assert_eq!(skills.len(), 2);
    }

    #[test]
    fn inventory_add_remove_and_count() {
        let mut inv = Inventory::new();
        assert!(inv.is_empty());
        inv.add("iron_ore", 10);
        inv.add("iron_ore", 5);
        inv.add("bread", 2);

        assert_eq!(inv.count_of("iron_ore"), 15);
        assert_eq!(inv.count_of("bread"), 2);
        assert_eq!(inv.count_of("sword"), 0);

        assert!(inv.remove("iron_ore", 6));
        assert_eq!(inv.count_of("iron_ore"), 9);

        // Cannot remove more than available
        assert!(!inv.remove("iron_ore", 20));
        assert_eq!(inv.count_of("iron_ore"), 9);

        // Removing exactly remainder
        assert!(inv.remove("iron_ore", 9));
        assert_eq!(inv.count_of("iron_ore"), 0);
        assert_eq!(inv.items().len(), 1); // only bread remains
    }

    #[test]
    fn simulation_tier_properties_and_rendering_separation() {
        let active = SimulationTier::Active;
        let reduced = SimulationTier::Reduced;
        let dormant = SimulationTier::Dormant;

        assert!(active.is_active());
        assert!(!active.is_reduced());
        assert!(!active.is_dormant());
        assert!(active.is_renderable());
        assert_eq!(active.update_interval(4, 10), 1);

        assert!(!reduced.is_active());
        assert!(reduced.is_reduced());
        assert!(!reduced.is_dormant());
        // Reduced NPCs are renderable when visible, even though update is throttled
        assert!(reduced.is_renderable());
        assert_eq!(reduced.update_interval(4, 10), 4);

        assert!(!dormant.is_active());
        assert!(!dormant.is_reduced());
        assert!(dormant.is_dormant());
        // Dormant NPCs are not renderable until promoted
        assert!(!dormant.is_renderable());
        assert_eq!(dormant.update_interval(4, 10), 10);
    }
}
