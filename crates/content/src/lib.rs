//! Serializable content definitions and JSON loading.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Faction {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub hostile_by_default: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnitArchetype {
    pub id: String,
    pub name: String,
    pub faction: String,
    pub max_health: f32,
    pub move_speed: f32,
    /// Id of a [`Race`].
    pub race: String,
    /// Id of a [`Behavior`].
    pub behavior: String,
    /// Whether this unit archetype can be recruited into player control.
    #[serde(default)]
    pub recruitable: bool,
}

/// A modification to a named stat: `(base + flat) * (1 + percent)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatModifier {
    pub stat: String,
    #[serde(default)]
    pub flat: f32,
    #[serde(default)]
    pub percent: f32,
}

/// Apply modifiers for `stat` to `base`. Flat bonuses sum first, then percentages sum.
pub fn effective_stat(stat: &str, base: f32, modifiers: &[StatModifier]) -> f32 {
    let (flat, pct) = modifiers
        .iter()
        .filter(|m| m.stat == stat)
        .fold((0.0, 0.0), |(f, p), m| (f + m.flat, p + m.percent));
    (base + flat) * (1.0 + pct)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Race {
    pub id: String,
    pub name: String,
    /// Base stat values keyed by stat name (e.g. "strength").
    pub base_stats: BTreeMap<String, f32>,
    /// Racial buffs and debuffs.
    #[serde(default)]
    pub modifiers: Vec<StatModifier>,
}

impl Race {
    /// Effective value of `stat` (base plus racial modifiers); 0 base if undefined.
    pub fn stat(&self, stat: &str) -> f32 {
        let base = self.base_stats.get(stat).copied().unwrap_or(0.0);
        effective_stat(stat, base, &self.modifiers)
    }
}

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
    /// Resource types this building can produce or hold for dispatch.
    #[serde(default)]
    pub output_resources: Vec<String>,
    /// Whether this building accepts output transfers from other buildings.
    #[serde(default)]
    pub storage: bool,
    /// Whether this building mines ore into its output slot while worked.
    #[serde(default)]
    pub mine: bool,
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

/// A crop: grows over in-game days; the clock drives growth, so there is no per-tick work.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Crop {
    pub id: String,
    pub name: String,
    pub growth_days: f32,
    pub yield_amount: f32,
    pub yield_resource: String,
}

/// Data-driven movement behaviour. `wander_range == 0` means the unit stays put.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Behavior {
    pub id: String,
    pub name: String,
    /// Wander destinations are within this many cells (per axis).
    pub wander_range: i32,
    pub pause_seconds_min: f32,
    pub pause_seconds_max: f32,
    /// Multiplier on the unit's move speed.
    pub speed_multiplier: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Biome {
    pub id: String,
    pub name: String,
    /// Relative harshness in `[0, 1]`.
    pub harshness: f32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Content {
    pub factions: Vec<Faction>,
    pub units: Vec<UnitArchetype>,
    pub biomes: Vec<Biome>,
    pub races: Vec<Race>,
    pub behaviors: Vec<Behavior>,
    pub buildings: Vec<BuildingDef>,
    pub crops: Vec<Crop>,
}

#[derive(Debug)]
pub enum ContentError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Parse {
        path: PathBuf,
        source: serde_json::Error,
    },
    Invalid {
        path: PathBuf,
        message: String,
    },
}

impl fmt::Display for ContentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "cannot read {}: {source}", path.display()),
            Self::Parse { path, source } => {
                write!(f, "invalid JSON in {}: {source}", path.display())
            }
            Self::Invalid { path, message } => {
                write!(f, "invalid content in {}: {message}", path.display())
            }
        }
    }
}

impl std::error::Error for ContentError {}

fn load_file<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Vec<T>, ContentError> {
    let text = std::fs::read_to_string(path).map_err(|source| ContentError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_str(&text).map_err(|source| ContentError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

fn invalid(path: &Path, message: String) -> ContentError {
    ContentError::Invalid {
        path: path.to_path_buf(),
        message,
    }
}

fn check_unique<'a>(
    path: &Path,
    ids: impl Iterator<Item = &'a String>,
) -> Result<(), ContentError> {
    let mut seen = std::collections::HashSet::new();
    for id in ids {
        if id.is_empty() {
            return Err(invalid(path, "empty id".into()));
        }
        if !seen.insert(id) {
            return Err(invalid(path, format!("duplicate id '{id}'")));
        }
    }
    Ok(())
}

/// Load `factions.json`, `units.json`, `biomes.json`, `races.json` and `behaviors.json` from `dir` and validate them.
pub fn load_content(dir: impl AsRef<Path>) -> Result<Content, ContentError> {
    let dir = dir.as_ref();
    let fp = dir.join("factions.json");
    let up = dir.join("units.json");
    let bp = dir.join("biomes.json");
    let factions: Vec<Faction> = load_file(&fp)?;
    let units: Vec<UnitArchetype> = load_file(&up)?;
    let biomes: Vec<Biome> = load_file(&bp)?;
    let rp = dir.join("races.json");
    let hp = dir.join("behaviors.json");
    let races: Vec<Race> = load_file(&rp)?;
    let behaviors: Vec<Behavior> = load_file(&hp)?;
    let bdp = dir.join("buildings.json");
    let cp = dir.join("crops.json");
    let buildings: Vec<BuildingDef> = load_file(&bdp)?;
    let crops: Vec<Crop> = load_file(&cp)?;
    check_unique(&bdp, buildings.iter().map(|b| &b.id))?;
    check_unique(&cp, crops.iter().map(|c| &c.id))?;
    for c in &crops {
        if c.growth_days <= 0.0 || c.yield_amount < 0.0 {
            return Err(invalid(&cp, format!("crop '{}' has invalid values", c.id)));
        }
    }
    for b in &buildings {
        if b.width < 1 || b.depth < 1 {
            return Err(invalid(
                &bdp,
                format!("building '{}' needs a footprint >= 1x1", b.id),
            ));
        }
        if b.min_resource_richness
            .into_iter()
            .chain(b.min_fertility)
            .any(|value| !(0.0..=1.0).contains(&value))
        {
            return Err(invalid(
                &bdp,
                format!("building '{}' has a site threshold outside [0, 1]", b.id),
            ));
        }
        if b.stack_limit == 0
            || (!b.input_resources.is_empty() && b.input_slots == 0)
            || (!b.output_resources.is_empty() && b.output_slots == 0)
            || (b.storage
                && (b.worker_slots == 0 || b.input_slots == 0 || b.input_resources.is_empty()))
        {
            return Err(invalid(
                &bdp,
                format!("building '{}' has incompatible logistics slots", b.id),
            ));
        }
        if let Some(c) = &b.crop {
            if !crops.iter().any(|x| &x.id == c) {
                return Err(invalid(
                    &bdp,
                    format!("building '{}' references unknown crop '{c}'", b.id),
                ));
            }
        }
    }
    check_unique(&rp, races.iter().map(|r| &r.id))?;
    check_unique(&hp, behaviors.iter().map(|b| &b.id))?;
    for b in &behaviors {
        if b.wander_range < 0
            || b.pause_seconds_min < 0.0
            || b.pause_seconds_max < b.pause_seconds_min
            || b.speed_multiplier < 0.0
        {
            return Err(invalid(
                &hp,
                format!("behavior '{}' has invalid values", b.id),
            ));
        }
    }
    for r in &races {
        for m in &r.modifiers {
            if !r.base_stats.contains_key(&m.stat) {
                return Err(invalid(
                    &rp,
                    format!("race '{}' modifies undefined stat '{}'", r.id, m.stat),
                ));
            }
        }
    }

    check_unique(&fp, factions.iter().map(|f| &f.id))?;
    check_unique(&up, units.iter().map(|u| &u.id))?;
    check_unique(&bp, biomes.iter().map(|b| &b.id))?;
    for u in &units {
        if !factions.iter().any(|f| f.id == u.faction) {
            return Err(invalid(
                &up,
                format!("unit '{}' references unknown faction '{}'", u.id, u.faction),
            ));
        }
        if !races.iter().any(|r| r.id == u.race) {
            return Err(invalid(
                &up,
                format!("unit '{}' references unknown race '{}'", u.id, u.race),
            ));
        }
        if !behaviors.iter().any(|b| b.id == u.behavior) {
            return Err(invalid(
                &up,
                format!(
                    "unit '{}' references unknown behavior '{}'",
                    u.id, u.behavior
                ),
            ));
        }
        if u.max_health <= 0.0 || u.move_speed < 0.0 {
            return Err(invalid(&up, format!("unit '{}' has invalid stats", u.id)));
        }
    }
    for b in &biomes {
        if !(0.0..=1.0).contains(&b.harshness) {
            return Err(invalid(
                &bp,
                format!("biome '{}' harshness must be in [0, 1]", b.id),
            ));
        }
    }
    Ok(Content {
        factions,
        units,
        biomes,
        races,
        behaviors,
        buildings,
        crops,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assets_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")
    }

    #[test]
    fn loads_example_content() {
        let c = load_content(assets_dir()).expect("example content loads");
        assert!(!c.factions.is_empty());
        assert!(!c.units.is_empty());
        assert!(!c.biomes.is_empty());

        let wanderer = c.units.iter().find(|u| u.id == "wanderer").unwrap();
        let raider = c.units.iter().find(|u| u.id == "raider").unwrap();
        let sentry = c.units.iter().find(|u| u.id == "sentry").unwrap();
        assert!(wanderer.recruitable);
        assert!(!raider.recruitable);
        assert!(!sentry.recruitable);
    }

    #[test]
    fn loads_buildings_and_crops() {
        let c = load_content(assets_dir()).unwrap();
        let farm = c.buildings.iter().find(|b| b.id == "farm").unwrap();
        assert_eq!(farm.crop.as_deref(), Some("wheat"));
        assert_eq!(farm.output_resources, vec!["wheat"]);
        assert!(c.buildings.iter().any(|b| b.dwelling));
        let mine = c.buildings.iter().find(|b| b.id == "iron_mine").unwrap();
        assert_eq!(mine.requires_resource.as_deref(), Some("iron_ore"));
        assert_eq!(mine.output_resources, vec!["iron_ore"]);
        let wheat_store = c
            .buildings
            .iter()
            .find(|b| b.id == "wheat_storage")
            .unwrap();
        assert!(wheat_store.storage);
        assert_eq!(wheat_store.input_resources, vec!["wheat"]);
        let ore_store = c.buildings.iter().find(|b| b.id == "ore_storage").unwrap();
        assert!(ore_store.storage);
        assert_eq!(ore_store.input_resources, vec!["iron_ore"]);
        let wheat = c.crops.iter().find(|crop| crop.id == "wheat").unwrap();
        assert_eq!(wheat.yield_resource, "wheat");
    }

    #[test]
    fn race_modifiers_apply() {
        let c = load_content(assets_dir()).unwrap();
        let human = c.races.iter().find(|r| r.id == "human").unwrap();
        let brute = c.races.iter().find(|r| r.id == "brute").unwrap();
        assert_eq!(human.stat("strength"), 10.0);
        // brute: base 10, +2 flat, +25% => 15; dexterity base 10, -20% => 8
        assert!((brute.stat("strength") - 15.0).abs() < 1e-4);
        assert!((brute.stat("dexterity") - 8.0).abs() < 1e-4);
        assert_eq!(brute.stat("unknown"), 0.0);
    }

    #[test]
    fn unit_references_resolve_and_bad_ones_fail() {
        let src = assets_dir();
        let dir = std::env::temp_dir().join("ashen_world_bad_ref");
        std::fs::create_dir_all(&dir).unwrap();
        for f in [
            "factions",
            "biomes",
            "races",
            "behaviors",
            "buildings",
            "crops",
        ] {
            std::fs::copy(src.join(format!("{f}.json")), dir.join(format!("{f}.json"))).unwrap();
        }
        std::fs::write(
            dir.join("units.json"),
            r#"[{"id":"x","name":"X","faction":"wanderers","max_health":1.0,"move_speed":1.0,"race":"human","behavior":"nope"}]"#,
        )
        .unwrap();
        let err = load_content(&dir).unwrap_err();
        assert!(err.to_string().contains("unknown behavior 'nope'"));
    }

    #[test]
    fn missing_dir_reports_path() {
        let err = load_content("/nonexistent/dir").unwrap_err();
        assert!(err.to_string().contains("factions.json"));
    }

    #[test]
    fn bad_json_reports_parse_error() {
        let dir = std::env::temp_dir().join("ashen_world_bad_content");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("factions.json"), "[{").unwrap();
        let err = load_content(&dir).unwrap_err();
        assert!(matches!(err, ContentError::Parse { .. }));
        assert!(err.to_string().contains("invalid JSON"));
    }
}
