use super::{ContentDomain, Reference};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A placeable building type. Everything about placement and production is data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BuildingDef {
    pub id: String,
    pub name: String,
    /// Footprint in cells (width along x, depth along z).
    pub width: i32,
    pub depth: i32,
    /// Resource node type this must be placed on (e.g. "iron_ore"); `None` = anywhere.
    #[serde(default)]
    pub requires_resource: Option<String>,
    /// Minimum footprint-average richness for the required resource, in `[0, 1]`.
    #[serde(default)]
    pub min_resource_richness: Option<f32>,
    /// Minimum footprint-average soil fertility, in `[0, 1]`.
    #[serde(default)]
    pub min_fertility: Option<f32>,
    /// Number of workers that can be assigned to this building.
    #[serde(default)]
    pub worker_slots: usize,
    /// Number of distinct resource stacks accepted as production inputs.
    #[serde(default)]
    pub input_slots: usize,
    /// Number of distinct resource stacks held as production outputs.
    #[serde(default)]
    pub output_slots: usize,
    /// Maximum quantity of one resource type per input/output slot.
    #[serde(default = "default_building_stack_limit")]
    pub stack_limit: u32,
    /// Resource types accepted by production inputs or storage.
    #[serde(default)]
    pub input_resources: Vec<String>,
    /// Resources that must be in the input slots for production to run.
    #[serde(default)]
    pub required_inputs: Vec<String>,
    /// Resource types this building can produce or hold for dispatch.
    #[serde(default)]
    pub output_resources: Vec<String>,
    /// Whether this building accepts output transfers from other buildings.
    #[serde(default)]
    pub storage: bool,
    /// Whether this building mines ore into its output slot while worked.
    #[serde(default)]
    pub mine: bool,
    /// Whether this building smelts iron ore into iron ingots while worked.
    #[serde(default)]
    pub smelter: bool,
    /// Whether residents can live here.
    #[serde(default)]
    pub dwelling: bool,
    /// Build cost: resource id -> amount.
    #[serde(default)]
    pub cost: BTreeMap<String, f32>,
    /// Crop id grown here, if this is a farm.
    #[serde(default)]
    pub crop: Option<String>,
}

fn default_building_stack_limit() -> u32 {
    100
}

impl ContentDomain for BuildingDef {
    const NAME: &'static str = "buildings";
    const KIND: &'static str = "building";

    fn id(&self) -> &str {
        &self.id
    }

    fn validate(&self) -> Result<(), String> {
        let b = self;
        if b.width < 1 || b.depth < 1 {
            return Err(format!("building '{}' needs a footprint >= 1x1", b.id));
        }
        if b.min_resource_richness
            .into_iter()
            .chain(b.min_fertility)
            .any(|value| !(0.0..=1.0).contains(&value))
        {
            return Err(format!(
                "building '{}' has a site threshold outside [0, 1]",
                b.id
            ));
        }
        if b.stack_limit == 0
            || (!b.input_resources.is_empty() && b.input_slots == 0)
            || (!b.output_resources.is_empty() && b.output_slots == 0)
            || (b.storage
                && (b.worker_slots == 0 || b.input_slots == 0 || b.input_resources.is_empty()))
        {
            return Err(format!(
                "building '{}' has incompatible logistics slots",
                b.id
            ));
        }
        let is_production = !b.output_resources.is_empty() && !b.storage;
        if (is_production && b.worker_slots == 0)
            || b.required_inputs
                .iter()
                .any(|item| !b.input_resources.contains(item))
        {
            return Err(format!(
                "building '{}' needs a worker slot and required inputs must be accepted inputs",
                b.id
            ));
        }
        Ok(())
    }

    fn references(&self) -> Vec<Reference<'_>> {
        self.crop
            .iter()
            .map(|c| Reference::new("crop", c))
            .collect()
    }
}
