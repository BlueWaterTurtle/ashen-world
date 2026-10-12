//! Headless, deterministic simulation core. No rendering dependencies.

pub mod chunk;
pub mod path;
pub mod rng;
pub mod terrain;
pub mod world;

pub use chunk::{ChunkCoord, WorldPos, CHUNK_SIZE};
pub use path::{find_path, Cell};
pub use rng::Rng;
pub use terrain::{height_at, is_walkable, ChunkStreamer, ChunkTerrain};
pub use world::{Entity, World, WorldConfig, TICKS_PER_SECOND, TICK_DT};
