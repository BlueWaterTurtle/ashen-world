use std::collections::BTreeMap;
use std::fmt;

use crate::chunk::WorldPos;

/// Stable, persistent identifier for an NPC. Survives across distance and despawn/resume.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct NpcId(pub u64);

impl fmt::Display for NpcId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NpcId({})", self.0)
    }
}

/// Category of entity in the simulation world.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EntityKind {
    /// Persistent NPC whose identity, skills, and inventory persist across distance and tiers.
    #[default]
    PersistentNpc,
    /// Disposable entity (e.g. ambient wildlife or temporary encounter traffic).
    Disposable,
}

/// Persistent skill progression for an NPC.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq, Default)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum SimTier {
    /// Full movement, pathfinding, and behavior updates every simulation tick.
    #[default]
    Active,
    /// Throttled updates (e.g. every N ticks) with reduced movement/scheduling overhead.
    Reduced,
    /// State survives persistently, but full A* pathfinding is omitted.
    /// Advances toward destinations with cheap direct movement and resumes cleanly when in focus.
    Dormant,
}

/// Configuration for simulation LOD tiering.
#[derive(Debug, Clone, PartialEq)]
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
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct NavGoal(pub Option<WorldPos>);

/// A complete snapshot of a persistent NPC, suitable for long-term storage or despawn/resume.
#[derive(Debug, Clone, PartialEq)]
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
    pub tier: SimTier,
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
}
