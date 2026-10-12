//! Headless, deterministic simulation core. No rendering dependencies.

pub mod chunk;
pub mod rng;
pub mod world;

pub use chunk::{ChunkCoord, WorldPos, CHUNK_SIZE};
pub use rng::Rng;
pub use world::{Entity, World, WorldConfig, TICKS_PER_SECOND, TICK_DT};
