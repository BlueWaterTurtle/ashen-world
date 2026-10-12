use std::collections::VecDeque;

use crate::behavior::Behavior;
use crate::chunk::{ChunkCoord, WorldPos, CHUNK_SIZE};
use crate::path::{find_path, Cell};
use crate::rng::Rng;
use crate::terrain::{height_at, is_walkable};

/// Fixed simulation rate.
pub const TICKS_PER_SECOND: u32 = 30;
/// Seconds per simulation tick.
pub const TICK_DT: f32 = 1.0 / TICKS_PER_SECOND as f32;

/// Opaque, stable handle to an agent: its spawn index. Spawn order is deterministic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Entity(pub u32);

/// Components stored in the ECS. Every agent always has all of them, so all agents live
/// in one archetype and iteration order equals spawn order (needed for determinism).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Position(pub WorldPos);
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Velocity(pub WorldPos);
/// Remaining waypoints (cell coordinates, in order); `None` when idle.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Path(pub Option<VecDeque<Cell>>);
/// Index into [`World::behaviors`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BehaviorId(pub usize);
/// Ticks remaining before an idle agent may pick a new destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pause(pub u32);

#[derive(Debug, Clone, PartialEq)]
pub struct WorldConfig {
    pub seed: u64,
    pub agent_count: usize,
    /// Agents spawn in chunks within this radius (in chunks) of the origin.
    pub chunk_radius: i32,
    /// Behaviour table; agent `i` uses `behaviors[i % behaviors.len()]`.
    pub behaviors: Vec<Behavior>,
    /// Maximum number of path requests served per tick (bounds A* cost).
    pub max_path_requests_per_tick: usize,
}

impl Default for WorldConfig {
    fn default() -> Self {
        Self {
            seed: 1,
            agent_count: 1000,
            chunk_radius: 2,
            behaviors: vec![Behavior::default()],
            max_path_requests_per_tick: 8,
        }
    }
}

/// The simulation world: an ECS holding agents plus global sim state.
pub struct World {
    pub seed: u64,
    pub tick: u64,
    pub ecs: hecs::World,
    ids: Vec<hecs::Entity>,
    pub behaviors: Vec<Behavior>,
    pub max_path_requests_per_tick: usize,
    rng: Rng,
    /// Round-robin cursor so every idle agent eventually gets a path request.
    wander_cursor: usize,
}

/// Maximum A* node expansions per path request.
pub const PATH_BUDGET: usize = 20_000;

impl World {
    /// Deterministically generate a world from the config.
    pub fn generate(config: &WorldConfig) -> Self {
        let mut rng = Rng::new(config.seed);
        let r = config.chunk_radius.max(0);
        let side = (2 * r + 1) as u64;
        let behaviors = if config.behaviors.is_empty() {
            vec![Behavior::default()]
        } else {
            config.behaviors.clone()
        };
        let mut ecs = hecs::World::new();
        let mut ids = Vec::with_capacity(config.agent_count);
        for i in 0..config.agent_count {
            let cx = (rng.next_u64() % side) as i32 - r;
            let cz = (rng.next_u64() % side) as i32 - r;
            let local = WorldPos::new(
                rng.range_f32(0.0, CHUNK_SIZE),
                0.0,
                rng.range_f32(0.0, CHUNK_SIZE),
            );
            let pos = WorldPos::from_chunk_local(ChunkCoord::new(cx, cz), local);
            ids.push(ecs.spawn((
                Position(pos),
                Velocity::default(),
                Path::default(),
                BehaviorId(i % behaviors.len()),
                Pause::default(),
            )));
        }
        Self {
            seed: config.seed,
            tick: 0,
            ecs,
            ids,
            behaviors,
            max_path_requests_per_tick: config.max_path_requests_per_tick,
            rng,
            wander_cursor: 0,
        }
    }

    pub fn entity_count(&self) -> usize {
        self.ids.len()
    }

    pub fn entities(&self) -> impl Iterator<Item = Entity> {
        (0..self.ids.len() as u32).map(Entity)
    }

    pub fn position(&self, e: Entity) -> WorldPos {
        self.ecs
            .get::<&Position>(self.ids[e.0 as usize])
            .map(|p| p.0)
            .unwrap_or_default()
    }

    pub fn set_position(&mut self, e: Entity, p: WorldPos) {
        if let Ok(mut c) = self.ecs.get::<&mut Position>(self.ids[e.0 as usize]) {
            c.0 = p;
        }
    }

    /// All agent positions in spawn order (stable between ticks).
    pub fn positions(&self) -> impl Iterator<Item = WorldPos> + '_ {
        self.ecs
            .query::<&Position>()
            .iter()
            .map(|(_, p)| p.0)
            .collect::<Vec<_>>()
            .into_iter()
    }

    pub fn is_navigating(&self, e: Entity) -> bool {
        self.ecs
            .get::<&Path>(self.ids[e.0 as usize])
            .map(|p| p.0.is_some())
            .unwrap_or(false)
    }

    pub fn behavior_of(&self, e: Entity) -> usize {
        self.ecs
            .get::<&BehaviorId>(self.ids[e.0 as usize])
            .map(|b| b.0)
            .unwrap_or(0)
    }

    pub fn pause_of(&self, e: Entity) -> u32 {
        self.ecs
            .get::<&Pause>(self.ids[e.0 as usize])
            .map(|p| p.0)
            .unwrap_or(0)
    }

    /// Number of agents currently following a path.
    pub fn navigating_count(&self) -> usize {
        self.ecs
            .query::<&Path>()
            .iter()
            .filter(|(_, p)| p.0.is_some())
            .count()
    }

    /// Plan a walkable route for `e` to the cell containing `goal`.
    /// Returns false (and leaves the entity unchanged) if no route is found.
    pub fn navigate_to(&mut self, e: Entity, goal: WorldPos) -> bool {
        let id = self.ids[e.0 as usize];
        let p = self.position(e);
        let start = (p.x.floor() as i32, p.z.floor() as i32);
        let goal = (goal.x.floor() as i32, goal.z.floor() as i32);
        let seed = self.seed;
        match find_path(start, goal, PATH_BUDGET, |c| is_walkable(seed, c)) {
            Some(path) => {
                if let Ok(mut c) = self.ecs.get::<&mut Path>(id) {
                    c.0 = Some(path.into_iter().skip(1).collect());
                }
                true
            }
            None => false,
        }
    }

    /// Advance the simulation by exactly one fixed tick.
    pub fn step(&mut self) {
        self.assign_wander_goals();
        self.follow_paths();
        self.tick += 1;
    }

    /// Serve a bounded number of path requests for idle agents, round-robin.
    fn assign_wander_goals(&mut self) {
        let n = self.ids.len();
        let mut requests = 0;
        for _ in 0..n {
            if requests >= self.max_path_requests_per_tick {
                break;
            }
            let i = self.wander_cursor;
            self.wander_cursor = (self.wander_cursor + 1) % n;
            let e = Entity(i as u32);
            let r = self.behaviors[self.behavior_of(e)].wander_range;
            if self.is_navigating(e) || self.pause_of(e) > 0 || r == 0 {
                continue;
            }
            requests += 1;
            let span = 2 * r as u64 + 1;
            let dx = (self.rng.next_u64() % span) as i32 - r;
            let dz = (self.rng.next_u64() % span) as i32 - r;
            let p = self.position(e);
            self.navigate_to(e, WorldPos::new(p.x + dx as f32, 0.0, p.z + dz as f32));
        }
    }

    fn follow_paths(&mut self) {
        let seed = self.seed;
        let behaviors = &self.behaviors;
        let rng = &mut self.rng;
        for (_, (pos, vel, path, bid, pause)) in self.ecs.query_mut::<(
            &mut Position,
            &mut Velocity,
            &mut Path,
            &BehaviorId,
            &mut Pause,
        )>() {
            pause.0 = pause.0.saturating_sub(1);
            let b = &behaviors[bid.0];
            let step_len = b.speed * TICK_DT;
            let Some(wp) = path.0.as_mut() else {
                continue;
            };
            let Some(&(cx, cz)) = wp.front() else {
                path.0 = None;
                vel.0 = WorldPos::default();
                let (min, max) = (b.pause_ticks_min, b.pause_ticks_max);
                let extra = if max > min {
                    (rng.next_u64() % (max - min + 1) as u64) as u32
                } else {
                    0
                };
                pause.0 = min + extra;
                continue;
            };
            let (tx, tz) = (cx as f32 + 0.5, cz as f32 + 0.5);
            let (dx, dz) = (tx - pos.0.x, tz - pos.0.z);
            let dist = (dx * dx + dz * dz).sqrt();
            if dist <= step_len {
                pos.0 = WorldPos::new(tx, height_at(seed, tx, tz), tz);
                vel.0 = WorldPos::default();
                wp.pop_front();
            } else {
                vel.0 = WorldPos::new(dx / dist * b.speed, 0.0, dz / dist * b.speed);
                pos.0.x += vel.0.x * TICK_DT;
                pos.0.z += vel.0.z * TICK_DT;
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

    fn snapshot(w: &World) -> Vec<WorldPos> {
        w.positions().collect()
    }

    #[test]
    fn same_seed_is_deterministic() {
        assert_eq!(
            snapshot(&World::generate(&cfg(42))),
            snapshot(&World::generate(&cfg(42)))
        );
        assert_ne!(
            snapshot(&World::generate(&cfg(42))),
            snapshot(&World::generate(&cfg(43)))
        );
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
    fn spawn_order_matches_entity_index() {
        let w = World::generate(&cfg(3));
        let by_index: Vec<_> = w.entities().map(|e| w.position(e)).collect();
        assert_eq!(by_index, snapshot(&w));
    }

    #[test]
    fn movement_over_fixed_ticks() {
        let mut w = World::generate(&WorldConfig {
            seed: 0,
            agent_count: 1,
            behaviors: vec![Behavior::from_seconds(0, 0.0, 0.0, 3.0)],
            ..Default::default()
        });
        let e = Entity(0);
        w.set_position(e, WorldPos::new(0.0, 0.0, 0.0));
        // Give the agent a straight run of 30 cells along +x; at 3 m/s it needs 10 s.
        let id = w.ids[0];
        w.ecs.get::<&mut Path>(id).unwrap().0 = Some((0..30).map(|x| (x, 0)).collect());
        w.run_ticks(TICKS_PER_SECOND as u64 * 4);
        let p = w.position(e);
        assert!(p.x > 8.0 && p.x < 14.0, "x = {}", p.x);
        assert!((p.z - 0.5).abs() < 1.0);
    }

    #[test]
    fn agent_follows_path_to_goal() {
        let mut w = World::generate(&WorldConfig {
            seed: 11,
            agent_count: 1,
            behaviors: vec![Behavior::from_seconds(0, 0.0, 0.0, 3.0)],
            ..Default::default()
        });
        let seed = w.seed;
        let start = (0..200)
            .map(|i| (i, i / 2))
            .find(|&c| is_walkable(seed, c))
            .unwrap();
        let goal = (0..200)
            .map(|i| (start.0 + 10 + i % 5, start.1 + i / 5))
            .find(|&c| is_walkable(seed, c))
            .unwrap();
        let e = Entity(0);
        w.set_position(
            e,
            WorldPos::new(start.0 as f32 + 0.5, 0.0, start.1 as f32 + 0.5),
        );
        let goal_pos = WorldPos::new(goal.0 as f32 + 0.5, 0.0, goal.1 as f32 + 0.5);
        assert!(w.navigate_to(e, goal_pos));
        w.run_ticks(TICKS_PER_SECOND as u64 * 120);
        let p = w.position(e);
        assert!((p.x - goal_pos.x).abs() < 1e-3 && (p.z - goal_pos.z).abs() < 1e-3);
        assert!(!w.is_navigating(e));
    }

    #[test]
    fn navigate_to_blocked_goal_fails() {
        let mut w = World::generate(&WorldConfig {
            seed: 11,
            agent_count: 1,
            behaviors: vec![Behavior::from_seconds(0, 0.0, 0.0, 3.0)],
            ..Default::default()
        });
        let seed = w.seed;
        let c = (0..500)
            .map(|i| (i, 3 * i))
            .find(|&c| !is_walkable(seed, c))
            .expect("unwalkable cell exists");
        assert!(!w.navigate_to(Entity(0), WorldPos::new(c.0 as f32, 0.0, c.1 as f32)));
        assert!(!w.is_navigating(Entity(0)));
    }

    #[test]
    fn wandering_is_deterministic_and_moves_agents() {
        let c = WorldConfig {
            seed: 5,
            agent_count: 50,
            ..Default::default()
        };
        let (mut a, mut b) = (World::generate(&c), World::generate(&c));
        let start = snapshot(&a);
        a.run_ticks(300);
        b.run_ticks(300);
        assert_eq!(snapshot(&a), snapshot(&b));
        assert_ne!(snapshot(&a), start);
    }

    #[test]
    fn path_requests_are_budgeted() {
        let c = WorldConfig {
            seed: 5,
            agent_count: 100,
            max_path_requests_per_tick: 3,
            ..Default::default()
        };
        let mut w = World::generate(&c);
        w.step();
        assert!(w.navigating_count() <= 3);
    }

    #[test]
    fn stationary_behavior_never_moves() {
        let c = WorldConfig {
            seed: 5,
            agent_count: 20,
            behaviors: vec![Behavior::from_seconds(0, 0.0, 0.0, 3.0)],
            ..Default::default()
        };
        let mut w = World::generate(&c);
        let start = snapshot(&w);
        w.run_ticks(200);
        assert_eq!(snapshot(&w), start);
    }

    #[test]
    fn behaviors_are_assigned_round_robin_and_pause_applies() {
        let c = WorldConfig {
            seed: 5,
            agent_count: 4,
            behaviors: vec![
                Behavior::from_seconds(0, 0.0, 0.0, 1.0),
                Behavior::from_seconds(10, 100.0, 100.0, 2.0),
            ],
            ..Default::default()
        };
        let mut w = World::generate(&c);
        let b: Vec<_> = w.entities().map(|e| w.behavior_of(e)).collect();
        assert_eq!(b, vec![0, 1, 0, 1]);
        // Entity 1 finishes a walk, then must pause for the configured 100 s.
        let id = w.ids[1];
        w.ecs.get::<&mut Path>(id).unwrap().0 = Some(VecDeque::new());
        w.step();
        assert!(w.pause_of(Entity(1)) >= 100 * TICKS_PER_SECOND - 1);
        assert!(!w.is_navigating(Entity(1)));
    }
}
