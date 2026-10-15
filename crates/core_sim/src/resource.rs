//! Resource item definitions. Stack sizes live here, in one place.

use serde::{Deserialize, Serialize};

/// Every resource item known to the simulation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ResourceKind {
    Wood,
    Iron,
    Tools,
    WheatSeed,
    Wheat,
    IronOre,
    IronIngot,
}

/// Default maximum stack size for resource items.
pub const DEFAULT_STACK_SIZE: u32 = 100;
/// Maximum stack size for ore.
pub const ORE_STACK_SIZE: u32 = 50;

/// Attributes that belong to a resource item itself. Add new per-item
/// attributes here so every consumer reads them from the item definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceDef {
    /// Content/inventory id.
    pub id: &'static str,
    /// Maximum number of this item in a single stack.
    pub stack_size: u32,
}

impl ResourceKind {
    pub const ALL: [ResourceKind; 7] = [
        ResourceKind::Wood,
        ResourceKind::Iron,
        ResourceKind::Tools,
        ResourceKind::WheatSeed,
        ResourceKind::Wheat,
        ResourceKind::IronOre,
        ResourceKind::IronIngot,
    ];

    /// The single definition of this item's attributes.
    pub const fn def(self) -> ResourceDef {
        let (id, stack_size) = match self {
            ResourceKind::Wood => ("wood", DEFAULT_STACK_SIZE),
            ResourceKind::Iron => ("iron", DEFAULT_STACK_SIZE),
            ResourceKind::Tools => ("tools", DEFAULT_STACK_SIZE),
            ResourceKind::WheatSeed => ("wheat_seed", DEFAULT_STACK_SIZE),
            ResourceKind::Wheat => ("wheat", DEFAULT_STACK_SIZE),
            ResourceKind::IronOre => ("iron_ore", ORE_STACK_SIZE),
            ResourceKind::IronIngot => ("iron_ingot", DEFAULT_STACK_SIZE),
        };
        ResourceDef { id, stack_size }
    }

    pub fn id(self) -> &'static str {
        self.def().id
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.id() == id)
    }

    pub fn stack_size(self) -> u32 {
        self.def().stack_size
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_resource_has_nonzero_stack_size() {
        for kind in ResourceKind::ALL {
            assert!(kind.stack_size() > 0, "{} has zero stack size", kind.id());
        }
    }

    #[test]
    fn ids_round_trip() {
        for kind in ResourceKind::ALL {
            assert_eq!(ResourceKind::from_id(kind.id()), Some(kind));
        }
        assert_eq!(ResourceKind::from_id("nope"), None);
        assert_eq!(ResourceKind::IronOre.stack_size(), ORE_STACK_SIZE);
    }
}
