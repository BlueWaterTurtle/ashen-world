use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

use crate::behavior::Behavior;
use crate::chunk::{ChunkCoord, WorldPos, CHUNK_SIZE};
use crate::npc::{
    DespawnError, EntityKind, Inventory, LodConfig, NavGoal, NpcId, PersistentNpcRecord, SimTier,
    SimulationTier, Skills,
};
use crate::path::{Cell, Pathfinder};
use crate::rng::Rng;
use crate::terrain::{TerrainEdit, TerrainEdits, WalkabilityCache};

/// Fixed simulation rate.
pub const TICKS_PER_SECOND: u32 = 30;
/// Seconds per simulation tick.
pub const TICK_DT: f32 = 1.0 / TICKS_PER_SECOND as f32;

/// Opaque, stable handle to an agent: its spawn index. Spawn order is deterministic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Entity(pub u32);

/// Components stored in the ECS. Every agent always has all of them, so all agents live
/// in one archetype and iteration order equals spawn order (needed for determinism).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Position(pub WorldPos);
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Velocity(pub WorldPos);
/// Remaining waypoints (cell coordinates, in order); `None` when idle.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Path(pub Option<VecDeque<Cell>>);
/// Index into [`World::behaviors`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BehaviorId(pub usize);
/// Ticks remaining before an idle agent may pick a new destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Pause(pub u32);
/// Whether this agent is currently controlled by the player.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PlayerControlled(pub bool);
/// Whether an agent may be recruited into player control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Recruitable(pub bool);

/// Maximum distance (in meters) for character-to-character recruitment.
pub const RECRUIT_MAX_DISTANCE: f32 = 8.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlError {
    NotFound,
    NotControllable,
    NoCharacterSelected,
}

impl std::fmt::Display for ControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "entity not found"),
            Self::NotControllable => write!(f, "entity is not controllable by player"),
            Self::NoCharacterSelected => write!(f, "no player character currently selected"),
        }
    }
}

impl std::error::Error for ControlError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecruitError {
    NotFound,
    AlreadyControlled,
    NotRecruitable,
    RecruiterNotControlled,
    OutOfRange,
}

impl std::fmt::Display for RecruitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "target entity not found"),
            Self::AlreadyControlled => write!(f, "target is already a player character"),
            Self::NotRecruitable => write!(f, "target character is not recruitable"),
            Self::RecruiterNotControlled => {
                write!(f, "recruiting character is not controlled by player")
            }
            Self::OutOfRange => write!(f, "target is out of recruitment interaction range"),
        }
    }
}

impl std::error::Error for RecruitError {}

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
    /// Number of starting characters controlled by the player (default: 1).
    pub starting_player_characters: usize,
    /// Simulation LOD configuration for persistent NPCs.
    pub lod: LodConfig,
}

impl Default for WorldConfig {
    fn default() -> Self {
        Self {
            seed: 1,
            agent_count: 1000,
            chunk_radius: 2,
            behaviors: vec![Behavior::default()],
            max_path_requests_per_tick: 8,
            starting_player_characters: 1,
            lod: LodConfig::default(),
        }
    }
}

impl WorldConfig {
    pub fn with_lod(mut self, lod: LodConfig) -> Self {
        self.lod = lod;
        self
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
    walkability_cache: WalkabilityCache,
    terrain_edits: TerrainEdits,
    pending_terrain_edits: VecDeque<TerrainEdit>,
    pathfinder: Pathfinder,
    /// FIFO of agents that may be ready for a wander request. Agents are enqueued when
    /// they become idle, so per-tick scheduling cost scales with requests served.
    wander_queue: VecDeque<u32>,
    /// Whether each agent is currently in `wander_queue`.
    wander_queued: Vec<bool>,
    /// Agents held in place by an external system (e.g. working a job); they never wander.
    held: std::collections::BTreeSet<u32>,
    /// The currently selected player character, if any.
    pub selected_player_character: Option<Entity>,
    /// Simulation LOD configuration.
    pub lod: LodConfig,
    /// Deterministic focus points driving LOD tier evaluation (e.g. camera, encounters).
    pub focus_points: Vec<WorldPos>,
}

/// Serializable deterministic state needed to restore a [`World`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorldSnapshot {
    pub seed: u64,
    pub tick: u64,
    pub behaviors: Vec<Behavior>,
    pub max_path_requests_per_tick: usize,
    pub rng: Rng,
    pub terrain_edits: Vec<((i32, i32), i32)>,
    pub pending_terrain_edits: Vec<TerrainEdit>,
    pub agents: Vec<AgentSnapshot>,
    pub agent_slots: usize,
    pub wander_queue: VecDeque<u32>,
    pub wander_queued: Vec<bool>,
    pub held: std::collections::BTreeSet<u32>,
    pub selected_player_character: Option<Entity>,
    pub lod: LodConfig,
    pub focus_points: Vec<WorldPos>,
}

/// Runtime state for one live agent, in ECS iteration order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentSnapshot {
    pub entity: Entity,
    pub record: PersistentNpcRecord,
    pub path: Option<VecDeque<Cell>>,
}

/// Maximum A* node expansions per path request.
pub const PATH_BUDGET: usize = 20_000;
/// Pause ticks applied to an agent when a wander path request fails.
pub const WANDER_BACKOFF_TICKS: u32 = 15;
/// Deterministic attempts to pick a walkable wander goal before giving up.
pub const WANDER_GOAL_RETRIES: usize = 8;

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
        let mut walkability_cache = WalkabilityCache::default();
        let starting_players = config.starting_player_characters.min(config.agent_count);
        for i in 0..config.agent_count {
            let pos = loop {
                let cx = (rng.next_u64() % side) as i32 - r;
                let cz = (rng.next_u64() % side) as i32 - r;
                let local = WorldPos::new(
                    rng.range_f32(0.0, CHUNK_SIZE),
                    0.0,
                    rng.range_f32(0.0, CHUNK_SIZE),
                );
                let pos = WorldPos::from_chunk_local(ChunkCoord::new(cx, cz), local);
                let cell = (pos.x.floor() as i32, pos.z.floor() as i32);
                if walkability_cache.is_walkable(config.seed, &TerrainEdits::default(), cell) {
                    let y = crate::terrain::height_at(config.seed, pos.x, pos.z);
                    break WorldPos::new(pos.x, y, pos.z);
                }
            };
            let bid = i % behaviors.len();
            let is_player = i < starting_players;
            let is_recruitable = !is_player && behaviors[bid].recruitable;
            let entity = Entity(i as u32);
            let npc_id = NpcId(i as u64 + 1);
            ids.push(ecs.spawn((
                entity,
                Position(pos),
                Velocity::default(),
                Path::default(),
                BehaviorId(bid),
                Pause::default(),
                PlayerControlled(is_player),
                Recruitable(is_recruitable),
                npc_id,
                EntityKind::PersistentNpc,
                SimTier::Active,
                Skills::default(),
                Inventory::default(),
                NavGoal::default(),
            )));
        }
        let selected_player_character = if starting_players > 0 {
            Some(Entity(0))
        } else {
            None
        };
        let wander_queue: VecDeque<u32> = (0..config.agent_count as u32).collect();
        let mut world = Self {
            seed: config.seed,
            tick: 0,
            ecs,
            ids,
            behaviors,
            max_path_requests_per_tick: config.max_path_requests_per_tick,
            rng,
            walkability_cache,
            terrain_edits: TerrainEdits::default(),
            pending_terrain_edits: VecDeque::new(),
            pathfinder: Pathfinder::new(),
            wander_queue,
            wander_queued: vec![true; config.agent_count],
            held: std::collections::BTreeSet::new(),
            selected_player_character,
            lod: config.lod.clone(),
            focus_points: Vec::new(),
        };
        if world.lod.enabled {
            world.update_simulation_tiers();
        }
        world
    }

    /// Capture all simulation state that can affect future ticks. Terrain generation and
    /// pathfinding caches are derived and intentionally omitted.
    pub fn snapshot(&self) -> WorldSnapshot {
        let mut agents = Vec::with_capacity(self.entity_count());
        for (
            _,
            (
                entity,
                position,
                velocity,
                path,
                behavior_id,
                pause,
                player_controlled,
                recruitable,
                npc_id,
                kind,
                tier,
                skills,
                inventory,
                nav_goal,
            ),
        ) in self
            .ecs
            .query::<(
                &Entity,
                &Position,
                &Velocity,
                &Path,
                &BehaviorId,
                &Pause,
                &PlayerControlled,
                &Recruitable,
                &NpcId,
                &EntityKind,
                &SimTier,
                &Skills,
                &Inventory,
                &NavGoal,
            )>()
            .iter()
        {
            agents.push(AgentSnapshot {
                entity: *entity,
                record: PersistentNpcRecord {
                    id: *npc_id,
                    kind: *kind,
                    position: position.0,
                    velocity: velocity.0,
                    goal: nav_goal.0,
                    behavior_id: behavior_id.0,
                    pause_ticks: pause.0,
                    player_controlled: player_controlled.0,
                    recruitable: recruitable.0,
                    tier: *tier,
                    skills: skills.clone(),
                    inventory: inventory.clone(),
                },
                path: path.0.clone(),
            });
        }
        WorldSnapshot {
            seed: self.seed,
            tick: self.tick,
            behaviors: self.behaviors.clone(),
            max_path_requests_per_tick: self.max_path_requests_per_tick,
            rng: self.rng.clone(),
            terrain_edits: self.terrain_edits.entries().collect(),
            pending_terrain_edits: self.pending_terrain_edits.iter().copied().collect(),
            agents,
            agent_slots: self.ids.len(),
            wander_queue: self.wander_queue.clone(),
            wander_queued: self.wander_queued.clone(),
            held: self.held.clone(),
            selected_player_character: self.selected_player_character,
            lod: self.lod.clone(),
            focus_points: self.focus_points.clone(),
        }
    }

    /// Restore a world snapshot, rejecting malformed entity references rather than panicking.
    pub fn from_snapshot(snapshot: WorldSnapshot) -> Result<Self, String> {
        if snapshot.behaviors.is_empty() {
            return Err("world snapshot has no behaviors".into());
        }
        if snapshot.wander_queued.len() != snapshot.agent_slots {
            return Err("world snapshot has an invalid wander queue size".into());
        }
        if snapshot
            .wander_queue
            .iter()
            .any(|&entity| entity as usize >= snapshot.agent_slots)
            || snapshot
                .held
                .iter()
                .any(|&entity| entity as usize >= snapshot.agent_slots)
        {
            return Err("world snapshot contains an out-of-range agent reference".into());
        }

        let mut ids = vec![hecs::Entity::DANGLING; snapshot.agent_slots];
        let mut present = vec![false; snapshot.agent_slots];
        let mut ecs = hecs::World::new();
        for agent in &snapshot.agents {
            let index = agent.entity.0 as usize;
            if index >= snapshot.agent_slots || present[index] {
                return Err("world snapshot contains an invalid or duplicate agent id".into());
            }
            let record = &agent.record;
            if record.behavior_id >= snapshot.behaviors.len() {
                return Err(format!(
                    "world snapshot agent {} has an invalid behavior id",
                    agent.entity.0
                ));
            }
            let hecs_id = ecs.spawn((
                agent.entity,
                Position(record.position),
                Velocity(record.velocity),
                Path(agent.path.clone()),
                BehaviorId(record.behavior_id),
                Pause(record.pause_ticks),
                PlayerControlled(record.player_controlled),
                Recruitable(record.recruitable),
                record.id,
                record.kind,
                record.tier,
                record.skills.clone(),
                record.inventory.clone(),
                NavGoal(record.goal),
            ));
            ids[index] = hecs_id;
            present[index] = true;
        }
        if let Some(selected) = snapshot.selected_player_character {
            if !present.get(selected.0 as usize).copied().unwrap_or(false)
                || !snapshot
                    .agents
                    .iter()
                    .any(|agent| agent.entity == selected && agent.record.player_controlled)
            {
                return Err("world snapshot has an invalid selected player character".into());
            }
        }

        Ok(Self {
            seed: snapshot.seed,
            tick: snapshot.tick,
            ecs,
            ids,
            behaviors: snapshot.behaviors,
            max_path_requests_per_tick: snapshot.max_path_requests_per_tick,
            rng: snapshot.rng,
            walkability_cache: WalkabilityCache::default(),
            terrain_edits: TerrainEdits::from_entries(snapshot.terrain_edits),
            pending_terrain_edits: snapshot.pending_terrain_edits.into(),
            pathfinder: Pathfinder::new(),
            wander_queue: snapshot.wander_queue,
            wander_queued: snapshot.wander_queued,
            held: snapshot.held,
            selected_player_character: snapshot.selected_player_character,
            lod: snapshot.lod,
            focus_points: snapshot.focus_points,
        })
    }

    pub fn is_alive(&self, e: Entity) -> bool {
        self.ids
            .get(e.0 as usize)
            .is_some_and(|&id| self.ecs.contains(id))
    }

    pub fn entity_count(&self) -> usize {
        self.ids.iter().filter(|&&id| self.ecs.contains(id)).count()
    }

    pub fn entities(&self) -> impl Iterator<Item = Entity> {
        (0..self.ids.len() as u32)
            .filter(|&i| {
                self.ids
                    .get(i as usize)
                    .is_some_and(|&id| self.ecs.contains(id))
            })
            .map(Entity)
            .collect::<Vec<_>>()
            .into_iter()
    }

    pub fn position(&self, e: Entity) -> WorldPos {
        self.ids
            .get(e.0 as usize)
            .and_then(|&id| self.ecs.get::<&Position>(id).ok().map(|p| p.0))
            .unwrap_or_default()
    }

    pub fn set_position(&mut self, e: Entity, p: WorldPos) {
        if let Some(&id) = self.ids.get(e.0 as usize) {
            if let Ok(mut c) = self.ecs.get::<&mut Position>(id) {
                c.0 = p;
            }
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
        self.ids
            .get(e.0 as usize)
            .and_then(|&id| self.ecs.get::<&Path>(id).ok().map(|p| p.0.is_some()))
            .unwrap_or(false)
    }

    pub fn behavior_of(&self, e: Entity) -> usize {
        self.ids
            .get(e.0 as usize)
            .and_then(|&id| self.ecs.get::<&BehaviorId>(id).ok().map(|b| b.0))
            .unwrap_or(0)
    }

    pub fn pause_of(&self, e: Entity) -> u32 {
        self.ids
            .get(e.0 as usize)
            .and_then(|&id| self.ecs.get::<&Pause>(id).ok().map(|p| p.0))
            .unwrap_or(0)
    }

    pub fn set_pause(&mut self, e: Entity, ticks: u32) {
        if let Some(&id) = self.ids.get(e.0 as usize) {
            if let Ok(mut p) = self.ecs.get::<&mut Pause>(id) {
                p.0 = ticks;
            }
        }
        if ticks == 0 {
            self.enqueue_wander(e.0);
        }
    }

    pub fn npc_id(&self, e: Entity) -> Option<NpcId> {
        self.ids
            .get(e.0 as usize)
            .and_then(|&id| self.ecs.get::<&NpcId>(id).ok().map(|n| *n))
    }

    pub fn entity_by_npc_id(&self, target: NpcId) -> Option<Entity> {
        for (_, (entity, id)) in self.ecs.query::<(&Entity, &NpcId)>().iter() {
            if *id == target {
                return Some(*entity);
            }
        }
        None
    }

    pub fn entity_kind(&self, e: Entity) -> Option<EntityKind> {
        self.ids
            .get(e.0 as usize)
            .and_then(|&id| self.ecs.get::<&EntityKind>(id).ok().map(|k| *k))
    }

    pub fn tier(&self, e: Entity) -> SimulationTier {
        self.ids
            .get(e.0 as usize)
            .and_then(|&id| self.ecs.get::<&SimulationTier>(id).ok().map(|t| *t))
            .unwrap_or(SimulationTier::Active)
    }

    pub fn set_tier(&mut self, e: Entity, new_tier: SimulationTier) {
        let current = self.tier(e);
        if current != new_tier {
            self.transition_tier(e, current, new_tier);
        }
    }

    /// Whether an entity's simulation tier permits rendering when within camera frustum.
    ///
    /// Active and Reduced tiers are renderable when visible.
    /// Dormant entities are not rendered until promoted to a higher simulation tier.
    pub fn is_renderable(&self, e: Entity) -> bool {
        self.tier(e).is_renderable()
    }

    /// Whether an entity is marked as a persistent NPC whose state is preserved.
    pub fn is_persistent(&self, e: Entity) -> bool {
        self.entity_kind(e) == Some(EntityKind::PersistentNpc)
    }

    pub fn skills(&self, e: Entity) -> Option<hecs::Ref<'_, Skills>> {
        let id = *self.ids.get(e.0 as usize)?;
        self.ecs.get::<&Skills>(id).ok()
    }

    pub fn skills_mut(&mut self, e: Entity) -> Option<hecs::RefMut<'_, Skills>> {
        let id = *self.ids.get(e.0 as usize)?;
        self.ecs.get::<&mut Skills>(id).ok()
    }

    pub fn inventory(&self, e: Entity) -> Option<hecs::Ref<'_, Inventory>> {
        let id = *self.ids.get(e.0 as usize)?;
        self.ecs.get::<&Inventory>(id).ok()
    }

    pub fn inventory_mut(&mut self, e: Entity) -> Option<hecs::RefMut<'_, Inventory>> {
        let id = *self.ids.get(e.0 as usize)?;
        self.ecs.get::<&mut Inventory>(id).ok()
    }

    pub fn nav_goal(&self, e: Entity) -> Option<WorldPos> {
        self.ids
            .get(e.0 as usize)
            .and_then(|&id| self.ecs.get::<&NavGoal>(id).ok().and_then(|g| g.0))
    }

    pub fn set_nav_goal(&mut self, e: Entity, goal: Option<WorldPos>) {
        if let Some(&id) = self.ids.get(e.0 as usize) {
            if let Ok(mut g) = self.ecs.get::<&mut NavGoal>(id) {
                g.0 = goal;
            }
        }
    }

    pub fn focus_points(&self) -> &[WorldPos] {
        &self.focus_points
    }

    pub fn set_focus_points(&mut self, points: Vec<WorldPos>) {
        self.focus_points = points;
    }

    pub fn add_focus_point(&mut self, point: WorldPos) {
        self.focus_points.push(point);
    }

    pub fn clear_focus_points(&mut self) {
        self.focus_points.clear();
    }

    /// Whether an agent currently has an active waypoint path.
    pub fn has_path(&self, e: Entity) -> bool {
        self.ids
            .get(e.0 as usize)
            .and_then(|&id| self.ecs.get::<&Path>(id).ok().map(|p| p.0.is_some()))
            .unwrap_or(false)
    }

    /// Despawn an entity from active simulation and extract its persistent state.
    pub fn despawn_npc(&mut self, e: Entity) -> Result<PersistentNpcRecord, DespawnError> {
        let idx = e.0 as usize;
        let Some(&id) = self.ids.get(idx) else {
            return Err(DespawnError::NotFound);
        };
        if !self.ecs.contains(id) {
            return Err(DespawnError::AlreadyDespawned);
        }

        let record = PersistentNpcRecord {
            id: self.ecs.get::<&NpcId>(id).map(|i| *i).unwrap_or_default(),
            kind: self
                .ecs
                .get::<&EntityKind>(id)
                .map(|k| *k)
                .unwrap_or_default(),
            position: self
                .ecs
                .get::<&Position>(id)
                .map(|p| p.0)
                .unwrap_or_default(),
            velocity: self
                .ecs
                .get::<&Velocity>(id)
                .map(|v| v.0)
                .unwrap_or_default(),
            goal: self.ecs.get::<&NavGoal>(id).ok().and_then(|g| g.0),
            behavior_id: self.ecs.get::<&BehaviorId>(id).map(|b| b.0).unwrap_or(0),
            pause_ticks: self.ecs.get::<&Pause>(id).map(|p| p.0).unwrap_or(0),
            player_controlled: self
                .ecs
                .get::<&PlayerControlled>(id)
                .map(|p| p.0)
                .unwrap_or(false),
            recruitable: self
                .ecs
                .get::<&Recruitable>(id)
                .map(|r| r.0)
                .unwrap_or(false),
            tier: self.ecs.get::<&SimTier>(id).map(|t| *t).unwrap_or_default(),
            skills: self.skills(e).map(|s| (*s).clone()).unwrap_or_default(),
            inventory: self.inventory(e).map(|i| (*i).clone()).unwrap_or_default(),
        };

        let _ = self.ecs.despawn(id);
        self.ids[idx] = hecs::Entity::DANGLING;
        if idx < self.wander_queued.len() {
            self.wander_queued[idx] = false;
        }

        if self.selected_player_character == Some(e) {
            let next_pc = self.player_characters().next();
            self.selected_player_character = next_pc;
        }

        Ok(record)
    }

    /// Resume a previously despawned persistent NPC into active simulation.
    pub fn resume_npc(&mut self, mut record: PersistentNpcRecord) -> Entity {
        if record.tier == SimTier::Active {
            let cell = (
                record.position.x.floor() as i32,
                record.position.z.floor() as i32,
            );
            if !self
                .walkability_cache
                .is_walkable(self.seed, &self.terrain_edits, cell)
            {
                if let Some(walkable) = self.find_nearest_walkable_cell(cell, 3) {
                    record.position = WorldPos::new(
                        walkable.0 as f32 + 0.5,
                        self.terrain_edits.height(
                            self.seed,
                            walkable.0 as f32 + 0.5,
                            walkable.1 as f32 + 0.5,
                        ),
                        walkable.1 as f32 + 0.5,
                    );
                }
            }
        }

        let (entity, is_new_slot) = if let Some((idx, _)) = self
            .ids
            .iter()
            .enumerate()
            .find(|(_, id)| !self.ecs.contains(**id))
        {
            (Entity(idx as u32), false)
        } else {
            (Entity(self.ids.len() as u32), true)
        };

        let hecs_id = self.ecs.spawn((
            entity,
            Position(record.position),
            Velocity(record.velocity),
            Path::default(),
            BehaviorId(record.behavior_id),
            Pause(record.pause_ticks),
            PlayerControlled(record.player_controlled),
            Recruitable(record.recruitable),
            record.id,
            record.kind,
            record.tier,
            record.skills,
            record.inventory,
            NavGoal(record.goal),
        ));

        if is_new_slot {
            self.ids.push(hecs_id);
            self.wander_queued.push(false);
        } else {
            self.ids[entity.0 as usize] = hecs_id;
            self.wander_queued[entity.0 as usize] = false;
        }

        if record.tier == SimTier::Active {
            if let Some(goal) = record.goal {
                self.navigate_to(entity, goal);
            } else if record.pause_ticks == 0 {
                self.enqueue_wander(entity.0);
            }
        }

        entity
    }

    pub(crate) fn find_nearest_walkable_cell(
        &mut self,
        center: (i32, i32),
        max_radius: i32,
    ) -> Option<(i32, i32)> {
        if self
            .walkability_cache
            .is_walkable(self.seed, &self.terrain_edits, center)
        {
            return Some(center);
        }
        for r in 1..=max_radius {
            for dx in -r..=r {
                for dz in -r..=r {
                    if dx.abs() != r && dz.abs() != r {
                        continue;
                    }
                    let c = (center.0 + dx, center.1 + dz);
                    if self
                        .walkability_cache
                        .is_walkable(self.seed, &self.terrain_edits, c)
                    {
                        return Some(c);
                    }
                }
            }
        }
        None
    }

    fn transition_tier(&mut self, e: Entity, old: SimulationTier, new: SimulationTier) {
        let Some(&id) = self.ids.get(e.0 as usize) else {
            return;
        };
        if !self.ecs.contains(id) {
            return;
        }

        if let Ok(mut t) = self.ecs.get::<&mut SimulationTier>(id) {
            *t = new;
        }

        match (old, new) {
            (SimulationTier::Dormant, SimulationTier::Active)
            | (SimulationTier::Dormant, SimulationTier::Reduced) => {
                let p = self.position(e);
                let cell = (p.x.floor() as i32, p.z.floor() as i32);
                let (cx, cz) = if let Some(walkable) = self.find_nearest_walkable_cell(cell, 3) {
                    walkable
                } else {
                    cell
                };
                let snapped = WorldPos::new(
                    cx as f32 + 0.5,
                    self.terrain_edits
                        .height(self.seed, cx as f32 + 0.5, cz as f32 + 0.5),
                    cz as f32 + 0.5,
                );
                self.set_position(e, snapped);

                let goal = self.nav_goal(e);
                if let Some(goal) = goal {
                    if !self.navigate_to(e, goal) {
                        self.set_pause(e, WANDER_BACKOFF_TICKS);
                        self.set_nav_goal(e, None);
                    }
                } else if self.pause_of(e) == 0 {
                    self.enqueue_wander(e.0);
                }
            }
            (SimulationTier::Active, SimulationTier::Dormant)
            | (SimulationTier::Reduced, SimulationTier::Dormant) => {
                if let Ok(mut path) = self.ecs.get::<&mut Path>(id) {
                    if let Some(wp) = path.0.take() {
                        if let Some(&(cx, cz)) = wp.back() {
                            let dest = WorldPos::new(
                                cx as f32 + 0.5,
                                self.terrain_edits.height(
                                    self.seed,
                                    cx as f32 + 0.5,
                                    cz as f32 + 0.5,
                                ),
                                cz as f32 + 0.5,
                            );
                            if let Ok(mut g) = self.ecs.get::<&mut NavGoal>(id) {
                                g.0 = Some(dest);
                            }
                        }
                    }
                }
                if let Ok(mut vel) = self.ecs.get::<&mut Velocity>(id) {
                    vel.0 = WorldPos::default();
                }
            }
            _ => {}
        }
    }

    pub fn update_simulation_tiers(&mut self) {
        if !self.lod.enabled {
            return;
        }

        let mut focus: Vec<WorldPos> = self.focus_points.clone();
        if self.lod.track_player_focus {
            for &id in &self.ids {
                if self.ecs.contains(id) {
                    if let Ok(pc) = self.ecs.get::<&PlayerControlled>(id) {
                        if pc.0 {
                            if let Ok(pos) = self.ecs.get::<&Position>(id) {
                                focus.push(pos.0);
                            }
                        }
                    }
                }
            }
        }
        if focus.is_empty() {
            focus.push(WorldPos::new(0.0, 0.0, 0.0));
        }

        let active_r_sq = self.lod.active_radius * self.lod.active_radius;
        let reduced_r_sq = self.lod.reduced_radius * self.lod.reduced_radius;

        let mut changes = Vec::new();
        for (_, (entity, pos, pc, tier)) in self
            .ecs
            .query::<(&Entity, &Position, &PlayerControlled, &SimTier)>()
            .iter()
        {
            let target_tier = if pc.0 {
                SimTier::Active
            } else {
                let mut min_d_sq = f32::MAX;
                for f in &focus {
                    let dx = pos.0.x - f.x;
                    let dz = pos.0.z - f.z;
                    let d_sq = dx * dx + dz * dz;
                    if d_sq < min_d_sq {
                        min_d_sq = d_sq;
                    }
                }
                if min_d_sq <= active_r_sq {
                    SimTier::Active
                } else if min_d_sq <= reduced_r_sq {
                    SimTier::Reduced
                } else {
                    SimTier::Dormant
                }
            };

            if target_tier != *tier {
                changes.push((*entity, *tier, target_tier));
            }
        }

        for (e, old_tier, new_tier) in changes {
            self.transition_tier(e, old_tier, new_tier);
        }
    }

    /// Hold (or release) an agent so autonomous wandering skips it. Explicit
    /// navigation still works. Releasing re-queues the agent for wandering.
    pub fn set_held(&mut self, e: Entity, held: bool) {
        if held {
            self.held.insert(e.0);
        } else if self.held.remove(&e.0) {
            self.enqueue_wander(e.0);
        }
    }

    pub fn is_held(&self, e: Entity) -> bool {
        self.held.contains(&e.0)
    }

    fn enqueue_wander(&mut self, i: u32) {
        if let Some(q) = self.wander_queued.get_mut(i as usize) {
            if !*q {
                *q = true;
                self.wander_queue.push_back(i);
            }
        }
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
        let Some(&id) = self.ids.get(e.0 as usize) else {
            return false;
        };
        if !self.ecs.contains(id) {
            return false;
        }
        let p = self.position(e);
        let start = (p.x.floor() as i32, p.z.floor() as i32);
        let goal_cell = (goal.x.floor() as i32, goal.z.floor() as i32);
        let seed = self.seed;
        let cache = &mut self.walkability_cache;
        let edits = &self.terrain_edits;
        match self
            .pathfinder
            .find_path(start, goal_cell, PATH_BUDGET, |c| {
                cache.is_walkable(seed, edits, c)
            }) {
            Some(path) => {
                if let Ok(mut c) = self.ecs.get::<&mut Path>(id) {
                    c.0 = Some(path.into_iter().skip(1).collect());
                }
                if let Ok(mut g) = self.ecs.get::<&mut NavGoal>(id) {
                    g.0 = Some(goal);
                }
                true
            }
            None => false,
        }
    }

    /// Terrain edits applied so far, layered over the procedural heightmap.
    pub fn terrain_edits(&self) -> &TerrainEdits {
        &self.terrain_edits
    }

    /// Replace the terrain edits wholesale (e.g. when loading a save).
    pub fn set_terrain_edits(&mut self, edits: TerrainEdits) {
        self.pending_terrain_edits.clear();
        self.terrain_edits = edits;
        self.walkability_cache = WalkabilityCache::default();
        self.refresh_agents_after_terrain_change();
    }

    /// Queue a terrain edit; it is applied at the start of the next tick, in queue order.
    pub fn queue_terrain_edit(&mut self, edit: TerrainEdit) {
        self.pending_terrain_edits.push_back(edit);
    }

    fn apply_pending_terrain_edits(&mut self) {
        if self.pending_terrain_edits.is_empty() {
            return;
        }
        while let Some(edit) = self.pending_terrain_edits.pop_front() {
            for chunk in self.terrain_edits.apply(self.seed, edit) {
                self.walkability_cache.invalidate(chunk);
            }
        }
        self.refresh_agents_after_terrain_change();
    }

    /// Re-seat agents on the edited surface and re-plan any path that now crosses
    /// unwalkable cells.
    fn refresh_agents_after_terrain_change(&mut self) {
        for e in self.entities() {
            let p = self.position(e);
            let y = self.terrain_edits.height(self.seed, p.x, p.z);
            self.set_position(e, WorldPos::new(p.x, y, p.z));

            let Some(&id) = self.ids.get(e.0 as usize) else {
                continue;
            };
            let waypoints: Vec<Cell> = self
                .ecs
                .get::<&Path>(id)
                .ok()
                .and_then(|path| path.0.as_ref().map(|w| w.iter().copied().collect()))
                .unwrap_or_default();
            let blocked = waypoints.iter().any(|&c| {
                !self
                    .walkability_cache
                    .is_walkable(self.seed, &self.terrain_edits, c)
            });
            if !blocked {
                continue;
            }
            let goal = self.nav_goal(e);
            if let Ok(mut path) = self.ecs.get::<&mut Path>(id) {
                path.0 = None;
            }
            if let Ok(mut v) = self.ecs.get::<&mut Velocity>(id) {
                v.0 = WorldPos::default();
            }
            let replanned = goal.is_some_and(|g| self.navigate_to(e, g));
            if !replanned {
                self.set_nav_goal(e, None);
                self.set_pause(e, WANDER_BACKOFF_TICKS);
            }
        }
    }

    /// Advance the simulation by exactly one fixed tick.
    pub fn step(&mut self) {
        self.apply_pending_terrain_edits();
        if self.lod.enabled {
            self.update_simulation_tiers();
        }
        self.assign_wander_goals();
        self.follow_paths();
        self.tick += 1;
    }

    /// Serve a bounded number of path requests for idle agents, in queue order.
    fn assign_wander_goals(&mut self) {
        let mut requests = 0;
        while requests < self.max_path_requests_per_tick {
            let Some(i) = self.wander_queue.pop_front() else {
                break;
            };
            self.wander_queued[i as usize] = false;
            let e = Entity(i);
            let r = self.behaviors[self.behavior_of(e)].wander_range;
            // Agents that aren't ready are dropped; they re-enter the queue when they
            // become idle again (path finished, pause expired, control released).
            if self.is_player_controlled(e)
                || self.is_held(e)
                || self.is_navigating(e)
                || self.pause_of(e) > 0
                || r == 0
            {
                continue;
            }
            if self.lod.enabled && self.tier(e) == SimTier::Dormant {
                continue;
            }
            requests += 1;
            let p = self.position(e);
            let goal = self.pick_wander_goal(p, r);
            if !self.navigate_to(e, goal) {
                self.set_pause(e, WANDER_BACKOFF_TICKS);
            }
        }
    }

    /// Pick a random goal within `r` cells of `p`, retrying a bounded number of times
    /// to land on a walkable cell. Falls back to the last candidate.
    fn pick_wander_goal(&mut self, p: WorldPos, r: i32) -> WorldPos {
        let span = 2 * r as u64 + 1;
        let mut goal = WorldPos::new(p.x, 0.0, p.z);
        for _ in 0..WANDER_GOAL_RETRIES {
            let dx = (self.rng.next_u64() % span) as i32 - r;
            let dz = (self.rng.next_u64() % span) as i32 - r;
            goal = WorldPos::new(p.x + dx as f32, 0.0, p.z + dz as f32);
            let cell = (goal.x.floor() as i32, goal.z.floor() as i32);
            if self
                .walkability_cache
                .is_walkable(self.seed, &self.terrain_edits, cell)
            {
                break;
            }
        }
        goal
    }

    fn follow_paths(&mut self) {
        let seed = self.seed;
        let edits = &self.terrain_edits;
        let behaviors = &self.behaviors;
        let rng = &mut self.rng;
        let queue = &mut self.wander_queue;
        let queued = &mut self.wander_queued;
        let reduced_interval = self.lod.reduced_tick_interval.max(1);
        let dormant_interval = self.lod.dormant_tick_interval.max(1);

        for (_, (entity, pos, vel, path, bid, pause, tier, nav_goal)) in self
            .ecs
            .query_mut::<(
                &Entity,
                &mut Position,
                &mut Velocity,
                &mut Path,
                &BehaviorId,
                &mut Pause,
                &SimTier,
                &mut NavGoal,
            )>()
            .into_iter()
        {
            let i = entity.0 as usize;
            let b = &behaviors[bid.0];

            match *tier {
                SimTier::Active => {
                    let was_paused = pause.0 > 0;
                    pause.0 = pause.0.saturating_sub(1);
                    let mut enqueue = was_paused && pause.0 == 0 && path.0.is_none();
                    let step_len = b.speed * TICK_DT;
                    let Some(wp) = path.0.as_mut() else {
                        if enqueue && i < queued.len() && !queued[i] {
                            queued[i] = true;
                            queue.push_back(entity.0);
                        }
                        continue;
                    };
                    let Some(&(cx, cz)) = wp.front() else {
                        path.0 = None;
                        nav_goal.0 = None;
                        vel.0 = WorldPos::default();
                        let (min, max) = (b.pause_ticks_min, b.pause_ticks_max);
                        let extra = if max > min {
                            (rng.next_u64() % (max - min + 1) as u64) as u32
                        } else {
                            0
                        };
                        pause.0 = min + extra;
                        if pause.0 == 0 {
                            enqueue = true;
                        }
                        if enqueue && i < queued.len() && !queued[i] {
                            queued[i] = true;
                            queue.push_back(entity.0);
                        }
                        continue;
                    };
                    let (tx, tz) = (cx as f32 + 0.5, cz as f32 + 0.5);
                    let (dx, dz) = (tx - pos.0.x, tz - pos.0.z);
                    let dist = (dx * dx + dz * dz).sqrt();
                    if dist <= step_len {
                        pos.0 = WorldPos::new(tx, edits.height(seed, tx, tz), tz);
                        vel.0 = WorldPos::default();
                        wp.pop_front();
                    } else {
                        vel.0 = WorldPos::new(dx / dist * b.speed, 0.0, dz / dist * b.speed);
                        pos.0.x += vel.0.x * TICK_DT;
                        pos.0.z += vel.0.z * TICK_DT;
                        pos.0.y = edits.height(seed, pos.0.x, pos.0.z);
                    }
                }
                SimTier::Reduced => {
                    let on_tick = (self.tick + entity.0 as u64) % (reduced_interval as u64) == 0;
                    if !on_tick {
                        continue;
                    }
                    let was_paused = pause.0 > 0;
                    pause.0 = pause.0.saturating_sub(reduced_interval);
                    let mut enqueue = was_paused && pause.0 == 0 && path.0.is_none();
                    let step_len = b.speed * TICK_DT * (reduced_interval as f32);
                    let Some(wp) = path.0.as_mut() else {
                        if enqueue && i < queued.len() && !queued[i] {
                            queued[i] = true;
                            queue.push_back(entity.0);
                        }
                        continue;
                    };
                    let Some(&(cx, cz)) = wp.front() else {
                        path.0 = None;
                        nav_goal.0 = None;
                        vel.0 = WorldPos::default();
                        let (min, max) = (b.pause_ticks_min, b.pause_ticks_max);
                        let extra = if max > min {
                            (rng.next_u64() % (max - min + 1) as u64) as u32
                        } else {
                            0
                        };
                        pause.0 = min + extra;
                        if pause.0 == 0 {
                            enqueue = true;
                        }
                        if enqueue && i < queued.len() && !queued[i] {
                            queued[i] = true;
                            queue.push_back(entity.0);
                        }
                        continue;
                    };
                    let (tx, tz) = (cx as f32 + 0.5, cz as f32 + 0.5);
                    let (dx, dz) = (tx - pos.0.x, tz - pos.0.z);
                    let dist = (dx * dx + dz * dz).sqrt();
                    if dist <= step_len {
                        pos.0 = WorldPos::new(tx, edits.height(seed, tx, tz), tz);
                        vel.0 = WorldPos::default();
                        wp.pop_front();
                    } else {
                        vel.0 = WorldPos::new(dx / dist * b.speed, 0.0, dz / dist * b.speed);
                        pos.0.x += vel.0.x * TICK_DT * (reduced_interval as f32);
                        pos.0.z += vel.0.z * TICK_DT * (reduced_interval as f32);
                        pos.0.y = edits.height(seed, pos.0.x, pos.0.z);
                    }
                }
                SimTier::Dormant => {
                    let on_tick = (self.tick + entity.0 as u64) % (dormant_interval as u64) == 0;
                    if !on_tick {
                        continue;
                    }
                    pause.0 = pause.0.saturating_sub(dormant_interval);
                    let step_len = b.speed * TICK_DT * (dormant_interval as f32);

                    if let Some(target) = nav_goal.0 {
                        let dx = target.x - pos.0.x;
                        let dz = target.z - pos.0.z;
                        let dist = (dx * dx + dz * dz).sqrt();
                        if dist <= step_len {
                            pos.0.x = target.x;
                            pos.0.z = target.z;
                            vel.0 = WorldPos::default();
                            nav_goal.0 = None;
                            let (min, max) = (b.pause_ticks_min, b.pause_ticks_max);
                            let extra = if max > min {
                                (rng.next_u64() % (max - min + 1) as u64) as u32
                            } else {
                                0
                            };
                            pause.0 = min + extra;
                        } else {
                            vel.0 = WorldPos::new(dx / dist * b.speed, 0.0, dz / dist * b.speed);
                            pos.0.x += vel.0.x * TICK_DT * (dormant_interval as f32);
                            pos.0.z += vel.0.z * TICK_DT * (dormant_interval as f32);
                        }
                    } else if pause.0 == 0 && b.wander_range > 0 {
                        let span = 2 * b.wander_range as u64 + 1;
                        let dx = (rng.next_u64() % span) as i32 - b.wander_range;
                        let dz = (rng.next_u64() % span) as i32 - b.wander_range;
                        let goal_pos = WorldPos::new(pos.0.x + dx as f32, 0.0, pos.0.z + dz as f32);
                        nav_goal.0 = Some(goal_pos);
                    }
                }
            }
        }
    }

    pub fn is_player_controlled(&self, e: Entity) -> bool {
        if e.0 as usize >= self.ids.len() {
            return false;
        }
        self.ecs
            .get::<&PlayerControlled>(self.ids[e.0 as usize])
            .map(|p| p.0)
            .unwrap_or(false)
    }

    /// Whether this entity is controllable by the player. Uncontrollable characters
    /// cannot receive player commands.
    pub fn is_controllable(&self, e: Entity) -> bool {
        self.is_player_controlled(e)
    }

    pub fn set_player_controlled(&mut self, e: Entity, controlled: bool) {
        if (e.0 as usize) < self.ids.len() {
            if let Ok(mut c) = self
                .ecs
                .get::<&mut PlayerControlled>(self.ids[e.0 as usize])
            {
                c.0 = controlled;
            }
            if !controlled {
                self.enqueue_wander(e.0);
            }
        }
    }

    pub fn is_recruitable(&self, e: Entity) -> bool {
        if e.0 as usize >= self.ids.len() {
            return false;
        }
        if self.is_player_controlled(e) {
            return false;
        }
        self.ecs
            .get::<&Recruitable>(self.ids[e.0 as usize])
            .map(|r| r.0)
            .unwrap_or(false)
    }

    pub fn set_recruitable(&mut self, e: Entity, recruitable: bool) {
        if (e.0 as usize) < self.ids.len() {
            if let Ok(mut c) = self.ecs.get::<&mut Recruitable>(self.ids[e.0 as usize]) {
                c.0 = recruitable;
            }
        }
    }

    /// All player-controlled characters in spawn order.
    pub fn player_characters(&self) -> impl Iterator<Item = Entity> + '_ {
        (0..self.ids.len() as u32)
            .map(Entity)
            .filter(|&e| self.is_player_controlled(e))
    }

    /// Total count of currently player-controlled characters.
    pub fn player_character_count(&self) -> usize {
        self.player_characters().count()
    }

    pub fn selected_character(&self) -> Option<Entity> {
        self.selected_player_character
    }

    pub fn select_character(&mut self, e: Entity) -> Result<(), ControlError> {
        if e.0 as usize >= self.ids.len() {
            return Err(ControlError::NotFound);
        }
        if !self.is_player_controlled(e) {
            return Err(ControlError::NotControllable);
        }
        self.selected_player_character = Some(e);
        Ok(())
    }

    /// Issue a move command to a player-controlled character. Returns `Err(ControlError::NotControllable)`
    /// if the character is uncontrollable.
    pub fn command_move(&mut self, e: Entity, goal: WorldPos) -> Result<bool, ControlError> {
        if e.0 as usize >= self.ids.len() {
            return Err(ControlError::NotFound);
        }
        if !self.is_player_controlled(e) {
            return Err(ControlError::NotControllable);
        }
        Ok(self.navigate_to(e, goal))
    }

    /// Issue a move command to the currently selected player character.
    pub fn command_move_selected(&mut self, goal: WorldPos) -> Result<bool, ControlError> {
        let selected = self
            .selected_player_character
            .ok_or(ControlError::NoCharacterSelected)?;
        self.command_move(selected, goal)
    }

    /// Check if target can be recruited.
    pub fn can_recruit(&self, target: Entity) -> bool {
        self.is_recruitable(target)
    }

    /// Check if target can be recruited by the given recruiter character.
    pub fn can_recruit_with(&self, recruiter: Entity, target: Entity) -> bool {
        if !self.is_player_controlled(recruiter) {
            return false;
        }
        if !self.can_recruit(target) {
            return false;
        }
        let rp = self.position(recruiter);
        let tp = self.position(target);
        let dx = rp.x - tp.x;
        let dz = rp.z - tp.z;
        (dx * dx + dz * dz) <= RECRUIT_MAX_DISTANCE * RECRUIT_MAX_DISTANCE
    }

    /// Recruit a character into player control. The target must exist and be recruitable.
    pub fn recruit(&mut self, target: Entity) -> Result<(), RecruitError> {
        if target.0 as usize >= self.ids.len() {
            return Err(RecruitError::NotFound);
        }
        if self.is_player_controlled(target) {
            return Err(RecruitError::AlreadyControlled);
        }
        if !self.is_recruitable(target) {
            return Err(RecruitError::NotRecruitable);
        }
        let id = self.ids[target.0 as usize];
        if let Ok(mut pc) = self.ecs.get::<&mut PlayerControlled>(id) {
            pc.0 = true;
        }
        if let Ok(mut rc) = self.ecs.get::<&mut Recruitable>(id) {
            rc.0 = false;
        }
        // Stop any current wander path / pause so the new recruit is immediately ready for orders.
        if let Ok(mut path) = self.ecs.get::<&mut Path>(id) {
            path.0 = None;
        }
        if let Ok(mut pause) = self.ecs.get::<&mut Pause>(id) {
            pause.0 = 0;
        }
        if let Ok(mut vel) = self.ecs.get::<&mut Velocity>(id) {
            vel.0 = WorldPos::default();
        }
        if self.selected_player_character.is_none() {
            self.selected_player_character = Some(target);
        }
        Ok(())
    }

    /// Recruit a character into player control using a specific player character as recruiter.
    /// Requires recruiter to be player-controlled and within `RECRUIT_MAX_DISTANCE` of target.
    pub fn recruit_with(&mut self, recruiter: Entity, target: Entity) -> Result<(), RecruitError> {
        if recruiter.0 as usize >= self.ids.len() || !self.is_player_controlled(recruiter) {
            return Err(RecruitError::RecruiterNotControlled);
        }
        if target.0 as usize >= self.ids.len() {
            return Err(RecruitError::NotFound);
        }
        let rp = self.position(recruiter);
        let tp = self.position(target);
        let dx = rp.x - tp.x;
        let dz = rp.z - tp.z;
        if (dx * dx + dz * dz) > RECRUIT_MAX_DISTANCE * RECRUIT_MAX_DISTANCE {
            return Err(RecruitError::OutOfRange);
        }
        self.recruit(target)
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
    use crate::terrain::{height_at, is_walkable};

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
    fn snapshot_round_trip_preserves_state_and_future_ticks() {
        let config = WorldConfig {
            seed: 71,
            agent_count: 12,
            ..Default::default()
        };
        let mut uninterrupted = World::generate(&config);
        uninterrupted.run_ticks(37);
        uninterrupted.queue_terrain_edit(TerrainEdit::Raise {
            center: (12, -8),
            radius: 4,
            amount_mm: 1700,
        });
        uninterrupted.run_ticks(1);
        uninterrupted.queue_terrain_edit(TerrainEdit::Flatten {
            center: (3, 7),
            radius: 2,
            target_mm: 10_000,
        });
        uninterrupted
            .skills_mut(Entity(2))
            .unwrap()
            .set("crafting", 4);
        uninterrupted
            .inventory_mut(Entity(2))
            .unwrap()
            .add("wood", 9);
        uninterrupted.set_held(Entity(3), true);

        let saved = uninterrupted.snapshot();
        let mut loaded = World::from_snapshot(saved.clone()).unwrap();
        assert_eq!(loaded.snapshot(), saved);
        uninterrupted.run_ticks(53);
        loaded.run_ticks(53);
        assert_eq!(loaded.snapshot(), uninterrupted.snapshot());
    }

    #[test]
    fn snapshot_rejects_invalid_agent_references() {
        let mut snapshot = World::generate(&cfg(5)).snapshot();
        snapshot.agents[0].entity = Entity(u32::MAX);
        assert!(matches!(
            World::from_snapshot(snapshot),
            Err(error) if error.contains("invalid or duplicate agent id")
        ));
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

    #[test]
    fn player_starts_with_single_character_and_others_are_uncontrollable() {
        let c = WorldConfig {
            seed: 123,
            agent_count: 50,
            ..Default::default()
        };
        let mut w = World::generate(&c);
        assert_eq!(w.player_character_count(), 1);
        assert_eq!(w.selected_character(), Some(Entity(0)));
        assert!(w.is_player_controlled(Entity(0)));
        assert!(w.is_controllable(Entity(0)));

        for e in w.entities().skip(1) {
            assert!(!w.is_player_controlled(e));
            assert!(!w.is_controllable(e));
            assert_eq!(
                w.command_move(e, WorldPos::new(10.0, 0.0, 10.0)),
                Err(ControlError::NotControllable)
            );
        }
    }

    #[test]
    fn agents_spawn_on_walkable_cells() {
        for seed in [1, 42, 999] {
            let c = WorldConfig {
                seed,
                agent_count: 100,
                ..Default::default()
            };
            let w = World::generate(&c);
            for e in w.entities() {
                let p = w.position(e);
                let cell = (p.x.floor() as i32, p.z.floor() as i32);
                assert!(
                    is_walkable(w.seed, cell),
                    "agent {:?} spawned at non-walkable cell {:?}",
                    e,
                    cell
                );
            }
        }
    }

    #[test]
    fn agent_y_matches_terrain_height_at_spawn_and_during_movement() {
        let c = WorldConfig {
            seed: 42,
            agent_count: 10,
            ..Default::default()
        };
        let mut w = World::generate(&c);
        for e in w.entities() {
            let p = w.position(e);
            assert!(
                (p.y - height_at(w.seed, p.x, p.z)).abs() < 1e-4,
                "spawn height mismatch: pos.y = {}, expected = {}",
                p.y,
                height_at(w.seed, p.x, p.z)
            );
        }

        let e = Entity(0);
        let id = w.ids[0];
        let p = w.position(e);
        let start_cell = (p.x.floor() as i32, p.z.floor() as i32);
        let waypoints: VecDeque<Cell> =
            (1..=10).map(|i| (start_cell.0 + i, start_cell.1)).collect();
        w.ecs.get::<&mut Path>(id).unwrap().0 = Some(waypoints);

        for _ in 0..60 {
            w.step();
            let p = w.position(e);
            assert!(
                (p.y - height_at(w.seed, p.x, p.z)).abs() < 1e-4,
                "movement height mismatch: pos.y = {}, expected = {}",
                p.y,
                height_at(w.seed, p.x, p.z)
            );
        }
    }

    #[test]
    fn failed_wander_goal_backs_off() {
        let c = WorldConfig {
            seed: 42,
            agent_count: 50,
            max_path_requests_per_tick: 50,
            ..Default::default()
        };
        let mut w = World::generate(&c);
        w.step();
        let failed_agents: Vec<_> = w
            .entities()
            .filter(|&e| !w.is_player_controlled(e) && !w.is_navigating(e))
            .collect();
        assert!(
            !failed_agents.is_empty(),
            "expected some wander requests to fail"
        );
        for e in failed_agents {
            assert_eq!(w.pause_of(e), WANDER_BACKOFF_TICKS - 1);
        }
    }

    #[test]
    fn wander_goals_are_walkable() {
        let mut w = World::generate(&cfg(42));
        let (mut walkable, total) = (0, 500);
        for i in 0..total {
            let p = w.position(Entity(i % 50));
            let g = w.pick_wander_goal(p, 12);
            walkable +=
                crate::terrain::is_walkable(42, (g.x.floor() as i32, g.z.floor() as i32)) as usize;
        }
        assert!(walkable * 100 >= total as usize * 95, "{walkable}/{total}");
    }

    #[test]
    fn large_world_is_deterministic() {
        let c = WorldConfig {
            seed: 77,
            agent_count: 2_000,
            ..Default::default()
        };
        let (mut a, mut b) = (World::generate(&c), World::generate(&c));
        a.run_ticks(300);
        b.run_ticks(300);
        assert_eq!(snapshot(&a), snapshot(&b));
        assert!(a.navigating_count() > 0);
    }

    /// Run with `cargo test -p core_sim --release -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn bench_step_timing() {
        for n in [500, 2_000, 10_000] {
            let mut w = World::generate(&WorldConfig {
                seed: 1,
                agent_count: n,
                ..Default::default()
            });
            let t = std::time::Instant::now();
            w.run_ticks(300);
            println!("{n} agents: {:?} per tick", t.elapsed() / 300);
        }
    }

    #[test]
    fn player_characters_do_not_wander_autonomously() {
        let c = WorldConfig {
            seed: 7,
            agent_count: 20,
            behaviors: vec![Behavior::from_seconds(30, 0.0, 0.0, 3.0)],
            ..Default::default()
        };
        let mut w = World::generate(&c);
        let player_start = w.position(Entity(0));
        let npc_starts: Vec<_> = w.entities().skip(1).map(|e| w.position(e)).collect();

        w.run_ticks(TICKS_PER_SECOND as u64 * 10);

        // Player character stayed in place
        assert_eq!(w.position(Entity(0)), player_start);

        // At least some NPCs moved from autonomous wandering
        let npc_current: Vec<_> = w.entities().skip(1).map(|e| w.position(e)).collect();
        assert_ne!(npc_starts, npc_current);
    }

    #[test]
    fn recruitment_converts_npc_to_player_control() {
        let c = WorldConfig {
            seed: 42,
            agent_count: 5,
            behaviors: vec![
                Behavior::from_seconds(0, 0.0, 0.0, 3.0).with_recruitable(true),
                Behavior::from_seconds(0, 0.0, 0.0, 3.0).with_recruitable(false),
            ],
            ..Default::default()
        };
        let mut w = World::generate(&c);

        let recruitable_npc = Entity(2); // 2 % 2 == 0 -> recruitable
        let unrecruitable_npc = Entity(1); // 1 % 2 == 1 -> not recruitable

        assert!(w.is_recruitable(recruitable_npc));
        assert!(!w.is_recruitable(unrecruitable_npc));

        // Attempting to recruit unrecruitable fails
        assert_eq!(
            w.recruit(unrecruitable_npc),
            Err(RecruitError::NotRecruitable)
        );
        assert!(!w.is_player_controlled(unrecruitable_npc));

        // Recruiting recruitable succeeds
        assert_eq!(w.recruit(recruitable_npc), Ok(()));
        assert!(w.is_player_controlled(recruitable_npc));
        assert_eq!(w.player_character_count(), 2);
        assert!(!w.is_recruitable(recruitable_npc));

        // Cannot recruit already controlled character
        assert_eq!(
            w.recruit(recruitable_npc),
            Err(RecruitError::AlreadyControlled)
        );

        // Now newly recruited character can receive player commands
        assert!(w.select_character(recruitable_npc).is_ok());
        assert_eq!(w.selected_character(), Some(recruitable_npc));
    }

    #[test]
    fn recruit_with_enforces_distance_and_recruiter_status() {
        let c = WorldConfig {
            seed: 99,
            agent_count: 3,
            behaviors: vec![Behavior::from_seconds(0, 0.0, 0.0, 3.0).with_recruitable(true)],
            ..Default::default()
        };
        let mut w = World::generate(&c);
        let player = Entity(0);
        let target = Entity(1);
        let other_npc = Entity(2);

        // Put player at origin and target far away
        w.set_position(player, WorldPos::new(0.0, 0.0, 0.0));
        w.set_position(target, WorldPos::new(50.0, 0.0, 50.0));

        // Out of range
        assert!(!w.can_recruit_with(player, target));
        assert_eq!(
            w.recruit_with(player, target),
            Err(RecruitError::OutOfRange)
        );

        // Non-player character cannot act as recruiter
        assert_eq!(
            w.recruit_with(other_npc, target),
            Err(RecruitError::RecruiterNotControlled)
        );

        // Move target close to player (within RECRUIT_MAX_DISTANCE = 8.0)
        w.set_position(target, WorldPos::new(3.0, 0.0, 2.0));
        assert!(w.can_recruit_with(player, target));
        assert_eq!(w.recruit_with(player, target), Ok(()));
        assert!(w.is_player_controlled(target));
    }

    #[test]
    fn skills_and_inventory_persist_across_tier_changes() {
        let mut cfg = WorldConfig {
            seed: 12345,
            agent_count: 2,
            ..Default::default()
        };
        cfg.lod = LodConfig::with_radii(30.0, 80.0);
        let mut w = World::generate(&cfg);

        // Clear player focus so we can control focus points precisely
        w.set_player_controlled(Entity(0), false);
        w.set_focus_points(vec![WorldPos::new(0.0, 0.0, 0.0)]);

        let target = Entity(1);
        if let Some(mut skills) = w.skills_mut(target) {
            skills.set("swordsmanship", 25);
            skills.set("survival", 10);
        }
        if let Some(mut inv) = w.inventory_mut(target) {
            inv.add("iron_sword", 1);
            inv.add("bread", 5);
        }

        // Active range (< 30.0)
        w.set_position(target, WorldPos::new(10.0, 0.0, 10.0));
        w.step();
        assert_eq!(w.tier(target), SimTier::Active);
        assert_eq!(w.skills(target).unwrap().get("swordsmanship"), 25);
        assert_eq!(w.inventory(target).unwrap().count_of("bread"), 5);

        // Reduced range (30.0 .. 80.0)
        w.set_position(target, WorldPos::new(50.0, 0.0, 0.0));
        w.step();
        assert_eq!(w.tier(target), SimTier::Reduced);
        assert_eq!(w.skills(target).unwrap().get("swordsmanship"), 25);
        assert_eq!(w.inventory(target).unwrap().count_of("bread"), 5);

        // Dormant range (> 80.0)
        w.set_position(target, WorldPos::new(150.0, 0.0, 0.0));
        w.step();
        assert_eq!(w.tier(target), SimTier::Dormant);
        assert_eq!(w.skills(target).unwrap().get("swordsmanship"), 25);
        assert_eq!(w.inventory(target).unwrap().count_of("bread"), 5);

        // Return to Active
        w.set_position(target, WorldPos::new(5.0, 0.0, 5.0));
        w.step();
        assert_eq!(w.tier(target), SimTier::Active);
        assert_eq!(w.skills(target).unwrap().get("swordsmanship"), 25);
        assert_eq!(w.inventory(target).unwrap().count_of("bread"), 5);
        assert_eq!(w.inventory(target).unwrap().count_of("iron_sword"), 1);
    }

    #[test]
    fn simulation_lod_is_deterministic_across_identical_worlds() {
        let mut cfg = WorldConfig {
            seed: 42,
            agent_count: 20,
            ..Default::default()
        };
        cfg.lod = LodConfig::with_radii(20.0, 60.0);

        let mut w1 = World::generate(&cfg);
        let mut w2 = World::generate(&cfg);

        w1.set_focus_points(vec![WorldPos::new(0.0, 0.0, 0.0)]);
        w2.set_focus_points(vec![WorldPos::new(0.0, 0.0, 0.0)]);

        for _ in 0..100 {
            w1.step();
            w2.step();
        }

        let e1: Vec<_> = w1.entities().collect();
        let e2: Vec<_> = w2.entities().collect();
        assert_eq!(e1, e2);

        for e in e1 {
            let p1 = w1.position(e);
            let p2 = w2.position(e);
            assert_eq!(p1.x.to_bits(), p2.x.to_bits());
            assert_eq!(p1.y.to_bits(), p2.y.to_bits());
            assert_eq!(p1.z.to_bits(), p2.z.to_bits());

            assert_eq!(w1.tier(e), w2.tier(e));
            assert_eq!(w1.npc_id(e), w2.npc_id(e));
            assert_eq!(w1.skills(e).as_deref(), w2.skills(e).as_deref());
            assert_eq!(w1.inventory(e).as_deref(), w2.inventory(e).as_deref());
        }
    }

    #[test]
    fn dormant_agent_advances_toward_goal_without_pathfinding_and_resumes() {
        let mut cfg = WorldConfig {
            seed: 999,
            agent_count: 2,
            ..Default::default()
        };
        cfg.lod = LodConfig::with_radii(25.0, 50.0);
        let mut w = World::generate(&cfg);

        w.set_player_controlled(Entity(0), false);
        w.set_focus_points(vec![WorldPos::new(0.0, 0.0, 0.0)]);

        let e = Entity(1);
        // Start far outside reduced radius (150m)
        let start_pos = WorldPos::new(150.0, 0.0, 150.0);
        w.set_position(e, start_pos);
        w.step();
        assert_eq!(w.tier(e), SimTier::Dormant);

        // Set distant navigation goal
        let goal_pos = WorldPos::new(180.0, 0.0, 150.0);
        w.set_nav_goal(e, Some(goal_pos));

        // Dormant agent must not have an A* path allocated
        assert!(!w.has_path(e));

        // Advance 20 ticks
        for _ in 0..20 {
            w.step();
        }

        // Pos should have advanced along the X direction towards goal
        let mid_pos = w.position(e);
        assert!(mid_pos.x > start_pos.x);
        assert!(!w.has_path(e)); // Still no A* path in Dormant tier

        // Now move focus point to the agent to promote it to Active
        w.set_focus_points(vec![mid_pos]);
        w.step();

        assert_eq!(w.tier(e), SimTier::Active);
        // It cleanly resumed into active simulation
        assert!(w.position(e).x >= mid_pos.x);
    }

    #[test]
    fn despawn_and_resume_npc_preserves_identity_and_state() {
        let cfg = WorldConfig {
            seed: 777,
            agent_count: 3,
            ..Default::default()
        };
        let mut w = World::generate(&cfg);

        let target = Entity(1);
        let npc_id = w.npc_id(target).unwrap();
        if let Some(mut skills) = w.skills_mut(target) {
            skills.set("archery", 50);
        }
        if let Some(mut inv) = w.inventory_mut(target) {
            inv.add("arrow", 30);
            inv.add("bow", 1);
        }
        let pos_before = w.position(target);

        // Despawn the target NPC
        let record = w.despawn_npc(target).expect("despawn should succeed");
        assert_eq!(record.id, npc_id);
        assert_eq!(record.skills.get("archery"), 50);
        assert_eq!(record.inventory.count_of("arrow"), 30);

        // Double despawn returns AlreadyDespawned
        assert_eq!(w.despawn_npc(target), Err(DespawnError::AlreadyDespawned));

        // Active entities count should be 2
        assert_eq!(w.entities().count(), 2);
        assert!(!w.entities().any(|e| e == target));

        // Resume the NPC
        let resumed_entity = w.resume_npc(record);
        assert_eq!(w.entities().count(), 3);
        assert_eq!(w.npc_id(resumed_entity), Some(npc_id));
        assert_eq!(w.skills(resumed_entity).unwrap().get("archery"), 50);
        assert_eq!(w.inventory(resumed_entity).unwrap().count_of("arrow"), 30);
        let pos_after = w.position(resumed_entity);
        assert_eq!(pos_after.x.to_bits(), pos_before.x.to_bits());
        assert_eq!(pos_after.z.to_bits(), pos_before.z.to_bits());

        // Step simulation to ensure resumed entity simulates normally
        w.step();
    }

    #[test]
    fn deterministic_tier_transitions_and_focus_tracking() {
        let mut cfg = WorldConfig {
            seed: 1234,
            agent_count: 5,
            ..Default::default()
        };
        cfg.lod = LodConfig {
            enabled: true,
            active_radius: 40.0,
            reduced_radius: 100.0,
            reduced_tick_interval: 4,
            dormant_tick_interval: 8,
            track_player_focus: true,
        };
        let mut w = World::generate(&cfg);

        // Player is entity 0; set at (0, 0, 0)
        let player = Entity(0);
        assert!(w.is_player_controlled(player));
        w.set_position(player, WorldPos::new(0.0, 0.0, 0.0));

        let npc_near = Entity(1);
        let npc_mid = Entity(2);
        let npc_far = Entity(3);

        w.set_player_controlled(npc_near, false);
        w.set_player_controlled(npc_mid, false);
        w.set_player_controlled(npc_far, false);

        // Within active radius (d = 30 < 40)
        w.set_position(npc_near, WorldPos::new(30.0, 0.0, 0.0));
        // Within reduced radius (40 < d = 70 < 100)
        w.set_position(npc_mid, WorldPos::new(70.0, 0.0, 0.0));
        // Beyond reduced radius (d = 150 > 100)
        w.set_position(npc_far, WorldPos::new(150.0, 0.0, 0.0));

        w.step();

        assert_eq!(w.tier(player), SimulationTier::Active);
        assert_eq!(w.tier(npc_near), SimulationTier::Active);
        assert_eq!(w.tier(npc_mid), SimulationTier::Reduced);
        assert_eq!(w.tier(npc_far), SimulationTier::Dormant);

        // Renderability check: Active and Reduced are renderable; Dormant is not
        assert!(w.is_renderable(player));
        assert!(w.is_renderable(npc_near));
        assert!(w.is_renderable(npc_mid));
        assert!(!w.is_renderable(npc_far));

        // Now move player towards npc_far (to 140, 0, 0)
        w.set_position(player, WorldPos::new(140.0, 0.0, 0.0));
        w.step();

        // Player focus shifted: npc_far is now close to player (d = 10 < 40), so promoted to Active!
        // npc_near is now far (d = 110 > 100), demoted to Dormant!
        // npc_mid is d = 70 (40 < d < 100), so Reduced!
        assert_eq!(w.tier(player), SimulationTier::Active);
        assert_eq!(w.tier(npc_far), SimulationTier::Active);
        assert_eq!(w.tier(npc_near), SimulationTier::Dormant);
        assert_eq!(w.tier(npc_mid), SimulationTier::Reduced);

        assert!(w.is_renderable(npc_far));
        assert!(!w.is_renderable(npc_near));
    }

    #[test]
    fn state_persistence_across_full_tier_cycle() {
        let mut cfg = WorldConfig {
            seed: 8888,
            agent_count: 2,
            ..Default::default()
        };
        cfg.lod = LodConfig::with_radii(25.0, 60.0);
        let mut w = World::generate(&cfg);

        w.set_player_controlled(Entity(0), false);
        w.set_focus_points(vec![WorldPos::new(0.0, 0.0, 0.0)]);

        let npc = Entity(1);
        let id = w.npc_id(npc).unwrap();

        // Populate skills and inventory
        if let Some(mut s) = w.skills_mut(npc) {
            s.set("farming", 15);
            s.set("crafting", 42);
        }
        if let Some(mut inv) = w.inventory_mut(npc) {
            inv.add("wheat", 50);
            inv.add("iron_ingot", 8);
        }
        let test_goal = WorldPos::new(120.0, 0.0, 120.0);
        w.set_nav_goal(npc, Some(test_goal));

        // Cycle through Active -> Reduced -> Dormant -> Reduced -> Active
        let positions = [
            (WorldPos::new(10.0, 0.0, 0.0), SimulationTier::Active),
            (WorldPos::new(45.0, 0.0, 0.0), SimulationTier::Reduced),
            (WorldPos::new(100.0, 0.0, 0.0), SimulationTier::Dormant),
            (WorldPos::new(40.0, 0.0, 0.0), SimulationTier::Reduced),
            (WorldPos::new(5.0, 0.0, 0.0), SimulationTier::Active),
        ];

        for (pos, expected_tier) in positions {
            w.set_position(npc, pos);
            w.step();
            assert_eq!(w.tier(npc), expected_tier);
            assert_eq!(w.npc_id(npc), Some(id));
            assert_eq!(w.skills(npc).unwrap().get("farming"), 15);
            assert_eq!(w.skills(npc).unwrap().get("crafting"), 42);
            assert_eq!(w.inventory(npc).unwrap().count_of("wheat"), 50);
            assert_eq!(w.inventory(npc).unwrap().count_of("iron_ingot"), 8);
        }
    }

    #[test]
    fn dormant_resume_into_active_simulation_with_terrain_alignment_and_path() {
        let mut cfg = WorldConfig {
            seed: 54321,
            agent_count: 2,
            ..Default::default()
        };
        cfg.lod = LodConfig::with_radii(30.0, 70.0);
        let mut w = World::generate(&cfg);

        w.set_player_controlled(Entity(0), false);
        w.set_focus_points(vec![WorldPos::new(0.0, 0.0, 0.0)]);

        let npc = Entity(1);
        // Start far in Dormant zone
        w.set_position(npc, WorldPos::new(200.0, 0.0, 200.0));
        let goal = WorldPos::new(205.0, 0.0, 200.0);
        w.set_nav_goal(npc, Some(goal));

        w.step();
        assert_eq!(w.tier(npc), SimulationTier::Dormant);
        assert!(!w.has_path(npc));

        // Promote to Active by moving focus point directly to the NPC
        w.set_focus_points(vec![WorldPos::new(200.0, 0.0, 200.0)]);
        w.step();

        assert_eq!(w.tier(npc), SimulationTier::Active);
        // Position must be aligned with height
        let pos = w.position(npc);
        let expected_y = height_at(w.seed, pos.x, pos.z);
        assert_eq!(pos.y.to_bits(), expected_y.to_bits());

        // Upon promotion with a goal, pathfinding should have been planned
        assert!(w.has_path(npc));

        // Step active simulation until it arrives or advances
        for _ in 0..50 {
            w.step();
        }
        let end_pos = w.position(npc);
        assert!(end_pos.x > 200.0);
    }

    #[test]
    fn deterministic_1000_agents_with_simulation_tiers() {
        let mut cfg = WorldConfig {
            seed: 9876,
            agent_count: 1_000,
            ..Default::default()
        };
        cfg.lod = LodConfig {
            enabled: true,
            active_radius: 50.0,
            reduced_radius: 120.0,
            reduced_tick_interval: 4,
            dormant_tick_interval: 10,
            track_player_focus: true,
        };

        let mut w1 = World::generate(&cfg);
        let mut w2 = World::generate(&cfg);

        w1.set_focus_points(vec![WorldPos::new(0.0, 0.0, 0.0)]);
        w2.set_focus_points(vec![WorldPos::new(0.0, 0.0, 0.0)]);

        // Run both worlds for 60 ticks
        for _ in 0..60 {
            w1.step();
            w2.step();
        }

        let mut active_count = 0;
        let mut reduced_count = 0;
        let mut dormant_count = 0;

        for e in w1.entities() {
            let p1 = w1.position(e);
            let p2 = w2.position(e);
            assert_eq!(p1.x.to_bits(), p2.x.to_bits());
            assert_eq!(p1.y.to_bits(), p2.y.to_bits());
            assert_eq!(p1.z.to_bits(), p2.z.to_bits());

            let t1 = w1.tier(e);
            let t2 = w2.tier(e);
            assert_eq!(t1, t2);
            match t1 {
                SimulationTier::Active => active_count += 1,
                SimulationTier::Reduced => reduced_count += 1,
                SimulationTier::Dormant => dormant_count += 1,
            }
        }

        // Verify all 3 tiers are populated across the 1000 agents
        assert!(active_count > 0, "should have active agents");
        assert!(reduced_count > 0, "should have reduced agents");
        assert!(dormant_count > 0, "should have dormant agents");
        assert_eq!(active_count + reduced_count + dormant_count, 1000);
    }

    #[test]
    fn queued_terrain_edits_apply_on_tick_and_keep_agents_grounded() {
        let config = WorldConfig {
            seed: 42,
            agent_count: 50,
            ..WorldConfig::default()
        };
        let mut w = World::generate(&config);
        let mut twin = World::generate(&config);
        let center = {
            let p = w.position(Entity(0));
            (p.x.floor() as i32, p.z.floor() as i32)
        };
        let edit = TerrainEdit::Raise {
            center,
            radius: 10,
            amount_mm: 6000,
        };
        w.queue_terrain_edit(edit);
        twin.queue_terrain_edit(edit);
        assert!(w.terrain_edits().is_empty());
        for _ in 0..20 {
            w.step();
            twin.step();
        }
        assert!(!w.terrain_edits().is_empty());
        for e in w.entities() {
            let p = w.position(e);
            assert!((p.y - w.terrain_edits().height(w.seed, p.x, p.z)).abs() < 1e-3);
        }
        assert!(w.positions().eq(twin.positions()));
    }
}
