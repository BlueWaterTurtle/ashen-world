//! Worker jobs: prioritized per-worker job lists that decide which building a
//! worker staffs. Jobs persist while a building is blocked; the worker returns
//! as soon as the building can be worked again.
//!
//! Cost: the check runs every [`JOB_CHECK_INTERVAL_TICKS`] ticks and only visits
//! workers that have jobs (O(workers x jobs)); per-tick work is untouched.

use std::collections::{BTreeMap, BTreeSet};

use crate::building::{Buildings, WorkStatus};
use crate::chunk::WorldPos;
use crate::resource::ResourceKind;
use crate::world::{ControlError, Entity, World, TICKS_PER_SECOND};
use serde::{Deserialize, Serialize};

/// How often (in ticks) job assignments are re-evaluated (twice per second).
pub const JOB_CHECK_INTERVAL_TICKS: u64 = (TICKS_PER_SECOND / 2) as u64;
/// Distance (meters) from the slot position within which a worker counts as working.
const ARRIVE_RADIUS: f32 = 0.75;
/// Most items a hauler carries per trip.
pub const HAUL_CARRY: u32 = 10;
/// How far (meters) from a construction site idle survivors look for material.
pub const BUILD_SOURCE_RADIUS: f32 = 30.0;

fn flat_dist(a: WorldPos, b: WorldPos) -> f32 {
    ((a.x - b.x).powi(2) + (a.z - b.z).powi(2)).sqrt()
}

/// The hidden build job an idle survivor takes on for a construction site.
/// `claim` is the amount of material this worker has promised to deliver, so
/// several builders never over-fetch; a claim of 0 marks the finisher.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct AutoBuild {
    site: usize,
    claim: u32,
}

/// Something a worker can be assigned to. Extend with new variants for new job kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Job {
    /// Staff a worker slot of the building at this index.
    Work { building: usize },
    /// Carry `item` from the output of `from` to the input of `to`.
    Haul {
        from: usize,
        to: usize,
        item: ResourceKind,
    },
    /// Reserve construction `site`, fetch its material from the output of `from`,
    /// deliver it and build.
    Build { site: usize, from: usize },
}

impl Job {
    /// The building this job is primarily about (the source for hauling).
    pub fn building(&self) -> usize {
        match *self {
            Job::Work { building } => building,
            Job::Haul { from, .. } | Job::Build { from, .. } => from,
        }
    }
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
    /// Carrying items between buildings.
    Hauling,
    /// Building no longer exists.
    Invalid,
}

enum Chosen {
    Work(usize),
    /// Index of the haul job in the worker's list.
    Haul(usize),
    /// Index of the build job in the worker's list.
    Build(usize),
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
    /// Index into `jobs` of the haul job being carried out, if any.
    #[serde(default)]
    hauling: Option<usize>,
    #[serde(default)]
    auto_build: Option<AutoBuild>,
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
        entry.hauling = None;
        if let Job::Build { site, .. } = removed {
            if let Some(site) = buildings.sites.get_mut(site) {
                if site.builder == Some(worker) {
                    site.builder = None;
                    site.worked = false;
                }
            }
        }
        if let Job::Work { building } = removed {
            if entry.active.is_some_and(|a| a.building == building) {
                Self::release(worker, entry, buildings, world);
            }
        } else if entry.active.is_none() {
            world.set_held(worker, false);
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

    /// Construction site this worker is automatically building, if any.
    pub fn auto_build_site(&self, worker: Entity) -> Option<usize> {
        self.workers.get(&worker)?.auto_build.map(|a| a.site)
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
        let job = *entry.jobs.get(index)?;
        if entry.suspended {
            return Some(JobStatus::Suspended);
        }
        let building = match job {
            Job::Work { building } => building,
            Job::Build { site, from } => {
                let Some(target) = buildings.sites.get(site) else {
                    return Some(JobStatus::Invalid);
                };
                if target.completed.is_some() || buildings.buildings.get(from).is_none() {
                    return Some(JobStatus::Invalid);
                }
                return Some(if entry.hauling == Some(index) {
                    JobStatus::Hauling
                } else if target.builder.is_some_and(|b| b != worker)
                    || buildings.site_needs(site) == 0
                    || buildings.can_supply_site(from, site)
                {
                    JobStatus::Queued
                } else {
                    JobStatus::Blocked(WorkStatus::MissingInput)
                });
            }
            Job::Haul { from, to, item } => {
                if buildings.buildings.get(from).is_none() || buildings.buildings.get(to).is_none()
                {
                    return Some(JobStatus::Invalid);
                }
                return Some(if entry.hauling == Some(index) {
                    JobStatus::Hauling
                } else if buildings.can_haul(from, to, item.id()) {
                    JobStatus::Queued
                } else {
                    JobStatus::Blocked(WorkStatus::MissingInput)
                });
            }
        };
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
            entry.hauling = None;
            Self::drop_auto(worker, entry, buildings);
            Self::release(worker, entry, buildings, world);
        }
        world.command_move(worker, goal)
    }

    /// Re-evaluate assignments. Cheap no-op except every [`JOB_CHECK_INTERVAL_TICKS`] ticks.
    pub fn update(&mut self, world: &mut World, buildings: &mut Buildings) {
        if world.tick % JOB_CHECK_INTERVAL_TICKS != 0 {
            return;
        }
        if self.workers.is_empty() && buildings.sites.iter().all(|s| s.completed.is_some()) {
            return;
        }
        for building in &mut buildings.buildings {
            building.worked = false;
        }
        self.workers.retain(|&worker, entry| {
            if world.is_alive(worker) {
                return true;
            }
            Self::drop_auto(worker, entry, buildings);
            Self::release(worker, entry, buildings, world);
            false
        });
        // A reservation lives only as long as its owner keeps the build job and is not suspended.
        for (index, site) in buildings.sites.iter_mut().enumerate() {
            site.worked = false;
            if let Some(owner) = site.builder {
                let kept = site.completed.is_none()
                    && self.workers.get(&owner).is_some_and(|entry| {
                        !entry.suspended
                            && (entry.auto_build.is_some_and(|a| a.site == index)
                                || entry.jobs.iter().any(
                                    |job| matches!(*job, Job::Build { site, .. } if site == index),
                                ))
                    });
                if !kept {
                    site.builder = None;
                }
            }
        }
        let mut busy = BTreeSet::new();
        for (&worker, entry) in self.workers.iter_mut() {
            let jobs_before = entry.jobs.len();
            entry.jobs.retain(|job| match *job {
                Job::Build { site, .. } => buildings
                    .sites
                    .get(site)
                    .is_some_and(|s| s.completed.is_none()),
                _ => true,
            });
            if entry.jobs.len() != jobs_before {
                entry.hauling = None;
                Self::release(worker, entry, buildings, world);
            }
            if entry.suspended {
                entry.hauling = None;
                Self::drop_auto(worker, entry, buildings);
                Self::release(worker, entry, buildings, world);
                continue;
            }
            let chosen = entry
                .jobs
                .iter()
                .enumerate()
                .find_map(|(index, job)| match *job {
                    Job::Work { building } => {
                        let b = buildings.buildings.get(building)?;
                        let ours = entry.active.is_some_and(|a| a.building == building);
                        (b.work_status() == WorkStatus::Ready && (ours || b.has_free_worker_slot()))
                            .then_some(Chosen::Work(building))
                    }
                    Job::Build { site, from } => {
                        let target = buildings.sites.get(site)?;
                        if target.builder.is_some_and(|b| b != worker) {
                            return None;
                        }
                        let carrying = world
                            .inventory(worker)
                            .is_some_and(|inv| inv.count_of(target.material.id()) > 0);
                        (carrying
                            || buildings.site_needs(site) == 0
                            || buildings.can_supply_site(from, site))
                        .then_some(Chosen::Build(index))
                    }
                    Job::Haul { from, to, item } => {
                        buildings.buildings.get(to)?;
                        let carrying = world
                            .inventory(worker)
                            .is_some_and(|inv| inv.count_of(item.id()) > 0);
                        (carrying || buildings.can_haul(from, to, item.id()))
                            .then_some(Chosen::Haul(index))
                    }
                });
            if chosen.is_some() {
                busy.insert(worker);
                Self::drop_auto(worker, entry, buildings);
            }
            match chosen {
                None => {
                    entry.hauling = None;
                    // Keep the hold on a worker the build pass below is driving.
                    if entry.auto_build.is_none() || entry.active.is_some() {
                        Self::release(worker, entry, buildings, world);
                    }
                }
                Some(Chosen::Haul(index)) => {
                    if entry.active.is_some() {
                        Self::release(worker, entry, buildings, world);
                    }
                    entry.hauling = Some(index);
                    world.set_held(worker, true);
                    let Job::Haul { from, to, item } = entry.jobs[index] else {
                        continue;
                    };
                    let carrying = world
                        .inventory(worker)
                        .is_some_and(|inv| inv.count_of(item.id()) > 0);
                    let stop = if carrying { to } else { from };
                    let target = buildings.buildings[stop].worker_slot_position(0);
                    let p = world.position(worker);
                    let dist = ((p.x - target.x).powi(2) + (p.z - target.z).powi(2)).sqrt();
                    if dist <= ARRIVE_RADIUS {
                        if let Some(mut inventory) = world.inventory_mut(worker) {
                            let _ = if carrying {
                                buildings.haul_drop(to, &mut inventory, item.id())
                            } else {
                                buildings.haul_pickup(
                                    from,
                                    to,
                                    &mut inventory,
                                    item.id(),
                                    HAUL_CARRY,
                                )
                            };
                        }
                    } else if !world.is_navigating(worker) {
                        world.navigate_to(worker, target);
                    }
                }
                Some(Chosen::Build(index)) => {
                    if entry.active.is_some() {
                        Self::release(worker, entry, buildings, world);
                    }
                    entry.hauling = Some(index);
                    world.set_held(worker, true);
                    let Job::Build { site, from } = entry.jobs[index] else {
                        continue;
                    };
                    buildings.sites[site].builder = Some(worker);
                    let material = buildings.sites[site].material.id();
                    let carrying = world
                        .inventory(worker)
                        .is_some_and(|inv| inv.count_of(material) > 0);
                    let at_site = carrying || buildings.site_needs(site) == 0;
                    let target = if at_site {
                        buildings.sites[site].worker_position()
                    } else if let Some(source) = buildings.buildings.get(from) {
                        source.worker_slot_position(0)
                    } else {
                        continue;
                    };
                    let p = world.position(worker);
                    let dist = ((p.x - target.x).powi(2) + (p.z - target.z).powi(2)).sqrt();
                    if dist <= ARRIVE_RADIUS {
                        if let Some(mut inventory) = world.inventory_mut(worker) {
                            let _ = if at_site {
                                buildings.site_deliver(site, &mut inventory)
                            } else {
                                buildings.site_pickup(from, site, &mut inventory, HAUL_CARRY)
                            };
                        }
                        let target = &mut buildings.sites[site];
                        target.worked = at_site && target.delivered >= target.required;
                    } else if !world.is_navigating(worker) {
                        world.navigate_to(worker, target);
                    }
                }
                Some(Chosen::Work(building)) => {
                    entry.hauling = None;
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
        self.auto_build(world, buildings, &busy);
        self.workers.retain(|_, e| {
            !(e.jobs.is_empty()
                && e.active.is_none()
                && e.hauling.is_none()
                && e.auto_build.is_none()
                && !e.suspended)
        });
    }

    fn drop_auto(worker: Entity, entry: &mut WorkerJobs, buildings: &mut Buildings) {
        if let Some(auto) = entry.auto_build.take() {
            if let Some(site) = buildings.sites.get_mut(auto.site) {
                if site.builder == Some(worker) {
                    site.builder = None;
                    site.worked = false;
                }
            }
        }
    }

    /// Idle player survivors take hidden build jobs on open construction sites:
    /// fetch material from nearby stock (or their own inventory), deliver it,
    /// and one of them finishes the building. Workers are visited in entity
    /// order, so the outcome is deterministic.
    fn auto_build(
        &mut self,
        world: &mut World,
        buildings: &mut Buildings,
        busy: &BTreeSet<Entity>,
    ) {
        let manual: BTreeSet<usize> = self
            .workers
            .values()
            .flat_map(|w| w.jobs.iter())
            .filter_map(|job| match *job {
                Job::Build { site, .. } => Some(site),
                _ => None,
            })
            .collect();
        for (&worker, entry) in self.workers.iter_mut() {
            let valid = entry.auto_build.is_some_and(|a| {
                world.is_player_controlled(worker)
                    && !manual.contains(&a.site)
                    && buildings
                        .sites
                        .get(a.site)
                        .is_some_and(|s| s.completed.is_none())
            });
            if entry.auto_build.is_some() && !valid {
                Self::drop_auto(worker, entry, buildings);
                world.set_held(worker, false);
            }
        }
        let candidates: Vec<Entity> = world.player_characters().collect();
        for worker in candidates {
            if busy.contains(&worker) {
                continue;
            }
            let state = self.workers.get(&worker);
            if state.is_some_and(|e| e.suspended) {
                continue;
            }
            let current = state.and_then(|e| e.auto_build);
            if current.is_none() && world.is_navigating(worker) {
                // Following an explicit player move command.
                continue;
            }
            let auto = match current {
                Some(auto) => Some(auto),
                None => self.pick_site(world, buildings, worker, &manual),
            };
            let next = auto.and_then(|auto| Self::step_build(world, buildings, worker, auto));
            match next {
                Some(auto) => {
                    self.workers.entry(worker).or_default().auto_build = Some(auto);
                    world.set_held(worker, true);
                }
                None => {
                    if let Some(entry) = self.workers.get_mut(&worker) {
                        Self::drop_auto(worker, entry, buildings);
                        if entry.active.is_none() && entry.hauling.is_none() {
                            world.set_held(worker, false);
                        }
                    }
                }
            }
        }
    }

    /// The nearest open site that still needs material or a finisher.
    fn pick_site(
        &self,
        world: &World,
        buildings: &Buildings,
        worker: Entity,
        manual: &BTreeSet<usize>,
    ) -> Option<AutoBuild> {
        let from = world.position(worker);
        let mut best: Option<(f32, AutoBuild)> = None;
        for (index, site) in buildings.sites.iter().enumerate() {
            if site.completed.is_some() || manual.contains(&index) {
                continue;
            }
            let claimed: u32 = self
                .workers
                .values()
                .filter_map(|w| w.auto_build)
                .filter(|a| a.site == index)
                .map(|a| a.claim)
                .sum();
            let unclaimed = site
                .required
                .saturating_sub(site.delivered)
                .saturating_sub(claimed);
            let claim = if unclaimed > 0 {
                let carried = world
                    .inventory(worker)
                    .map_or(0, |inv| inv.count_of(site.material.id()));
                if carried > 0 {
                    unclaimed.min(carried)
                } else {
                    let Some(source) = buildings.find_material_source(index, BUILD_SOURCE_RADIUS)
                    else {
                        continue;
                    };
                    let stock = buildings.buildings[source]
                        .outputs
                        .count_of(site.material.id());
                    unclaimed.min(HAUL_CARRY).min(stock)
                }
            } else if site.delivered >= site.required && site.builder.is_none() {
                0
            } else {
                continue;
            };
            let dist = flat_dist(from, site.worker_position());
            if best.is_none_or(|(d, _)| dist < d) {
                best = Some((dist, AutoBuild { site: index, claim }));
            }
        }
        best.map(|(_, auto)| auto)
    }

    /// Advance one worker's build job by one check; `None` ends the job.
    fn step_build(
        world: &mut World,
        buildings: &mut Buildings,
        worker: Entity,
        auto: AutoBuild,
    ) -> Option<AutoBuild> {
        let site = buildings.sites.get(auto.site)?;
        let material = site.material.id();
        let site_pos = site.worker_position();
        let p = world.position(worker);
        if site.delivered >= site.required {
            // Fully supplied: the first worker to get here reserves and finishes it.
            match site.builder {
                Some(b) if b != worker => return None,
                _ => buildings.sites[auto.site].builder = Some(worker),
            }
            if flat_dist(p, site_pos) <= ARRIVE_RADIUS {
                buildings.sites[auto.site].worked = true;
            } else if !world.is_navigating(worker) {
                world.navigate_to(worker, site_pos);
            }
            return Some(AutoBuild { claim: 0, ..auto });
        }
        let carried = world
            .inventory(worker)
            .map_or(0, |inv| inv.count_of(material));
        if carried > 0 {
            if flat_dist(p, site_pos) <= ARRIVE_RADIUS {
                let moved = {
                    let mut inventory = world.inventory_mut(worker)?;
                    buildings
                        .site_deliver(auto.site, &mut inventory)
                        .unwrap_or(0)
                };
                let claim = auto.claim.saturating_sub(moved);
                let site = &buildings.sites[auto.site];
                if claim == 0 && site.delivered < site.required {
                    return None;
                }
                return Some(AutoBuild { claim, ..auto });
            }
            if !world.is_navigating(worker) {
                world.navigate_to(worker, site_pos);
            }
            return Some(auto);
        }
        if auto.claim == 0 {
            return None;
        }
        let source = buildings.find_material_source(auto.site, BUILD_SOURCE_RADIUS)?;
        let source_pos = buildings.buildings[source].worker_slot_position(0);
        if flat_dist(p, source_pos) <= ARRIVE_RADIUS {
            let taken = {
                let mut inventory = world.inventory_mut(worker)?;
                buildings
                    .site_pickup(
                        source,
                        auto.site,
                        &mut inventory,
                        auto.claim.min(HAUL_CARRY),
                    )
                    .unwrap_or(0)
            };
            (taken > 0).then_some(auto)
        } else {
            if !world.is_navigating(worker) {
                world.navigate_to(worker, source_pos);
            }
            Some(auto)
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
    use crate::resource::ResourceKind;
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

    /// Mine (building 0) -> haul -> workshop (building 1) -> ingots in the workshop output.
    fn chain_setup() -> (World, Buildings, Jobs, Entity, Entity) {
        let world = World::generate(&WorldConfig {
            agent_count: 2,
            ..Default::default()
        });
        let mut ids = world.entities();
        let (miner, hauler) = (ids.next().unwrap(), ids.next().unwrap());
        drop(ids);
        let mut buildings = Buildings::default();
        let rules = PlacementRules::default();
        let mut place = |x: i32, def: &str, logistics: BuildingLogistics| {
            buildings
                .place_with_logistics(
                    world.seed,
                    world.terrain_edits(),
                    def,
                    Footprint {
                        x,
                        z: 0,
                        width: 2,
                        depth: 2,
                    },
                    &rules,
                    logistics,
                )
                .unwrap()
        };
        place(
            0,
            "iron_mine",
            BuildingLogistics {
                worker_slots: 1,
                output_slots: 2,
                output_resources: vec!["iron_ore".into()],
                is_mine: true,
                ..Default::default()
            },
        );
        place(
            10,
            "workshop",
            BuildingLogistics {
                worker_slots: 1,
                input_slots: 2,
                output_slots: 2,
                input_resources: vec!["iron_ore".into()],
                output_resources: vec!["iron_ingot".into()],
                required_inputs: vec!["iron_ore".into()],
                is_smelter: true,
                ..Default::default()
            },
        );
        let mut jobs = Jobs::new();
        jobs.add_job(miner, Job::Work { building: 0 });
        jobs.add_job(
            hauler,
            Job::Haul {
                from: 0,
                to: 1,
                item: ResourceKind::IronOre,
            },
        );
        (world, buildings, jobs, miner, hauler)
    }

    fn step(
        world: &mut World,
        buildings: &mut Buildings,
        jobs: &mut Jobs,
        at: [(Entity, usize); 2],
    ) {
        for (who, b) in at {
            let p = buildings.buildings[b].worker_slot_position(0);
            world.set_position(who, p);
        }
        world.tick = JOB_CHECK_INTERVAL_TICKS * (world.tick / JOB_CHECK_INTERVAL_TICKS + 1);
        jobs.update(world, buildings);
        buildings.tick_production(2.0);
    }

    #[test]
    fn haul_moves_ore_and_workshop_smelts_ingots() {
        let (mut world, mut buildings, mut jobs, miner, hauler) = chain_setup();
        // Miner at the mine produces ore into its output.
        for _ in 0..4 {
            step(
                &mut world,
                &mut buildings,
                &mut jobs,
                [(miner, 0), (hauler, 1)],
            );
        }
        let ore = buildings.buildings[0].outputs.count_of("iron_ore");
        assert!(ore > 0);
        // Hauler picks up at the mine, then drops at the workshop.
        step(
            &mut world,
            &mut buildings,
            &mut jobs,
            [(miner, 0), (hauler, 0)],
        );
        assert_eq!(
            jobs.job_status(hauler, 0, &buildings),
            Some(JobStatus::Hauling)
        );
        assert!(world.inventory(hauler).unwrap().count_of("iron_ore") > 0);
        step(
            &mut world,
            &mut buildings,
            &mut jobs,
            [(miner, 0), (hauler, 1)],
        );
        assert_eq!(world.inventory(hauler).unwrap().count_of("iron_ore"), 0);
        assert!(buildings.buildings[1].inputs.count_of("iron_ore") > 0);
        // A worker at the workshop turns ore into ingots.
        jobs.add_job(miner, Job::Work { building: 1 });
        let ingots_before = buildings.buildings[1].outputs.count_of("iron_ingot");
        let ore_before = buildings.buildings[1].inputs.count_of("iron_ore");
        // Miner's mine is top priority; make the workshop first.
        jobs.move_up(miner, 1);
        for _ in 0..3 {
            step(
                &mut world,
                &mut buildings,
                &mut jobs,
                [(miner, 1), (hauler, 1)],
            );
        }
        assert!(buildings.buildings[1].outputs.count_of("iron_ingot") > ingots_before);
        assert!(buildings.buildings[1].inputs.count_of("iron_ore") < ore_before);
    }

    #[test]
    fn chain_is_deterministic_across_clone_and_serde_state() {
        let (mut world, mut buildings, mut jobs, miner, hauler) = chain_setup();
        for _ in 0..6 {
            step(
                &mut world,
                &mut buildings,
                &mut jobs,
                [(miner, 0), (hauler, 0)],
            );
        }
        let mut b2 = buildings.clone();
        let mut j2 = jobs.clone();
        let snapshot = world.snapshot();
        let mut w2 = World::from_snapshot(snapshot).expect("restore");
        for _ in 0..6 {
            step(
                &mut world,
                &mut buildings,
                &mut jobs,
                [(miner, 1), (hauler, 1)],
            );
            step(&mut w2, &mut b2, &mut j2, [(miner, 1), (hauler, 1)]);
        }
        assert_eq!(buildings.buildings, b2.buildings);
    }

    /// A wood store with 30 wood and a requested 15-wood construction site.
    fn construction_setup() -> (World, Buildings, Jobs, Entity, usize, usize) {
        let world = World::generate(&WorldConfig {
            agent_count: 2,
            ..Default::default()
        });
        let worker = world.entities().next().unwrap();
        let mut buildings = Buildings::default();
        let store = buildings
            .place_with_logistics(
                world.seed,
                world.terrain_edits(),
                "wood_storage",
                Footprint {
                    x: 0,
                    z: 0,
                    width: 2,
                    depth: 2,
                },
                &PlacementRules::default(),
                BuildingLogistics {
                    input_slots: 1,
                    output_slots: 1,
                    input_resources: vec!["wood".into()],
                    output_resources: vec!["wood".into()],
                    is_storage: true,
                    ..Default::default()
                },
            )
            .unwrap();
        buildings.buildings[store]
            .outputs
            .insert_resource("wood", 30);
        let site = buildings
            .request_construction(
                world.seed,
                world.terrain_edits(),
                "ore_storage",
                Footprint {
                    x: 10,
                    z: 0,
                    width: 2,
                    depth: 2,
                },
                &PlacementRules::default(),
                BuildingLogistics {
                    input_slots: 1,
                    output_slots: 1,
                    input_resources: vec!["iron_ore".into()],
                    output_resources: vec!["iron_ore".into()],
                    is_storage: true,
                    ..Default::default()
                },
                ResourceKind::Wood,
                15,
            )
            .unwrap();
        let mut jobs = Jobs::new();
        jobs.add_job(worker, Job::Build { site, from: store });
        (world, buildings, jobs, worker, store, site)
    }

    /// Teleport the worker to wherever it should be next and run one job check.
    fn build_step(
        world: &mut World,
        buildings: &mut Buildings,
        jobs: &mut Jobs,
        w: Entity,
        store: usize,
        site: usize,
    ) {
        let carrying = world.inventory(w).is_some_and(|i| i.count_of("wood") > 0);
        let p = if carrying || buildings.site_needs(site) == 0 {
            buildings.sites[site].worker_position()
        } else {
            buildings.buildings[store].worker_slot_position(0)
        };
        world.set_position(w, p);
        world.tick = JOB_CHECK_INTERVAL_TICKS * (world.tick / JOB_CHECK_INTERVAL_TICKS + 1);
        jobs.update(world, buildings);
        buildings.tick_production(2.0);
    }

    #[test]
    fn construction_request_to_completion() {
        let (mut world, mut buildings, mut jobs, w, store, site) = construction_setup();
        assert_eq!(buildings.buildings.len(), 1);
        // The reserved site cannot be requested twice.
        assert!(buildings
            .request_construction(
                world.seed,
                world.terrain_edits(),
                "ore_storage",
                buildings.sites[site].footprint,
                &PlacementRules::default(),
                BuildingLogistics::default(),
                ResourceKind::Wood,
                1,
            )
            .is_err());
        for _ in 0..40 {
            build_step(&mut world, &mut buildings, &mut jobs, w, store, site);
            if buildings.sites[site].completed.is_some() {
                break;
            }
        }
        let built = buildings.sites[site].completed.expect("site completed");
        assert_eq!(buildings.buildings[built].def, "ore_storage");
        assert!(buildings.buildings[built].is_storage);
        assert_eq!(buildings.buildings[built].footprint.x, 10);
        assert_eq!(buildings.buildings[store].outputs.count_of("wood"), 15);
        assert_eq!(buildings.sites[site].delivered, 15);
        assert!(buildings.blocks((10, 0)));
        // Next check drops the finished job and frees the worker.
        build_step(&mut world, &mut buildings, &mut jobs, w, store, site);
        assert!(jobs.jobs(w).is_empty());
        assert!(!world.is_held(w));
    }

    #[test]
    fn construction_waits_for_missing_materials_before_progressing() {
        let (mut world, mut buildings, mut jobs, w, store, site) = construction_setup();
        assert_eq!(
            buildings.buildings[store]
                .outputs
                .remove_resource("wood", 20),
            20
        );

        build_step(&mut world, &mut buildings, &mut jobs, w, store, site);
        build_step(&mut world, &mut buildings, &mut jobs, w, store, site);
        assert_eq!(buildings.sites[site].delivered, 10);
        assert_eq!(buildings.site_needs(site), 5);
        assert_eq!(buildings.sites[site].progress, 0.0);
        assert_eq!(buildings.sites[site].completed, None);

        build_step(&mut world, &mut buildings, &mut jobs, w, store, site);
        assert_eq!(
            jobs.job_status(w, 0, &buildings),
            Some(JobStatus::Blocked(WorkStatus::MissingInput))
        );
        assert_eq!(buildings.sites[site].progress, 0.0);

        buildings.buildings[store]
            .outputs
            .insert_resource("wood", 5);
        for _ in 0..3 {
            build_step(&mut world, &mut buildings, &mut jobs, w, store, site);
        }
        assert_eq!(buildings.sites[site].delivered, 15);
        assert!(buildings.sites[site].progress > 0.0);
        assert!(buildings.sites[site].completed.is_some());
    }

    #[test]
    fn site_is_reserved_by_one_worker() {
        let (world, mut buildings, mut jobs, w, store, site) = construction_setup();
        let mut world = world;
        let other = world.entities().nth(1).unwrap();
        jobs.add_job(other, Job::Build { site, from: store });
        world.set_position(w, buildings.buildings[store].worker_slot_position(0));
        world.set_position(other, buildings.buildings[store].worker_slot_position(0));
        world.tick = JOB_CHECK_INTERVAL_TICKS;
        jobs.update(&mut world, &mut buildings);
        assert_eq!(buildings.sites[site].builder, Some(w));
        assert_eq!(world.inventory(w).unwrap().count_of("wood"), HAUL_CARRY);
        assert_eq!(world.inventory(other).unwrap().count_of("wood"), 0);
        // Removing the owner's job releases the reservation.
        jobs.remove_job(w, 0, &mut buildings, &mut world);
        assert_eq!(buildings.sites[site].builder, None);
    }

    #[test]
    fn construction_is_deterministic_across_clone() {
        let (mut world, mut buildings, mut jobs, w, store, site) = construction_setup();
        for _ in 0..4 {
            build_step(&mut world, &mut buildings, &mut jobs, w, store, site);
        }
        assert!(buildings.sites[site].completed.is_none());
        let (mut b2, mut j2) = (buildings.clone(), jobs.clone());
        let mut w2 = World::from_snapshot(world.snapshot()).expect("restore");
        for _ in 0..8 {
            build_step(&mut world, &mut buildings, &mut jobs, w, store, site);
            build_step(&mut w2, &mut b2, &mut j2, w, store, site);
        }
        assert_eq!(buildings.sites, b2.sites);
        assert_eq!(buildings.buildings, b2.buildings);
        assert!(buildings.sites[site].completed.is_some());
    }

    /// Three player survivors, an ore store with 5 ore and a one-ore site.
    fn auto_setup() -> (World, Buildings, Jobs, Vec<Entity>, usize, usize) {
        let mut world = World::generate(&WorldConfig {
            agent_count: 3,
            ..Default::default()
        });
        let workers: Vec<Entity> = world.entities().take(3).collect();
        for &w in &workers {
            world.set_player_controlled(w, true);
        }
        let mut buildings = Buildings::default();
        let store = buildings
            .place_with_logistics(
                world.seed,
                world.terrain_edits(),
                "ore_storage",
                Footprint {
                    x: 0,
                    z: 0,
                    width: 2,
                    depth: 2,
                },
                &PlacementRules::default(),
                BuildingLogistics {
                    input_slots: 1,
                    output_slots: 1,
                    input_resources: vec!["iron_ore".into()],
                    output_resources: vec!["iron_ore".into()],
                    is_storage: true,
                    ..Default::default()
                },
            )
            .unwrap();
        buildings.buildings[store]
            .outputs
            .insert_resource("iron_ore", 5);
        let site = buildings
            .request_construction(
                world.seed,
                world.terrain_edits(),
                "house",
                Footprint {
                    x: 10,
                    z: 0,
                    width: 2,
                    depth: 2,
                },
                &PlacementRules::default(),
                BuildingLogistics::default(),
                ResourceKind::IronOre,
                1,
            )
            .unwrap();
        (world, buildings, Jobs::new(), workers, store, site)
    }

    /// Teleport every builder to its next stop, then run one job check.
    fn auto_step(world: &mut World, buildings: &mut Buildings, jobs: &mut Jobs, store: usize) {
        let players: Vec<Entity> = world.player_characters().collect();
        for w in players {
            let Some(site) = jobs.auto_build_site(w) else {
                world.set_position(w, buildings.buildings[store].worker_slot_position(0));
                continue;
            };
            let carrying = world
                .inventory(w)
                .is_some_and(|i| i.count_of("iron_ore") > 0);
            let p = if carrying || buildings.site_needs(site) == 0 {
                buildings.sites[site].worker_position()
            } else {
                buildings.buildings[store].worker_slot_position(0)
            };
            world.set_position(w, p);
        }
        world.tick = JOB_CHECK_INTERVAL_TICKS * (world.tick / JOB_CHECK_INTERVAL_TICKS + 1);
        jobs.update(world, buildings);
        buildings.tick_production(2.0);
    }

    fn ore_carried(world: &World, workers: &[Entity]) -> u32 {
        workers
            .iter()
            .map(|&w| world.inventory(w).unwrap().count_of("iron_ore"))
            .sum()
    }

    #[test]
    fn only_one_idle_survivor_fetches_a_one_ore_need() {
        let (mut world, mut buildings, mut jobs, workers, store, _site) = auto_setup();
        auto_step(&mut world, &mut buildings, &mut jobs, store);
        let building: Vec<_> = workers
            .iter()
            .filter(|&&w| jobs.auto_build_site(w).is_some())
            .collect();
        assert_eq!(building.len(), 1);
        assert_eq!(ore_carried(&world, &workers), 1);
        assert_eq!(buildings.buildings[store].outputs.count_of("iron_ore"), 4);
    }

    #[test]
    fn idle_survivors_build_the_site_to_completion() {
        let (mut world, mut buildings, mut jobs, workers, store, site) = auto_setup();
        for _ in 0..40 {
            auto_step(&mut world, &mut buildings, &mut jobs, store);
            if buildings.sites[site].completed.is_some() {
                break;
            }
        }
        let built = buildings.sites[site].completed.expect("site completed");
        assert_eq!(buildings.buildings[built].def, "house");
        assert_eq!(buildings.buildings[store].outputs.count_of("iron_ore"), 4);
        auto_step(&mut world, &mut buildings, &mut jobs, store);
        for &w in &workers {
            assert_eq!(jobs.auto_build_site(w), None);
            assert!(!world.is_held(w));
        }
        assert!(jobs.workers.is_empty());
    }

    #[test]
    fn player_move_releases_the_claim_for_someone_else() {
        let (mut world, mut buildings, mut jobs, workers, store, site) = auto_setup();
        auto_step(&mut world, &mut buildings, &mut jobs, store);
        let first = workers
            .iter()
            .copied()
            .find(|&w| jobs.auto_build_site(w).is_some())
            .unwrap();
        let goal = WorldPos::new(40.0, 0.0, 40.0);
        jobs.player_move(&mut world, &mut buildings, first, goal)
            .unwrap();
        assert_eq!(jobs.auto_build_site(first), None);
        auto_step(&mut world, &mut buildings, &mut jobs, store);
        let second = workers
            .iter()
            .copied()
            .find(|&w| jobs.auto_build_site(w) == Some(site))
            .expect("another survivor takes over");
        assert_ne!(second, first);
    }

    #[test]
    fn stock_running_out_releases_the_claim() {
        let (mut world, mut buildings, mut jobs, workers, store, _site) = auto_setup();
        for &w in &workers {
            world.set_position(w, WorldPos::new(20.0, 0.0, 20.0));
        }
        world.tick = JOB_CHECK_INTERVAL_TICKS;
        jobs.update(&mut world, &mut buildings);
        assert!(workers.iter().any(|&w| jobs.auto_build_site(w).is_some()));
        buildings.buildings[store]
            .outputs
            .remove_resource("iron_ore", 5);
        world.tick = JOB_CHECK_INTERVAL_TICKS * 2;
        jobs.update(&mut world, &mut buildings);
        assert!(workers.iter().all(|&w| jobs.auto_build_site(w).is_none()));
    }

    #[test]
    fn sites_with_a_manual_build_job_are_left_alone() {
        let (mut world, mut buildings, mut jobs, workers, store, site) = auto_setup();
        jobs.add_job(workers[0], Job::Build { site, from: store });
        world.tick = JOB_CHECK_INTERVAL_TICKS;
        jobs.update(&mut world, &mut buildings);
        assert!(workers[1..]
            .iter()
            .all(|&w| jobs.auto_build_site(w).is_none()));
    }

    #[test]
    fn auto_building_is_deterministic_across_clone() {
        let (mut world, mut buildings, mut jobs, _workers, store, site) = auto_setup();
        auto_step(&mut world, &mut buildings, &mut jobs, store);
        let (mut b2, mut j2) = (buildings.clone(), jobs.clone());
        let mut w2 = World::from_snapshot(world.snapshot()).expect("restore");
        for _ in 0..12 {
            auto_step(&mut world, &mut buildings, &mut jobs, store);
            auto_step(&mut w2, &mut b2, &mut j2, store);
        }
        assert_eq!(buildings.sites, b2.sites);
        assert_eq!(buildings.buildings, b2.buildings);
        assert!(buildings.sites[site].completed.is_some());
    }
}
