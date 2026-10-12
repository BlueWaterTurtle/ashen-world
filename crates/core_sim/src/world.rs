use crate::chunk::{ChunkCoord, WorldPos, CHUNK_SIZE};
use crate::rng::Rng;

/// Fixed simulation rate.
pub const TICKS_PER_SECOND: u32 = 30;
/// Seconds per simulation tick.
pub const TICK_DT: f32 = 1.0 / TICKS_PER_SECOND as f32;

/// Opaque entity handle (index into component storage).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Entity(pub u32);

#[derive(Debug, Clone, PartialEq)]
pub struct WorldConfig {
    pub seed: u64,
    pub agent_count: usize,
    /// Agents spawn in chunks within this radius (in chunks) of the origin.
    pub chunk_radius: i32,
    pub max_speed: f32,
}

impl Default for WorldConfig {
    fn default() -> Self {
        Self {
            seed: 1,
            agent_count: 1000,
            chunk_radius: 2,
            max_speed: 3.0,
        }
    }
}

/// Struct-of-arrays world: component vectors indexed by entity id.
#[derive(Debug, Clone, PartialEq)]
pub struct World {
    pub seed: u64,
    pub tick: u64,
    pub positions: Vec<WorldPos>,
    pub velocities: Vec<WorldPos>,
}

impl World {
    /// Deterministically generate a world from the config.
    pub fn generate(config: &WorldConfig) -> Self {
        let mut rng = Rng::new(config.seed);
        let r = config.chunk_radius.max(0);
        let side = (2 * r + 1) as u64;
        let mut positions = Vec::with_capacity(config.agent_count);
        let mut velocities = Vec::with_capacity(config.agent_count);
        for _ in 0..config.agent_count {
            let cx = (rng.next_u64() % side) as i32 - r;
            let cz = (rng.next_u64() % side) as i32 - r;
            let local = WorldPos::new(
                rng.range_f32(0.0, CHUNK_SIZE),
                0.0,
                rng.range_f32(0.0, CHUNK_SIZE),
            );
            positions.push(WorldPos::from_chunk_local(ChunkCoord::new(cx, cz), local));
            velocities.push(WorldPos::new(
                rng.range_f32(-config.max_speed, config.max_speed),
                0.0,
                rng.range_f32(-config.max_speed, config.max_speed),
            ));
        }
        Self {
            seed: config.seed,
            tick: 0,
            positions,
            velocities,
        }
    }

    pub fn entity_count(&self) -> usize {
        self.positions.len()
    }

    pub fn entities(&self) -> impl Iterator<Item = Entity> {
        (0..self.positions.len() as u32).map(Entity)
    }

    pub fn position(&self, e: Entity) -> WorldPos {
        self.positions[e.0 as usize]
    }

    /// Advance the simulation by exactly one fixed tick.
    pub fn step(&mut self) {
        for (p, v) in self.positions.iter_mut().zip(&self.velocities) {
            p.x += v.x * TICK_DT;
            p.y += v.y * TICK_DT;
            p.z += v.z * TICK_DT;
        }
        self.tick += 1;
    }

    pub fn run_ticks(&mut self, n: u64) {
        for _ in 0..n {
            self.step();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(seed: u64) -> WorldConfig {
        WorldConfig {
            seed,
            agent_count: 200,
            ..Default::default()
        }
    }

    #[test]
    fn same_seed_is_deterministic() {
        assert_eq!(World::generate(&cfg(42)), World::generate(&cfg(42)));
        assert_ne!(World::generate(&cfg(42)), World::generate(&cfg(43)));
    }

    #[test]
    fn agents_spawn_in_nearby_chunks() {
        let c = cfg(7);
        let w = World::generate(&c);
        assert_eq!(w.entity_count(), 200);
        for e in w.entities() {
            let ch = w.position(e).chunk();
            assert!(ch.x.abs() <= c.chunk_radius && ch.z.abs() <= c.chunk_radius);
        }
    }

    #[test]
    fn movement_over_fixed_ticks() {
        let mut w = World {
            seed: 0,
            tick: 0,
            positions: vec![WorldPos::new(0.0, 0.0, 0.0)],
            velocities: vec![WorldPos::new(3.0, 0.0, -6.0)],
        };
        w.run_ticks(TICKS_PER_SECOND as u64);
        assert_eq!(w.tick, TICKS_PER_SECOND as u64);
        let p = w.positions[0];
        assert!((p.x - 3.0).abs() < 1e-3);
        assert!((p.z + 6.0).abs() < 1e-3);
    }
}
