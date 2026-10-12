//! Headless, deterministic simulation core. No rendering dependencies.

pub mod behavior;
pub mod building;
pub mod chunk;
pub mod clock;
pub mod npc;
pub mod path;
pub mod rng;
pub mod terrain;
pub mod world;

pub use behavior::Behavior;
pub use building::{
    crop_progress, Building, Buildings, Footprint, PlaceError, PlacementRules, ResourceNode,
};
pub use chunk::{ChunkCoord, WorldPos, CHUNK_SIZE};
pub use clock::GameClock;
pub use npc::{
    DespawnError, EntityKind, Inventory, ItemStack, LodConfig, NavGoal, NpcId, PersistentNpcRecord,
    SimTier, Skills,
};
pub use path::{find_path, Cell, Pathfinder};
pub use rng::Rng;
pub use terrain::{height_at, is_walkable, ChunkStreamer, ChunkTerrain};
pub use world::{
    ControlError, Entity, PlayerControlled, RecruitError, Recruitable, World, WorldConfig,
    RECRUIT_MAX_DISTANCE, TICKS_PER_SECOND, TICK_DT,
};
