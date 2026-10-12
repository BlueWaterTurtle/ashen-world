use std::collections::VecDeque;

use crate::chunk::{ChunkCoord, WorldPos, CHUNK_SIZE};
use crate::path::{find_path, Cell};
use crate::rng::Rng;
use crate::terrain::{height_at, is_walkable};

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
    /// Remaining waypoints (cell centers, in order) for each entity, if it is navigating.
    pub paths: Vec<Option<VecDeque<Cell>>>,
    /// Walking speed (m/s) used when following a path.
    pub walk_speed: f32,
}

/// Maximum A* node expansions per path request.
pub const PATH_BUDGET: usize = 20_000;

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
        let positions_len = positions.len();
        Self {
            seed: config.seed,
            tick: 0,
            paths: vec![None; positions_len],
            positions,
            velocities,
            walk_speed: config.max_speed,
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

    /// Plan a walkable route for `e` to the cell containing `goal`.
    /// Returns false (and leaves the entity unchanged) if no route is found.
    pub fn navigate_to(&mut self, e: Entity, goal: WorldPos) -> bool {
        let i = e.0 as usize;
        let p = self.positions[i];
        let start = (p.x.floor() as i32, p.z.floor() as i32);
        let goal = (goal.x.floor() as i32, goal.z.floor() as i32);
        let seed = self.seed;
        match find_path(start, goal, PATH_BUDGET, |c| is_walkable(seed, c)) {
            Some(path) => {
                self.paths[i] = Some(path.into_iter().skip(1).collect());
                true
            }
            None => false,
        }
    }

    /// Advance the simulation by exactly one fixed tick.
    pub fn step(&mut self) {
        self.follow_paths();
        for (p, v) in self.positions.iter_mut().zip(&self.velocities) {
            p.x += v.x * TICK_DT;
            p.y += v.y * TICK_DT;
            p.z += v.z * TICK_DT;
        }
        self.tick += 1;
    }

    fn follow_paths(&mut self) {
        let step_len = self.walk_speed * TICK_DT;
        for i in 0..self.paths.len() {
            let Some(path) = self.paths[i].as_mut() else {
                continue;
            };
            let Some(&(cx, cz)) = path.front() else {
                self.paths[i] = None;
                self.velocities[i] = WorldPos::default();
                continue;
            };
            let (tx, tz) = (cx as f32 + 0.5, cz as f32 + 0.5);
            let p = self.positions[i];
            let (dx, dz) = (tx - p.x, tz - p.z);
            let dist = (dx * dx + dz * dz).sqrt();
            if dist <= step_len {
                self.positions[i] = WorldPos::new(tx, height_at(self.seed, tx, tz), tz);
                self.velocities[i] = WorldPos::default();
                path.pop_front();
            } else {
                self.velocities[i] = WorldPos::new(
                    dx / dist * self.walk_speed,
                    0.0,
                    dz / dist * self.walk_speed,
                );
            }
        }
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
            paths: vec![None],
            walk_speed: 3.0,
        };
        w.run_ticks(TICKS_PER_SECOND as u64);
        assert_eq!(w.tick, TICKS_PER_SECOND as u64);
        let p = w.positions[0];
        assert!((p.x - 3.0).abs() < 1e-3);
        assert!((p.z + 6.0).abs() < 1e-3);
    }

    #[test]
    fn agent_follows_path_to_goal() {
        let mut w = World::generate(&WorldConfig {
            seed: 11,
            agent_count: 1,
            ..Default::default()
        });
        let seed = w.seed;
        // Place the agent on a walkable cell with a walkable goal nearby.
        let start = (0..200)
            .map(|i| (i, i / 2))
            .find(|&c| is_walkable(seed, c))
            .unwrap();
        let goal = (0..200)
            .map(|i| (start.0 + 10 + i % 5, start.1 + i / 5))
            .find(|&c| is_walkable(seed, c))
            .unwrap();
        w.positions[0] = WorldPos::new(start.0 as f32 + 0.5, 0.0, start.1 as f32 + 0.5);
        let goal_pos = WorldPos::new(goal.0 as f32 + 0.5, 0.0, goal.1 as f32 + 0.5);
        assert!(w.navigate_to(Entity(0), goal_pos));
        {
            w.run_ticks(TICKS_PER_SECOND as u64 * 120);
            let p = w.positions[0];
            assert!((p.x - goal_pos.x).abs() < 1e-3 && (p.z - goal_pos.z).abs() < 1e-3);
            assert!(w.paths[0].is_none());
        }
    }

    #[test]
    fn navigate_to_blocked_goal_fails() {
        let mut w = World::generate(&WorldConfig {
            seed: 11,
            agent_count: 1,
            ..Default::default()
        });
        let seed = w.seed;
        let bad = (0..500)
            .map(|i| (i, 3 * i))
            .find(|&c| !is_walkable(seed, c));
        let c = bad.expect("unwalkable cell exists");
        {
            let before = w.paths[0].clone();
            assert!(!w.navigate_to(Entity(0), WorldPos::new(c.0 as f32, 0.0, c.1 as f32)));
            assert_eq!(w.paths[0], before);
        }
    }
}
