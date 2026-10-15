use crate::chunk::WorldPos;
use crate::clock::GameClock;
use crate::npc::{Inventory, ItemStack};
use crate::production::{MineProduction, SmeltProduction, SMELT_SECONDS_PER_INGOT};
use crate::resource::ResourceKind;
use crate::terrain::{
    fertility_at, resource_richness_at, TerrainEdit, TerrainEdits, MAX_STEP_HEIGHT,
};
use crate::world::Entity;
use serde::{Deserialize, Serialize};

/// Axis-aligned footprint in world cells: `[x, x + width) x [z, z + depth)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResourceNode {
    pub kind: String,
    pub cell: (i32, i32),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Ore smelting state, present for smelters (iron ore in, iron ingots out).
    #[serde(default)]
    pub smelt: Option<SmeltProduction>,
    /// Resources that must be present in `inputs` for the building to run.
    pub required_inputs: Vec<String>,
    /// Set by the job system while a worker is at a slot and working.
    pub worked: bool,
}

/// Why a building can or cannot currently be worked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkStatus {
    Ready,
    OutputFull,
    MissingInput,
}

impl Building {
    /// Whether production could run right now (ignores whether anyone is working it).
    pub fn work_status(&self) -> WorkStatus {
        if self
            .required_inputs
            .iter()
            .any(|item| self.inputs.count_of(item) == 0)
        {
            return WorkStatus::MissingInput;
        }
        let output_full = match &self.mine {
            Some(mine) => mine.output.is_full(),
            None => {
                !self.is_storage
                    && !self.outputs.allowed_resources.is_empty()
                    && self
                        .outputs
                        .allowed_resources
                        .iter()
                        .all(|item| self.outputs.available_for(item) == 0)
            }
        };
        if output_full {
            WorkStatus::OutputFull
        } else {
            WorkStatus::Ready
        }
    }

    /// World position where the worker in `slot` stands (just south of the footprint).
    pub fn worker_slot_position(&self, slot: usize) -> WorldPos {
        let f = self.footprint;
        WorldPos::new(
            f.x as f32 + 0.5 + (slot as i32 % f.width.max(1)) as f32,
            0.0,
            (f.z + f.depth) as f32 + 0.5,
        )
    }

    pub fn has_free_worker_slot(&self) -> bool {
        self.workers.iter().any(Option::is_none)
    }
}

/// Finite storage where each slot holds one resource type up to `stack_limit`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
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

    /// Whether this inventory accepts `item` at all.
    pub fn allows(&self, item: &str) -> bool {
        self.stack_limit > 0
            && !self.slots.is_empty()
            && (self.allowed_resources.is_empty()
                || self.allowed_resources.iter().any(|r| r == item))
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildingLogistics {
    pub worker_slots: usize,
    pub input_slots: usize,
    pub output_slots: usize,
    pub stack_limit: u32,
    pub input_resources: Vec<String>,
    pub output_resources: Vec<String>,
    pub is_storage: bool,
    pub is_mine: bool,
    pub is_smelter: bool,
    pub required_inputs: Vec<String>,
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
            is_smelter: false,
            required_inputs: Vec::new(),
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Buildings {
    pub buildings: Vec<Building>,
    pub resources: Vec<ResourceNode>,
    /// Areas where the player may not build (e.g. a city claim plus buffer).
    pub protected: Vec<Footprint>,
    /// Construction sites (in progress and completed), indexed by site id.
    #[serde(default)]
    pub sites: Vec<ConstructionSite>,
}

/// Seconds a builder must work at a fully supplied site to finish it.
pub const BUILD_SECONDS: f32 = 4.0;

/// A reserved site where a building is being constructed. Completed sites are
/// kept (with `completed` set) so site indices held by jobs stay stable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConstructionSite {
    /// Definition id of the building to create.
    pub def: String,
    pub footprint: Footprint,
    /// Logistics the finished building starts with.
    pub logistics: BuildingLogistics,
    /// Material that must be delivered to the site.
    pub material: ResourceKind,
    pub required: u32,
    pub delivered: u32,
    /// Seconds of construction work done so far.
    pub progress: f32,
    /// Worker that has reserved the site.
    pub builder: Option<Entity>,
    /// Set by the job system while the builder is at the site with all materials.
    pub worked: bool,
    /// Index of the finished building once construction completes.
    pub completed: Option<usize>,
}

impl ConstructionSite {
    /// World position where the builder stands (just south of the footprint).
    pub fn worker_position(&self) -> WorldPos {
        let f = self.footprint;
        WorldPos::new(f.x as f32 + 0.5, 0.0, (f.z + f.depth) as f32 + 0.5)
    }
}

impl Buildings {
    /// Whether applying `edit` keeps every existing building's footprint within the slope limit.
    pub fn terrain_edit_is_allowed(
        &self,
        seed: u64,
        edits: &TerrainEdits,
        edit: TerrainEdit,
    ) -> bool {
        let mut edited = edits.clone();
        edited.apply(seed, edit);
        self.buildings
            .iter()
            .all(|b| footprint_slope_ok(seed, &edited, b.footprint))
    }

    /// Check whether `fp` is a legal placement. Returns the first reason it is not.
    pub fn validate(
        &self,
        seed: u64,
        edits: &TerrainEdits,
        fp: Footprint,
        rules: &PlacementRules,
    ) -> Result<(), PlaceError> {
        if !footprint_slope_ok(seed, edits, fp) {
            return Err(PlaceError::TooSteep);
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
        edits: &TerrainEdits,
        def: &str,
        fp: Footprint,
        rules: &PlacementRules,
    ) -> Result<usize, PlaceError> {
        self.place_with_logistics(seed, edits, def, fp, rules, BuildingLogistics::default())
    }

    pub fn place_with_logistics(
        &mut self,
        seed: u64,
        edits: &TerrainEdits,
        def: &str,
        fp: Footprint,
        rules: &PlacementRules,
        logistics: BuildingLogistics,
    ) -> Result<usize, PlaceError> {
        self.validate(seed, edits, fp, rules)?;
        Ok(self.push_building(def, fp, logistics))
    }

    fn push_building(&mut self, def: &str, fp: Footprint, logistics: BuildingLogistics) -> usize {
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
            smelt: logistics.is_smelter.then(SmeltProduction::default),
            required_inputs: logistics.required_inputs,
            worked: false,
        });
        self.buildings.len() - 1
    }

    /// Request construction: validate the location, reserve it as a site and
    /// return the site index. Nothing is built until materials arrive and work is done.
    #[allow(clippy::too_many_arguments)]
    pub fn request_construction(
        &mut self,
        seed: u64,
        edits: &TerrainEdits,
        def: &str,
        fp: Footprint,
        rules: &PlacementRules,
        logistics: BuildingLogistics,
        material: ResourceKind,
        required: u32,
    ) -> Result<usize, PlaceError> {
        self.validate(seed, edits, fp, rules)?;
        if self
            .sites
            .iter()
            .any(|s| s.completed.is_none() && s.footprint.overlaps(&fp, rules.building_margin))
        {
            return Err(PlaceError::Overlaps);
        }
        self.sites.push(ConstructionSite {
            def: def.to_string(),
            footprint: fp,
            logistics,
            material,
            required,
            delivered: 0,
            progress: 0.0,
            builder: None,
            worked: false,
            completed: None,
        });
        Ok(self.sites.len() - 1)
    }

    /// Material still missing at an unfinished site (0 if finished or unknown).
    pub fn site_needs(&self, site: usize) -> u32 {
        self.sites
            .get(site)
            .filter(|s| s.completed.is_none())
            .map_or(0, |s| s.required.saturating_sub(s.delivered))
    }

    /// Nearest building (within `radius` of the site) holding the site's
    /// material in its output stock; ties go to the lowest index.
    pub fn find_material_source(&self, site: usize, radius: f32) -> Option<usize> {
        let site = self.sites.get(site).filter(|s| s.completed.is_none())?;
        let origin = site.worker_position();
        let mut best: Option<(usize, f32)> = None;
        for (index, building) in self.buildings.iter().enumerate() {
            if building.outputs.count_of(site.material.id()) == 0 {
                continue;
            }
            let p = building.worker_slot_position(0);
            let dist = ((p.x - origin.x).powi(2) + (p.z - origin.z).powi(2)).sqrt();
            if dist <= radius && best.is_none_or(|(_, d)| dist < d) {
                best = Some((index, dist));
            }
        }
        best.map(|(index, _)| index)
    }

    /// Whether `site_pickup` from `source` would move anything.
    pub fn can_supply_site(&self, source: usize, site: usize) -> bool {
        match (self.buildings.get(source), self.sites.get(site)) {
            (Some(s), Some(t)) => {
                self.site_needs(site) > 0 && s.outputs.count_of(t.material.id()) > 0
            }
            _ => false,
        }
    }

    /// Move up to `requested` of the site's material (never more than it still
    /// needs) from `source`'s output into the hauler's inventory.
    pub fn site_pickup(
        &mut self,
        source: usize,
        site: usize,
        inventory: &mut Inventory,
        requested: u32,
    ) -> Result<u32, LogisticsError> {
        let material = self
            .sites
            .get(site)
            .ok_or(LogisticsError::BuildingNotFound)?
            .material;
        let wanted = requested.min(self.site_needs(site));
        let source = self
            .buildings
            .get_mut(source)
            .ok_or(LogisticsError::BuildingNotFound)?;
        let taken = source.outputs.remove(material.id(), wanted);
        inventory.add(material.id(), taken);
        Ok(taken)
    }

    /// Deliver carried material to the site; excess stays with the hauler.
    pub fn site_deliver(
        &mut self,
        site: usize,
        inventory: &mut Inventory,
    ) -> Result<u32, LogisticsError> {
        let needs = self.site_needs(site);
        let target = self
            .sites
            .get_mut(site)
            .ok_or(LogisticsError::BuildingNotFound)?;
        let moved = needs.min(inventory.count_of(target.material.id()));
        if moved > 0 {
            inventory.remove(target.material.id(), moved);
            target.delivered += moved;
        }
        Ok(moved)
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

    /// Haul pickup: move up to `requested` of `item` from the building's output
    /// into the hauler's inventory, limited by room at `destination`'s input.
    /// No worker slot is needed. Returns the amount picked up.
    pub fn haul_pickup(
        &mut self,
        source: usize,
        destination: usize,
        inventory: &mut Inventory,
        item: &str,
        requested: u32,
    ) -> Result<u32, LogisticsError> {
        if source == destination {
            return Err(LogisticsError::SameBuilding);
        }
        let room = self
            .buildings
            .get(destination)
            .ok_or(LogisticsError::BuildingNotFound)?
            .inputs
            .available_for(item);
        let source = self
            .buildings
            .get_mut(source)
            .ok_or(LogisticsError::BuildingNotFound)?;
        let taken = source.outputs.remove(item, requested.min(room));
        inventory.add(item, taken);
        Ok(taken)
    }

    /// Haul drop-off: move `item` from the hauler's inventory into the building's
    /// input. Anything that does not fit stays with the hauler. Returns the amount delivered.
    pub fn haul_drop(
        &mut self,
        destination: usize,
        inventory: &mut Inventory,
        item: &str,
    ) -> Result<u32, LogisticsError> {
        let building = self
            .buildings
            .get_mut(destination)
            .ok_or(LogisticsError::BuildingNotFound)?;
        let moved = inventory
            .count_of(item)
            .min(building.inputs.available_for(item));
        if moved == 0 {
            return Ok(0);
        }
        let removed = inventory.remove(item, moved);
        debug_assert!(removed);
        Ok(building.inputs.insert(item, moved))
    }

    /// Whether `haul_pickup` from `source` to `destination` would move anything.
    pub fn can_haul(&self, source: usize, destination: usize, item: &str) -> bool {
        source != destination
            && match (self.buildings.get(source), self.buildings.get(destination)) {
                (Some(s), Some(d)) => {
                    s.outputs.count_of(item) > 0 && d.inputs.available_for(item) > 0
                }
                _ => false,
            }
    }

    /// Advance production by `dt` seconds. A building is worked while the job
    /// system has flagged it, a worker holds a slot, and it is ready to run.
    pub fn tick_production(&mut self, dt: f32) {
        for index in 0..self.sites.len() {
            let site = &mut self.sites[index];
            if site.completed.is_some()
                || !site.worked
                || site.builder.is_none()
                || site.delivered < site.required
                || dt <= 0.0
                || dt.is_nan()
            {
                continue;
            }
            site.progress += dt;
            if site.progress >= BUILD_SECONDS {
                let (def, fp, logistics) =
                    (site.def.clone(), site.footprint, site.logistics.clone());
                site.progress = BUILD_SECONDS;
                site.builder = None;
                site.worked = false;
                let built = self.push_building(&def, fp, logistics);
                self.sites[index].completed = Some(built);
            }
        }
        for building in &mut self.buildings {
            let worked = building.worked
                && building.workers.iter().any(Option::is_some)
                && building
                    .required_inputs
                    .iter()
                    .all(|i| building.inputs.count_of(i) > 0);
            if let Some(mine) = &mut building.mine {
                mine.advance(dt, worked);
                // Move mined ore into the building's output storage so it can be hauled.
                let ore = mine.output.item.id();
                let moved = building.outputs.insert(ore, mine.output.count);
                mine.output.take(moved);
            }
            if let Some(smelt) = &mut building.smelt {
                let ore = ResourceKind::IronOre.id();
                let ingot = ResourceKind::IronIngot.id();
                let ready =
                    building.inputs.count_of(ore) > 0 && building.outputs.available_for(ingot) > 0;
                if !worked || !ready || dt <= 0.0 || dt.is_nan() {
                    continue;
                }
                smelt.progress += dt;
                while smelt.progress >= SMELT_SECONDS_PER_INGOT
                    && building.inputs.count_of(ore) > 0
                    && building.outputs.available_for(ingot) > 0
                {
                    building.inputs.remove(ore, 1);
                    building.outputs.insert(ingot, 1);
                    smelt.progress -= SMELT_SECONDS_PER_INGOT;
                }
                smelt.progress = smelt.progress.min(SMELT_SECONDS_PER_INGOT);
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
/// Whether every cell of `fp` is within the slope limit of its corner cell.
fn footprint_slope_ok(seed: u64, edits: &TerrainEdits, fp: Footprint) -> bool {
    let h = |x: i32, z: i32| edits.height(seed, x as f32 + 0.5, z as f32 + 0.5);
    let base = h(fp.x, fp.z);
    let limit = MAX_STEP_HEIGHT * fp.width.max(fp.depth) as f32;
    (fp.z..fp.z + fp.depth)
        .all(|z| (fp.x..fp.x + fp.width).all(|x| (h(x, z) - base).abs() <= limit))
}

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
        assert!(b
            .place(1, &TerrainEdits::default(), "house", fp(0, 0, 2, 2), &rules)
            .is_ok());
        assert_eq!(
            b.validate(1, &TerrainEdits::default(), fp(1, 1, 2, 2), &rules),
            Err(PlaceError::Overlaps)
        );
        b.protected.push(fp(100, 100, 50, 50));
        assert_eq!(
            b.validate(1, &TerrainEdits::default(), fp(110, 110, 2, 2), &rules),
            Err(PlaceError::InProtectedZone)
        );
        let mine = PlacementRules {
            requires_resource: Some("iron_ore".into()),
            minimum_resource_richness: None,
            minimum_fertility: None,
            building_margin: 0,
        };
        assert_eq!(
            b.validate(1, &TerrainEdits::default(), fp(10, 10, 2, 2), &mine),
            Err(PlaceError::MissingResource("iron_ore".into()))
        );
        b.resources.push(ResourceNode {
            kind: "iron_ore".into(),
            cell: (11, 11),
        });
        assert!(b
            .validate(1, &TerrainEdits::default(), fp(10, 10, 2, 2), &mine)
            .is_ok());

        let rich_site = PlacementRules {
            requires_resource: Some("iron_ore".into()),
            minimum_resource_richness: Some(0.0),
            minimum_fertility: None,
            building_margin: 0,
        };
        assert!(b
            .validate(1, &TerrainEdits::default(), fp(10, 10, 1, 1), &rich_site)
            .is_ok());
        let impossible_richness = PlacementRules {
            minimum_resource_richness: Some(1.0),
            ..rich_site.clone()
        };
        assert_eq!(
            b.validate(
                1,
                &TerrainEdits::default(),
                fp(10, 10, 1, 1),
                &impossible_richness
            ),
            Err(PlaceError::InsufficientResourceRichness("iron_ore".into()))
        );
        let fertile_site = PlacementRules {
            minimum_fertility: Some(0.0),
            ..PlacementRules::default()
        };
        assert!(b
            .validate(1, &TerrainEdits::default(), fp(10, 10, 1, 1), &fertile_site)
            .is_ok());
        let impossible_fertility = PlacementRules {
            minimum_fertility: Some(1.0),
            ..PlacementRules::default()
        };
        assert_eq!(
            b.validate(
                1,
                &TerrainEdits::default(),
                fp(10, 10, 1, 1),
                &impossible_fertility
            ),
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
                &TerrainEdits::default(),
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
                    is_smelter: false,
                    required_inputs: Vec::new(),
                },
            )
            .unwrap();
        let storage = buildings
            .place_with_logistics(
                1,
                &TerrainEdits::default(),
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
                    is_smelter: false,
                    required_inputs: Vec::new(),
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
    fn mine_produces_only_while_worked_and_ready() {
        let mut buildings = Buildings::default();
        let mine = buildings
            .place_with_logistics(
                1,
                &TerrainEdits::default(),
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
        let count = |b: &Buildings| b.buildings[mine].mine.as_ref().unwrap().output.count;
        buildings.buildings[mine].worked = true;
        buildings.tick_production(2.0);
        assert_eq!(count(&buildings), 0, "no worker assigned");
        buildings.assign_worker(mine, Entity(1)).unwrap();
        buildings.buildings[mine].worked = false;
        buildings.tick_production(2.0);
        assert_eq!(count(&buildings), 0, "not worked");
        buildings.buildings[mine].worked = true;
        buildings.tick_production(2.0);
        assert_eq!(count(&buildings), 1);
    }

    #[test]
    fn work_status_reports_missing_input_and_full_output() {
        let mut buildings = Buildings::default();
        let fp = Footprint {
            x: 0,
            z: 0,
            width: 2,
            depth: 2,
        };
        let i = buildings
            .place_with_logistics(
                1,
                &TerrainEdits::default(),
                "mine",
                fp,
                &PlacementRules::default(),
                BuildingLogistics {
                    worker_slots: 1,
                    input_slots: 1,
                    input_resources: vec!["tools".into()],
                    required_inputs: vec!["tools".into()],
                    is_mine: true,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            buildings.buildings[i].work_status(),
            WorkStatus::MissingInput
        );
        buildings.buildings[i].inputs.insert_resource("tools", 1);
        assert_eq!(buildings.buildings[i].work_status(), WorkStatus::Ready);
        let cap = ResourceKind::IronOre.stack_size();
        buildings.buildings[i].mine.as_mut().unwrap().output.count = cap;
        assert_eq!(buildings.buildings[i].work_status(), WorkStatus::OutputFull);
        let pos = buildings.buildings[i].worker_slot_position(0);
        assert!(!buildings.buildings[i]
            .footprint
            .contains((pos.x.floor() as i32, pos.z.floor() as i32)));
    }

    #[test]
    fn terrain_edits_that_make_buildings_too_steep_are_rejected() {
        let mut b = Buildings::default();
        let rules = PlacementRules::default();
        let site = fp(10, 10, 2, 2);
        let placed = b.place(1, &TerrainEdits::default(), "house", site, &rules);
        assert!(placed.is_ok());
        let edits = TerrainEdits::default();
        let cliff = TerrainEdit::Raise {
            center: (10, 10),
            radius: 1,
            amount_mm: 30_000,
        };
        assert!(!b.terrain_edit_is_allowed(1, &edits, cliff));
        let elsewhere = TerrainEdit::Raise {
            center: (60, 60),
            radius: 4,
            amount_mm: 3000,
        };
        assert!(b.terrain_edit_is_allowed(1, &edits, elsewhere));
    }

    fn ore_store(b: &mut Buildings, x: i32, z: i32, ore: u32) -> usize {
        let index = b
            .place_with_logistics(
                1,
                &TerrainEdits::default(),
                "store",
                fp(x, z, 1, 1),
                &PlacementRules::default(),
                BuildingLogistics {
                    input_slots: 1,
                    output_slots: 1,
                    stack_limit: 10,
                    input_resources: vec!["iron_ore".into()],
                    output_resources: vec!["iron_ore".into()],
                    is_storage: true,
                    ..Default::default()
                },
            )
            .unwrap();
        b.buildings[index].outputs.insert("iron_ore", ore);
        index
    }

    fn ore_site(b: &mut Buildings) -> usize {
        b.request_construction(
            1,
            &TerrainEdits::default(),
            "house",
            fp(0, 0, 2, 2),
            &PlacementRules::default(),
            BuildingLogistics::default(),
            ResourceKind::IronOre,
            1,
        )
        .unwrap()
    }

    #[test]
    fn material_source_is_nearest_stocked_building_within_radius() {
        let mut b = Buildings::default();
        let site = ore_site(&mut b);
        let far = ore_store(&mut b, 20, 0, 5);
        let empty = ore_store(&mut b, 4, 0, 0);
        assert_eq!(b.find_material_source(site, 30.0), Some(far));
        let near = ore_store(&mut b, 8, 0, 1);
        assert_eq!(b.find_material_source(site, 30.0), Some(near));
        assert_ne!(Some(empty), b.find_material_source(site, 30.0));
        assert_eq!(b.find_material_source(site, 5.0), None);
    }

    #[test]
    fn material_source_ignores_finished_sites() {
        let mut b = Buildings::default();
        let site = ore_site(&mut b);
        ore_store(&mut b, 8, 0, 1);
        b.sites[site].completed = Some(0);
        assert_eq!(b.find_material_source(site, 30.0), None);
    }
}
