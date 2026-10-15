//! Worker jobs: prioritized per-worker job lists that decide which building a
//! worker staffs. Jobs persist while a building is blocked; the worker returns
//! as soon as the building can be worked again.
//!
//! Cost: the check runs every [`JOB_CHECK_INTERVAL_TICKS`] ticks and only visits
//! workers that have jobs (O(workers x jobs)); per-tick work is untouched.

use std::collections::BTreeMap;

use crate::building::{Buildings, WorkStatus};
use crate::chunk::WorldPos;
use crate::world::{ControlError, Entity, World, TICKS_PER_SECOND};
use serde::{Deserialize, Serialize};

/// How often (in ticks) job assignments are re-evaluated (twice per second).
pub const JOB_CHECK_INTERVAL_TICKS: u64 = (TICKS_PER_SECOND / 2) as u64;
/// Distance (meters) from the slot position within which a worker counts as working.
const ARRIVE_RADIUS: f32 = 0.75;

/// Something a worker can be assigned to. Extend with new variants for new job kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Job {
    /// Staff a worker slot of the building at this index.
    Work { building: usize },
}

/// What a worker is doing right now; drives the (placeholder) work animation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerActivity {
    Idle,
    Working,
}

/// Per-job status shown in the job panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    /// At the slot and working.
    Working,
    /// Assigned to this job and walking to its slot.
    Moving,
    /// Building cannot run (output full / missing input); job is kept.
    Blocked(WorkStatus),
    /// Building is ready but all worker slots are taken.
    NoFreeSlot,
    /// Waiting behind a higher-priority job that is active.
    Queued,
    /// Jobs are suspended after an explicit player move.
    Suspended,
    /// Building no longer exists.
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct ActiveWork {
    building: usize,
    slot: usize,
    arrived: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct WorkerJobs {
    jobs: Vec<Job>,
    suspended: bool,
    active: Option<ActiveWork>,
}

/// Job lists for all workers. Iteration order is by entity for determinism.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Jobs {
    workers: BTreeMap<Entity, WorkerJobs>,
}

impl Jobs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a job (lowest priority) and resume the worker's jobs. Duplicates are ignored.
    pub fn add_job(&mut self, worker: Entity, job: Job) {
        let entry = self.workers.entry(worker).or_default();
        entry.suspended = false;
        if !entry.jobs.contains(&job) {
            entry.jobs.push(job);
        }
    }

    pub fn jobs(&self, worker: Entity) -> &[Job] {
        self.workers.get(&worker).map_or(&[], |w| &w.jobs)
    }

    /// Remove the job at `index`, releasing the slot if it was the active one.
    pub fn remove_job(
        &mut self,
        worker: Entity,
        index: usize,
        buildings: &mut Buildings,
        world: &mut World,
    ) {
        let Some(entry) = self.workers.get_mut(&worker) else {
            return;
        };
        if index >= entry.jobs.len() {
            return;
        }
        let removed = entry.jobs.remove(index);
        let Job::Work { building } = removed;
        if entry.active.is_some_and(|a| a.building == building) {
            Self::release(worker, entry, buildings, world);
        }
    }

    /// Swap the job at `index` with the one above it (higher priority).
    pub fn move_up(&mut self, worker: Entity, index: usize) {
        if let Some(entry) = self.workers.get_mut(&worker) {
            if index > 0 && index < entry.jobs.len() {
                entry.jobs.swap(index, index - 1);
            }
        }
    }

    /// Swap the job at `index` with the one below it (lower priority).
    pub fn move_down(&mut self, worker: Entity, index: usize) {
        if let Some(entry) = self.workers.get_mut(&worker) {
            if index + 1 < entry.jobs.len() {
                entry.jobs.swap(index, index + 1);
            }
        }
    }

    pub fn is_suspended(&self, worker: Entity) -> bool {
        self.workers.get(&worker).is_some_and(|w| w.suspended)
    }

    /// Let a suspended worker go back to their jobs.
    pub fn resume(&mut self, worker: Entity) {
        if let Some(entry) = self.workers.get_mut(&worker) {
            entry.suspended = false;
        }
    }

    pub fn activity(&self, worker: Entity) -> WorkerActivity {
        match self.workers.get(&worker).and_then(|w| w.active) {
            Some(a) if a.arrived => WorkerActivity::Working,
            _ => WorkerActivity::Idle,
        }
    }

    /// Status of the job at `index` for display.
    pub fn job_status(
        &self,
        worker: Entity,
        index: usize,
        buildings: &Buildings,
    ) -> Option<JobStatus> {
        let entry = self.workers.get(&worker)?;
        let Job::Work { building } = *entry.jobs.get(index)?;
        if entry.suspended {
            return Some(JobStatus::Suspended);
        }
        let Some(b) = buildings.buildings.get(building) else {
            return Some(JobStatus::Invalid);
        };
        if let Some(active) = entry.active {
            if active.building == building {
                return Some(if active.arrived {
                    JobStatus::Working
                } else {
                    JobStatus::Moving
                });
            }
        }
        Some(match b.work_status() {
            WorkStatus::Ready if b.has_free_worker_slot() => JobStatus::Queued,
            WorkStatus::Ready => JobStatus::NoFreeSlot,
            blocked => JobStatus::Blocked(blocked),
        })
    }

    /// Explicit player move: the worker leaves their slot and stops taking jobs
    /// until [`Jobs::resume`] (or a new job) is requested.
    pub fn player_move(
        &mut self,
        world: &mut World,
        buildings: &mut Buildings,
        worker: Entity,
        goal: WorldPos,
    ) -> Result<bool, ControlError> {
        if let Some(entry) = self.workers.get_mut(&worker) {
            if entry.active.is_some() || !entry.jobs.is_empty() {
                entry.suspended = true;
            }
            Self::release(worker, entry, buildings, world);
        }
        world.command_move(worker, goal)
    }

    /// Re-evaluate assignments. Cheap no-op except every [`JOB_CHECK_INTERVAL_TICKS`] ticks.
    pub fn update(&mut self, world: &mut World, buildings: &mut Buildings) {
        if self.workers.is_empty() || world.tick % JOB_CHECK_INTERVAL_TICKS != 0 {
            return;
        }
        for building in &mut buildings.buildings {
            building.worked = false;
        }
        self.workers.retain(|&worker, entry| {
            if world.is_alive(worker) {
                return true;
            }
            Self::release(worker, entry, buildings, world);
            false
        });
        for (&worker, entry) in self.workers.iter_mut() {
            if entry.suspended {
                Self::release(worker, entry, buildings, world);
                continue;
            }
            let chosen = entry.jobs.iter().find_map(|job| {
                let Job::Work { building } = *job;
                let b = buildings.buildings.get(building)?;
                let ours = entry.active.is_some_and(|a| a.building == building);
                (b.work_status() == WorkStatus::Ready && (ours || b.has_free_worker_slot()))
                    .then_some(building)
            });
            match chosen {
                None => Self::release(worker, entry, buildings, world),
                Some(building) => {
                    if entry.active.is_some_and(|a| a.building != building) {
                        Self::release(worker, entry, buildings, world);
                    }
                    if entry.active.is_none() {
                        match buildings.assign_worker(building, worker) {
                            Ok(slot) => {
                                entry.active = Some(ActiveWork {
                                    building,
                                    slot,
                                    arrived: false,
                                });
                                world.set_held(worker, true);
                            }
                            Err(_) => continue,
                        }
                    }
                    let Some(active) = entry.active.as_mut() else {
                        continue;
                    };
                    let target = buildings.buildings[building].worker_slot_position(active.slot);
                    let p = world.position(worker);
                    let dist = ((p.x - target.x).powi(2) + (p.z - target.z).powi(2)).sqrt();
                    active.arrived = dist <= ARRIVE_RADIUS;
                    if active.arrived {
                        buildings.buildings[building].worked = true;
                    } else if !world.is_navigating(worker) {
                        world.navigate_to(worker, target);
                    }
                }
            }
        }
    }

    fn release(
        worker: Entity,
        entry: &mut WorkerJobs,
        buildings: &mut Buildings,
        world: &mut World,
    ) {
        if let Some(active) = entry.active.take() {
            let _ = buildings.unassign_worker(active.building, worker);
            if let Some(b) = buildings.buildings.get_mut(active.building) {
                b.worked = false;
            }
        }
        world.set_held(worker, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::building::{BuildingLogistics, Footprint, PlacementRules};
    use crate::world::WorldConfig;

    fn setup() -> (World, Buildings, Entity, usize, usize) {
        let world = World::generate(&WorldConfig {
            agent_count: 2,
            ..Default::default()
        });
        let worker = world.entities().next().unwrap();
        let mut buildings = Buildings::default();
        let mut place = |x: i32| {
            buildings
                .place_with_logistics(
                    world.seed,
                    world.terrain_edits(),
                    "mine",
                    Footprint {
                        x,
                        z: 0,
                        width: 2,
                        depth: 2,
                    },
                    &PlacementRules {
                        building_margin: 0,
                        ..Default::default()
                    },
                    BuildingLogistics {
                        worker_slots: 1,
                        is_mine: true,
                        ..Default::default()
                    },
                )
                .unwrap()
        };
        let a = place(0);
        let b = place(10);
        (world, buildings, worker, a, b)
    }

    /// Teleport the worker to the building's slot and run a check.
    fn arrive_and_update(
        world: &mut World,
        buildings: &mut Buildings,
        jobs: &mut Jobs,
        w: Entity,
        b: usize,
    ) {
        let p = buildings.buildings[b].worker_slot_position(0);
        world.set_position(w, p);
        world.tick = JOB_CHECK_INTERVAL_TICKS * (world.tick / JOB_CHECK_INTERVAL_TICKS + 1);
        jobs.update(world, buildings);
    }

    #[test]
    fn worker_staffs_building_and_it_is_worked() {
        let (mut world, mut buildings, w, a, _) = setup();
        let mut jobs = Jobs::new();
        jobs.add_job(w, Job::Work { building: a });
        arrive_and_update(&mut world, &mut buildings, &mut jobs, w, a);
        assert_eq!(buildings.buildings[a].workers[0], Some(w));
        assert!(buildings.buildings[a].worked);
        assert_eq!(jobs.activity(w), WorkerActivity::Working);
        assert!(world.is_held(w));
        assert_eq!(jobs.job_status(w, 0, &buildings), Some(JobStatus::Working));
        buildings.tick_production(2.0);
        assert_eq!(
            buildings.buildings[a].mine.as_ref().unwrap().output.count,
            1
        );
    }

    #[test]
    fn blocked_building_keeps_job_and_worker_returns() {
        let (mut world, mut buildings, w, a, _) = setup();
        let mut jobs = Jobs::new();
        jobs.add_job(w, Job::Work { building: a });
        let cap = crate::resource::ResourceKind::IronOre.stack_size();
        buildings.buildings[a].mine.as_mut().unwrap().output.count = cap;
        arrive_and_update(&mut world, &mut buildings, &mut jobs, w, a);
        assert_eq!(buildings.buildings[a].workers[0], None);
        assert_eq!(jobs.activity(w), WorkerActivity::Idle);
        assert_eq!(jobs.jobs(w).len(), 1);
        assert_eq!(
            jobs.job_status(w, 0, &buildings),
            Some(JobStatus::Blocked(WorkStatus::OutputFull))
        );
        buildings.buildings[a].mine.as_mut().unwrap().output.take(5);
        arrive_and_update(&mut world, &mut buildings, &mut jobs, w, a);
        assert_eq!(jobs.activity(w), WorkerActivity::Working);
    }

    #[test]
    fn priority_falls_through_to_next_job() {
        let (mut world, mut buildings, w, a, b) = setup();
        let mut jobs = Jobs::new();
        jobs.add_job(w, Job::Work { building: a });
        jobs.add_job(w, Job::Work { building: b });
        let cap = crate::resource::ResourceKind::IronOre.stack_size();
        buildings.buildings[a].mine.as_mut().unwrap().output.count = cap;
        arrive_and_update(&mut world, &mut buildings, &mut jobs, w, b);
        assert_eq!(buildings.buildings[b].workers[0], Some(w));
        assert_eq!(buildings.buildings[a].workers[0], None);
        // Top job becomes workable again: worker switches back.
        buildings.buildings[a].mine.as_mut().unwrap().output.count = 0;
        arrive_and_update(&mut world, &mut buildings, &mut jobs, w, a);
        assert_eq!(buildings.buildings[a].workers[0], Some(w));
        assert_eq!(buildings.buildings[b].workers[0], None);
    }

    #[test]
    fn reorder_changes_priority() {
        let mut jobs = Jobs::new();
        let w = Entity(0);
        jobs.add_job(w, Job::Work { building: 1 });
        jobs.add_job(w, Job::Work { building: 2 });
        jobs.move_up(w, 1);
        assert_eq!(jobs.jobs(w)[0], Job::Work { building: 2 });
        jobs.move_down(w, 0);
        assert_eq!(jobs.jobs(w)[0], Job::Work { building: 1 });
    }

    #[test]
    fn player_move_releases_slot_and_suspends_until_resumed() {
        let (mut world, mut buildings, w, a, _) = setup();
        world.set_player_controlled(w, true);
        let mut jobs = Jobs::new();
        jobs.add_job(w, Job::Work { building: a });
        arrive_and_update(&mut world, &mut buildings, &mut jobs, w, a);
        let goal = WorldPos::new(30.0, 0.0, 30.0);
        jobs.player_move(&mut world, &mut buildings, w, goal)
            .unwrap();
        assert_eq!(buildings.buildings[a].workers[0], None);
        assert!(!world.is_held(w));
        assert!(jobs.is_suspended(w));
        arrive_and_update(&mut world, &mut buildings, &mut jobs, w, a);
        assert_eq!(jobs.activity(w), WorkerActivity::Idle);
        jobs.resume(w);
        arrive_and_update(&mut world, &mut buildings, &mut jobs, w, a);
        assert_eq!(jobs.activity(w), WorkerActivity::Working);
    }

    #[test]
    fn single_slot_is_not_shared() {
        let (mut world, mut buildings, w, a, _) = setup();
        let other = world.entities().nth(1).unwrap();
        let mut jobs = Jobs::new();
        jobs.add_job(w, Job::Work { building: a });
        jobs.add_job(other, Job::Work { building: a });
        arrive_and_update(&mut world, &mut buildings, &mut jobs, w, a);
        assert_eq!(buildings.buildings[a].workers, vec![Some(w)]);
        assert_eq!(
            jobs.job_status(other, 0, &buildings),
            Some(JobStatus::NoFreeSlot)
        );
    }
}
