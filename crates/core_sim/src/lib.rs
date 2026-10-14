//! Headless, deterministic simulation core. No rendering dependencies.

pub mod behavior;
pub mod building;
pub mod chunk;
pub mod clock;
pub mod npc;
pub mod path;
pub mod production;
pub mod resource;
pub mod rng;
pub mod terrain;
pub mod world;

pub use behavior::Behavior;
pub use building::{
    average_fertility, average_resource_richness, crop_progress, Building, BuildingLogistics,
    Buildings, Footprint, LogisticsError, PlaceError, PlacementRules, ResourceNode, SlotInventory,
};
pub use chunk::{ChunkCoord, WorldPos, CHUNK_SIZE};
pub use clock::GameClock;
pub use npc::{
    DespawnError, EntityKind, Inventory, ItemStack, LodConfig, NavGoal, NpcId, PersistentNpcRecord,
    SimTier, SimulationTier, Skills,
};
pub use path::{find_path, Cell, Pathfinder};
pub use production::{MineProduction, OutputSlot, ORE_SECONDS_PER_UNIT};
pub use resource::{ResourceDef, ResourceKind, DEFAULT_STACK_SIZE, ORE_STACK_SIZE};
pub use rng::Rng;
pub use terrain::{
    fertility_at, height_at, is_walkable, resource_richness_at, ChunkStreamer, ChunkTerrain,
};
pub use world::{
    ControlError, Entity, PlayerControlled, RecruitError, Recruitable, World, WorldConfig,
    RECRUIT_MAX_DISTANCE, TICKS_PER_SECOND, TICK_DT,
};
