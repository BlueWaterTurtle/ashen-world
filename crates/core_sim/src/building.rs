use crate::chunk::WorldPos;
use crate::clock::GameClock;
use crate::npc::{Inventory, ItemStack};
use crate::production::MineProduction;
use crate::terrain::{fertility_at, height_at, resource_richness_at, MAX_STEP_HEIGHT};
use crate::world::Entity;

/// Axis-aligned footprint in world cells: `[x, x + width) x [z, z + depth)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Footprint {
    pub x: i32,
    pub z: i32,
    pub width: i32,
    pub depth: i32,
}

impl Footprint {
    pub fn overlaps(&self, other: &Footprint, margin: i32) -> bool {
        self.x < other.x + other.width + margin
            && other.x < self.x + self.width + margin
            && self.z < other.z + other.depth + margin
            && other.z < self.z + self.depth + margin
    }

    pub fn contains(&self, c: (i32, i32)) -> bool {
        (self.x..self.x + self.width).contains(&c.0) && (self.z..self.z + self.depth).contains(&c.1)
    }
}

/// A point resource on the map (ore deposit, fertile patch, ...).
#[derive(Debug, Clone, PartialEq)]
pub struct ResourceNode {
    pub kind: String,
    pub cell: (i32, i32),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Building {
    /// Building definition id (from content).
    pub def: String,
    pub footprint: Footprint,
    /// Tick at which the current crop was planted, for farms.
    pub planted_tick: Option<u64>,
    pub workers: Vec<Option<Entity>>,
    pub inputs: SlotInventory,
    pub outputs: SlotInventory,
    pub is_storage: bool,
    /// Ore production state, present for mines.
    pub mine: Option<MineProduction>,
}

/// Finite storage where each slot holds one resource type up to `stack_limit`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SlotInventory {
    slots: Vec<Option<ItemStack>>,
    stack_limit: u32,
    allowed_resources: Vec<String>,
}

impl SlotInventory {
    pub fn new(slot_count: usize, stack_limit: u32, allowed_resources: Vec<String>) -> Self {
        Self {
            slots: vec![None; slot_count],
            stack_limit,
            allowed_resources,
        }
    }

    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }

    pub fn stack_limit(&self) -> u32 {
        self.stack_limit
    }

    pub fn count_of(&self, item: &str) -> u32 {
        self.slots
            .iter()
            .flatten()
            .filter(|stack| stack.item == item)
            .map(|stack| stack.count)
            .sum()
    }

    pub fn items(&self) -> impl Iterator<Item = &ItemStack> {
        self.slots.iter().flatten()
    }

    pub fn insert_resource(&mut self, item: &str, count: u32) -> u32 {
        self.insert(item, count)
    }

    pub fn remove_resource(&mut self, item: &str, count: u32) -> u32 {
        self.remove(item, count)
    }

    fn available_for(&self, item: &str) -> u32 {
        if self.stack_limit == 0
            || (!self.allowed_resources.is_empty()
                && !self
                    .allowed_resources
                    .iter()
                    .any(|resource| resource == item))
        {
            return 0;
        }
        if let Some(stack) = self.slots.iter().flatten().find(|stack| stack.item == item) {
            self.stack_limit.saturating_sub(stack.count)
        } else if self.slots.iter().any(Option::is_none) {
            self.stack_limit
        } else {
            0
        }
    }

    fn insert(&mut self, item: &str, count: u32) -> u32 {
        let accepted = count.min(self.available_for(item));
        if accepted == 0 {
            return 0;
        }
        if let Some(stack) = self
            .slots
            .iter_mut()
            .flatten()
            .find(|stack| stack.item == item)
        {
            stack.count += accepted;
        } else if let Some(slot) = self.slots.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(ItemStack::new(item, accepted));
        }
        accepted
    }

    fn remove(&mut self, item: &str, count: u32) -> u32 {
        let mut remaining = count;
        for slot in &mut self.slots {
            if let Some(stack) = slot {
                if stack.item == item {
                    let taken = stack.count.min(remaining);
                    stack.count -= taken;
                    remaining -= taken;
                    if stack.count == 0 {
                        *slot = None;
                    }
                    if remaining == 0 {
                        break;
                    }
                }
            }
        }
        count - remaining
    }
}

/// Initial worker and storage capacity for a newly placed building.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildingLogistics {
    pub worker_slots: usize,
    pub input_slots: usize,
    pub output_slots: usize,
    pub stack_limit: u32,
    pub input_resources: Vec<String>,
    pub output_resources: Vec<String>,
    pub is_storage: bool,
    pub is_mine: bool,
}

impl Default for BuildingLogistics {
    fn default() -> Self {
        Self {
            worker_slots: 0,
            input_slots: 0,
            output_slots: 0,
            stack_limit: 100,
            input_resources: Vec::new(),
            output_resources: Vec::new(),
            is_storage: false,
            is_mine: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogisticsError {
    BuildingNotFound,
    NoWorkerSlots,
    WorkerAlreadyAssigned,
    WorkerAssignedElsewhere,
    WorkerNotAssigned,
    SameBuilding,
    NotStorage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaceError {
    TooSteep,
    Overlaps,
    InProtectedZone,
    MissingResource(String),
    InsufficientResourceRichness(String),
    InsufficientFertility,
}

/// What a building needs to be placed; filled from content by the caller.
#[derive(Debug, Clone, Default)]
pub struct PlacementRules {
    pub requires_resource: Option<String>,
    /// Minimum footprint-average richness for `requires_resource`, in `[0, 1]`.
    pub minimum_resource_richness: Option<f32>,
    /// Minimum footprint-average soil fertility, in `[0, 1]`.
    pub minimum_fertility: Option<f32>,
    /// Extra empty cells required around other buildings.
    pub building_margin: i32,
}

/// Map-wide things placement is checked against.
#[derive(Debug, Default)]
pub struct Buildings {
    pub buildings: Vec<Building>,
    pub resources: Vec<ResourceNode>,
    /// Areas where the player may not build (e.g. a city claim plus buffer).
    pub protected: Vec<Footprint>,
}

impl Buildings {
    /// Check whether `fp` is a legal placement. Returns the first reason it is not.
    pub fn validate(
        &self,
        seed: u64,
        fp: Footprint,
        rules: &PlacementRules,
    ) -> Result<(), PlaceError> {
        let h = |x: i32, z: i32| height_at(seed, x as f32 + 0.5, z as f32 + 0.5);
        let base = h(fp.x, fp.z);
        for z in fp.z..fp.z + fp.depth {
            for x in fp.x..fp.x + fp.width {
                if (h(x, z) - base).abs() > MAX_STEP_HEIGHT * fp.width.max(fp.depth) as f32 {
                    return Err(PlaceError::TooSteep);
                }
            }
        }
        if self.protected.iter().any(|p| p.overlaps(&fp, 0)) {
            return Err(PlaceError::InProtectedZone);
        }
        if self
            .buildings
            .iter()
            .any(|b| b.footprint.overlaps(&fp, rules.building_margin))
        {
            return Err(PlaceError::Overlaps);
        }
        if let Some(kind) = &rules.requires_resource {
            if let Some(minimum) = rules.minimum_resource_richness {
                if average_resource_richness(seed, fp, kind) <= minimum {
                    return Err(PlaceError::InsufficientResourceRichness(kind.clone()));
                }
            } else if !self
                .resources
                .iter()
                .any(|r| &r.kind == kind && fp.contains(r.cell))
            {
                return Err(PlaceError::MissingResource(kind.clone()));
            }
        }
        if rules
            .minimum_fertility
            .is_some_and(|minimum| average_fertility(seed, fp) <= minimum)
        {
            return Err(PlaceError::InsufficientFertility);
        }
        Ok(())
    }

    /// Validate and place; returns the new building's index.
    pub fn place(
        &mut self,
        seed: u64,
        def: &str,
        fp: Footprint,
        rules: &PlacementRules,
    ) -> Result<usize, PlaceError> {
        self.place_with_logistics(seed, def, fp, rules, BuildingLogistics::default())
    }

    pub fn place_with_logistics(
        &mut self,
        seed: u64,
        def: &str,
        fp: Footprint,
        rules: &PlacementRules,
        logistics: BuildingLogistics,
    ) -> Result<usize, PlaceError> {
        self.validate(seed, fp, rules)?;
        self.buildings.push(Building {
            def: def.to_string(),
            footprint: fp,
            planted_tick: None,
            workers: vec![None; logistics.worker_slots],
            inputs: SlotInventory::new(
                logistics.input_slots,
                logistics.stack_limit,
                logistics.input_resources.clone(),
            ),
            outputs: SlotInventory::new(
                logistics.output_slots,
                logistics.stack_limit,
                logistics.output_resources.clone(),
            ),
            is_storage: logistics.is_storage,
            mine: logistics.is_mine.then(MineProduction::default),
        });
        Ok(self.buildings.len() - 1)
    }

    pub fn assign_worker(
        &mut self,
        building_index: usize,
        worker: Entity,
    ) -> Result<usize, LogisticsError> {
        let building = self
            .buildings
            .get(building_index)
            .ok_or(LogisticsError::BuildingNotFound)?;
        if building.workers.contains(&Some(worker)) {
            return Err(LogisticsError::WorkerAlreadyAssigned);
        }
        if self.buildings.iter().enumerate().any(|(index, building)| {
            index != building_index && building.workers.contains(&Some(worker))
        }) {
            return Err(LogisticsError::WorkerAssignedElsewhere);
        }
        let building = self
            .buildings
            .get_mut(building_index)
            .ok_or(LogisticsError::BuildingNotFound)?;
        let slot = building
            .workers
            .iter_mut()
            .find(|slot| slot.is_none())
            .ok_or(LogisticsError::NoWorkerSlots)?;
        *slot = Some(worker);
        Ok(building
            .workers
            .iter()
            .position(|slot| *slot == Some(worker))
            .expect("worker was just assigned"))
    }

    pub fn unassign_worker(
        &mut self,
        building_index: usize,
        worker: Entity,
    ) -> Result<(), LogisticsError> {
        let building = self
            .buildings
            .get_mut(building_index)
            .ok_or(LogisticsError::BuildingNotFound)?;
        let slot = building
            .workers
            .iter_mut()
            .find(|slot| **slot == Some(worker))
            .ok_or(LogisticsError::WorkerNotAssigned)?;
        *slot = None;
        Ok(())
    }

    pub fn transfer_to_input(
        &mut self,
        building_index: usize,
        worker: Entity,
        inventory: &mut Inventory,
        item: &str,
        requested: u32,
    ) -> Result<u32, LogisticsError> {
        let building = self
            .buildings
            .get_mut(building_index)
            .ok_or(LogisticsError::BuildingNotFound)?;
        if !building.workers.contains(&Some(worker)) {
            return Err(LogisticsError::WorkerNotAssigned);
        }
        let transfer = requested
            .min(inventory.count_of(item))
            .min(building.inputs.available_for(item));
        if transfer == 0 {
            return Ok(0);
        }
        let removed = inventory.remove(item, transfer);
        debug_assert!(removed);
        Ok(building.inputs.insert(item, transfer))
    }

    pub fn transfer_from_output(
        &mut self,
        building_index: usize,
        worker: Entity,
        inventory: &mut Inventory,
        item: &str,
        requested: u32,
    ) -> Result<u32, LogisticsError> {
        let building = self
            .buildings
            .get_mut(building_index)
            .ok_or(LogisticsError::BuildingNotFound)?;
        if !building.workers.contains(&Some(worker)) {
            return Err(LogisticsError::WorkerNotAssigned);
        }
        let transfer = requested.min(building.outputs.count_of(item));
        if transfer == 0 {
            return Ok(0);
        }
        let removed = building.outputs.remove(item, transfer);
        inventory.add(item, removed);
        Ok(removed)
    }

    pub fn transfer_from_input(
        &mut self,
        building_index: usize,
        worker: Entity,
        inventory: &mut Inventory,
        item: &str,
        requested: u32,
    ) -> Result<u32, LogisticsError> {
        let building = self
            .buildings
            .get_mut(building_index)
            .ok_or(LogisticsError::BuildingNotFound)?;
        if !building.workers.contains(&Some(worker)) {
            return Err(LogisticsError::WorkerNotAssigned);
        }
        let transfer = requested.min(building.inputs.count_of(item));
        let removed = building.inputs.remove(item, transfer);
        inventory.add(item, removed);
        Ok(removed)
    }

    pub fn transfer_input_to_output(
        &mut self,
        building_index: usize,
        worker: Entity,
        item: &str,
        requested: u32,
    ) -> Result<u32, LogisticsError> {
        let building = self
            .buildings
            .get_mut(building_index)
            .ok_or(LogisticsError::BuildingNotFound)?;
        if !building.is_storage {
            return Err(LogisticsError::NotStorage);
        }
        if !building.workers.contains(&Some(worker)) {
            return Err(LogisticsError::WorkerNotAssigned);
        }
        let transfer = requested
            .min(building.inputs.count_of(item))
            .min(building.outputs.available_for(item));
        let removed = building.inputs.remove(item, transfer);
        let inserted = building.outputs.insert(item, removed);
        debug_assert_eq!(removed, inserted);
        Ok(inserted)
    }

    pub fn transfer_output_to_input(
        &mut self,
        source_index: usize,
        destination_index: usize,
        worker: Entity,
        item: &str,
        requested: u32,
    ) -> Result<u32, LogisticsError> {
        if source_index == destination_index {
            return Err(LogisticsError::SameBuilding);
        }
        let (source, destination) = if source_index < destination_index {
            let (before, after) = self.buildings.split_at_mut(destination_index);
            (before.get_mut(source_index), after.first_mut())
        } else {
            let (before, after) = self.buildings.split_at_mut(source_index);
            (after.first_mut(), before.get_mut(destination_index))
        };
        let source = source.ok_or(LogisticsError::BuildingNotFound)?;
        let destination = destination.ok_or(LogisticsError::BuildingNotFound)?;
        if !source.workers.contains(&Some(worker)) && !destination.workers.contains(&Some(worker)) {
            return Err(LogisticsError::WorkerNotAssigned);
        }
        if !destination.is_storage {
            return Err(LogisticsError::NotStorage);
        }
        let transfer = requested
            .min(source.outputs.count_of(item))
            .min(destination.inputs.available_for(item));
        if transfer == 0 {
            return Ok(0);
        }
        let removed = source.outputs.remove(item, transfer);
        let inserted = destination.inputs.insert(item, removed);
        debug_assert_eq!(removed, inserted);
        Ok(inserted)
    }

    pub fn consume_input(
        &mut self,
        building_index: usize,
        worker: Entity,
        item: &str,
        requested: u32,
    ) -> Result<u32, LogisticsError> {
        let building = self
            .buildings
            .get_mut(building_index)
            .ok_or(LogisticsError::BuildingNotFound)?;
        if !building.workers.contains(&Some(worker)) {
            return Err(LogisticsError::WorkerNotAssigned);
        }
        Ok(building.inputs.remove(item, requested))
    }

    pub fn produce_output(
        &mut self,
        building_index: usize,
        worker: Entity,
        item: &str,
        count: u32,
    ) -> Result<u32, LogisticsError> {
        let building = self
            .buildings
            .get_mut(building_index)
            .ok_or(LogisticsError::BuildingNotFound)?;
        if !building.workers.contains(&Some(worker)) {
            return Err(LogisticsError::WorkerNotAssigned);
        }
        Ok(building.outputs.insert(item, count))
    }

    /// Advance mine production by `dt` seconds. A mine is worked while any assigned
    /// worker satisfies `is_working`.
    pub fn tick_production(&mut self, dt: f32, is_working: impl Fn(Entity) -> bool) {
        for building in &mut self.buildings {
            let worked = building.workers.iter().flatten().any(|w| is_working(*w));
            if let Some(mine) = &mut building.mine {
                mine.advance(dt, worked);
            }
        }
    }

    /// True if a cell is blocked by a building (for pathfinding obstacles).
    pub fn blocks(&self, cell: (i32, i32)) -> bool {
        self.buildings.iter().any(|b| b.footprint.contains(cell))
    }

    pub fn center(&self, i: usize) -> WorldPos {
        let f = self.buildings[i].footprint;
        WorldPos::new(
            f.x as f32 + f.width as f32 / 2.0,
            0.0,
            f.z as f32 + f.depth as f32 / 2.0,
        )
    }
}

/// Average resource richness across all cells covered by a footprint.
pub fn average_resource_richness(seed: u64, fp: Footprint, resource: &str) -> f32 {
    footprint_average(fp, |x, z| {
        resource_richness_at(seed, resource, x as f32 + 0.5, z as f32 + 0.5)
    })
}

/// Average soil fertility across all cells covered by a footprint.
pub fn average_fertility(seed: u64, fp: Footprint) -> f32 {
    footprint_average(fp, |x, z| {
        fertility_at(seed, x as f32 + 0.5, z as f32 + 0.5)
    })
}

fn footprint_average(fp: Footprint, sample: impl Fn(i32, i32) -> f32) -> f32 {
    let mut total = 0.0;
    let mut count = 0;
    for z in fp.z..fp.z + fp.depth {
        for x in fp.x..fp.x + fp.width {
            total += sample(x, z);
            count += 1;
        }
    }
    if count == 0 {
        0.0
    } else {
        total / count as f32
    }
}

/// Crop growth progress in `[0, 1]`, computed from the clock on demand (no per-tick cost).
pub fn crop_progress(clock: &GameClock, planted_tick: u64, now: u64, growth_days: f32) -> f32 {
    let days = clock.days_elapsed(now.saturating_sub(planted_tick)) as f32;
    (days / growth_days.max(f32::EPSILON)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::TICKS_PER_SECOND;

    fn fp(x: i32, z: i32, w: i32, d: i32) -> Footprint {
        Footprint {
            x,
            z,
            width: w,
            depth: d,
        }
    }

    #[test]
    fn overlap_and_margin() {
        assert!(fp(0, 0, 4, 4).overlaps(&fp(3, 3, 4, 4), 0));
        assert!(!fp(0, 0, 4, 4).overlaps(&fp(4, 0, 4, 4), 0));
        assert!(fp(0, 0, 4, 4).overlaps(&fp(5, 0, 4, 4), 2));
    }

    #[test]
    fn placement_rules() {
        let mut b = Buildings::default();
        let rules = PlacementRules::default();
        assert!(b.place(1, "house", fp(0, 0, 2, 2), &rules).is_ok());
        assert_eq!(
            b.validate(1, fp(1, 1, 2, 2), &rules),
            Err(PlaceError::Overlaps)
        );
        b.protected.push(fp(100, 100, 50, 50));
        assert_eq!(
            b.validate(1, fp(110, 110, 2, 2), &rules),
            Err(PlaceError::InProtectedZone)
        );
        let mine = PlacementRules {
            requires_resource: Some("iron_ore".into()),
            minimum_resource_richness: None,
            minimum_fertility: None,
            building_margin: 0,
        };
        assert_eq!(
            b.validate(1, fp(10, 10, 2, 2), &mine),
            Err(PlaceError::MissingResource("iron_ore".into()))
        );
        b.resources.push(ResourceNode {
            kind: "iron_ore".into(),
            cell: (11, 11),
        });
        assert!(b.validate(1, fp(10, 10, 2, 2), &mine).is_ok());

        let rich_site = PlacementRules {
            requires_resource: Some("iron_ore".into()),
            minimum_resource_richness: Some(0.0),
            minimum_fertility: None,
            building_margin: 0,
        };
        assert!(b.validate(1, fp(10, 10, 1, 1), &rich_site).is_ok());
        let impossible_richness = PlacementRules {
            minimum_resource_richness: Some(1.0),
            ..rich_site.clone()
        };
        assert_eq!(
            b.validate(1, fp(10, 10, 1, 1), &impossible_richness),
            Err(PlaceError::InsufficientResourceRichness("iron_ore".into()))
        );
        let fertile_site = PlacementRules {
            minimum_fertility: Some(0.0),
            ..PlacementRules::default()
        };
        assert!(b.validate(1, fp(10, 10, 1, 1), &fertile_site).is_ok());
        let impossible_fertility = PlacementRules {
            minimum_fertility: Some(1.0),
            ..PlacementRules::default()
        };
        assert_eq!(
            b.validate(1, fp(10, 10, 1, 1), &impossible_fertility),
            Err(PlaceError::InsufficientFertility)
        );
        assert!(b.blocks((1, 1)) && !b.blocks((5, 5)));
    }

    #[test]
    fn worker_produces_and_hauls_resources_into_typed_storage() {
        let mut buildings = Buildings::default();
        let rules = PlacementRules::default();
        let farm = buildings
            .place_with_logistics(
                1,
                "farm",
                fp(0, 0, 1, 1),
                &rules,
                BuildingLogistics {
                    worker_slots: 1,
                    input_slots: 1,
                    output_slots: 1,
                    stack_limit: 5,
                    input_resources: vec!["wheat_seed".into()],
                    output_resources: vec!["wheat".into()],
                    is_storage: false,
                    is_mine: false,
                },
            )
            .unwrap();
        let storage = buildings
            .place_with_logistics(
                1,
                "wheat_storage",
                fp(5, 0, 1, 1),
                &rules,
                BuildingLogistics {
                    worker_slots: 1,
                    input_slots: 2,
                    output_slots: 2,
                    stack_limit: 3,
                    input_resources: vec!["wheat".into()],
                    output_resources: vec!["wheat".into()],
                    is_storage: true,
                    is_mine: false,
                },
            )
            .unwrap();

        let farm_worker = Entity(1);
        let storage_worker = Entity(2);
        assert_eq!(buildings.assign_worker(farm, farm_worker), Ok(0));
        assert_eq!(
            buildings.assign_worker(storage, farm_worker),
            Err(LogisticsError::WorkerAssignedElsewhere)
        );
        assert_eq!(
            buildings.assign_worker(farm, farm_worker),
            Err(LogisticsError::WorkerAlreadyAssigned)
        );

        let mut farm_inventory = Inventory::default();
        farm_inventory.add("wheat_seed", 10);
        assert_eq!(
            buildings.transfer_to_input(farm, farm_worker, &mut farm_inventory, "wheat_seed", 8),
            Ok(5)
        );
        assert_eq!(farm_inventory.count_of("wheat_seed"), 5);
        assert_eq!(
            buildings.consume_input(farm, farm_worker, "wheat_seed", 2),
            Ok(2)
        );
        assert_eq!(
            buildings.produce_output(farm, farm_worker, "wheat", 8),
            Ok(5)
        );
        assert_eq!(
            buildings.produce_output(farm, farm_worker, "iron_ore", 1),
            Ok(0)
        );

        assert_eq!(
            buildings.transfer_output_to_input(farm, storage, farm_worker, "wheat", 5),
            Ok(3)
        );
        assert_eq!(buildings.buildings[farm].outputs.count_of("wheat"), 2);
        assert_eq!(buildings.buildings[storage].inputs.count_of("wheat"), 3);
        assert_eq!(buildings.assign_worker(storage, storage_worker), Ok(0));
        assert_eq!(
            buildings.transfer_input_to_output(storage, storage_worker, "wheat", 3),
            Ok(3)
        );
        let mut storage_inventory = Inventory::default();
        assert_eq!(
            buildings.transfer_from_output(
                storage,
                storage_worker,
                &mut storage_inventory,
                "wheat",
                10,
            ),
            Ok(3)
        );
        assert_eq!(storage_inventory.count_of("wheat"), 3);
    }

    #[test]
    fn crop_growth_follows_clock() {
        let clock = GameClock {
            day_length_seconds: 10.0,
        };
        let day = 10 * TICKS_PER_SECOND as u64;
        assert_eq!(crop_progress(&clock, 0, 0, 3.0), 0.0);
        assert!((crop_progress(&clock, 0, day * 3 / 2, 3.0) - 0.5).abs() < 1e-5);
        assert_eq!(crop_progress(&clock, 0, day * 10, 3.0), 1.0);
    }

    #[test]
    fn mine_produces_only_while_worker_working() {
        let mut buildings = Buildings::default();
        let mine = buildings
            .place_with_logistics(
                1,
                "iron_mine",
                Footprint {
                    x: 0,
                    z: 0,
                    width: 1,
                    depth: 1,
                },
                &PlacementRules::default(),
                BuildingLogistics {
                    worker_slots: 1,
                    is_mine: true,
                    ..Default::default()
                },
            )
            .unwrap();
        let worker = Entity(1);
        buildings.tick_production(2.0, |_| true);
        assert_eq!(
            buildings.buildings[mine]
                .mine
                .as_ref()
                .unwrap()
                .output
                .count,
            0
        );
        buildings.assign_worker(mine, worker).unwrap();
        buildings.tick_production(2.0, |_| false);
        assert_eq!(
            buildings.buildings[mine]
                .mine
                .as_ref()
                .unwrap()
                .output
                .count,
            0
        );
        buildings.tick_production(2.0, |w| w == worker);
        assert_eq!(
            buildings.buildings[mine]
                .mine
                .as_ref()
                .unwrap()
                .output
                .count,
            1
        );
    }
}
