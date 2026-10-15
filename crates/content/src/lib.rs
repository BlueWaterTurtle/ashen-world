//! Serializable content definitions and RON loading.
//!
//! Preferred API: [`load_content`] returns a fully validated [`Content`] from a flat
//! directory of `<domain>.ron` files. The layered [`registry`]/[`cache`] loader is the
//! lower-level mod-overlay mechanism for a single domain; it does no validation and is
//! not used by the game yet.

use serde::Deserialize;
use std::fmt;
use std::path::{Path, PathBuf};

pub mod cache;
pub mod domains;
pub mod manifest;
pub mod registry;
pub mod scenario;

pub use domains::{
    effective_stat, Behavior, Biome, BuildingDef, ContentDomain, Crop, Faction, KnownIds, Race,
    Reference, StatModifier, UnitArchetype,
};
pub use manifest::Manifest;
pub use scenario::{Scenario, WorldOverrides};

#[derive(Debug)]
pub enum ContentError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Parse {
        path: PathBuf,
        source: ron::error::SpannedError,
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
                write!(f, "invalid RON in {}: {source}", path.display())
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
    ron::from_str(&text).map_err(|source| ContentError::Parse {
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

fn check_unique<'a>(path: &Path, ids: impl Iterator<Item = &'a str>) -> Result<(), ContentError> {
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

/// Load, uniqueness-check and validate one domain from `<dir>/<NAME>.ron`.
fn load_domain<T: ContentDomain>(dir: &Path) -> Result<(Vec<T>, PathBuf), ContentError> {
    let path = dir.join(format!("{}.ron", T::NAME));
    let items: Vec<T> = load_file(&path)?;
    check_unique(&path, items.iter().map(|i| i.id()))?;
    for item in &items {
        item.validate().map_err(|m| invalid(&path, m))?;
    }
    Ok((items, path))
}

fn check_refs<T: ContentDomain>(
    ids: &KnownIds,
    items: &[T],
    path: &Path,
) -> Result<(), ContentError> {
    ids.check_references(items).map_err(|m| invalid(path, m))
}

/// Load every content domain from `dir` (`factions.ron`, `units.ron`, ...) and validate it,
/// including cross-domain references.
pub fn load_content(dir: impl AsRef<Path>) -> Result<Content, ContentError> {
    load_content_with(dir, &Manifest::default())
}

/// Load only the domains enabled in `manifest`; disabled domains stay empty and their files are
/// not read. References into a disabled domain are not checked.
pub fn load_content_with(
    dir: impl AsRef<Path>,
    manifest: &Manifest,
) -> Result<Content, ContentError> {
    let dir = dir.as_ref();
    let mut c = Content::default();
    let mut unchecked: Vec<&'static str> = Vec::new();
    let mut paths = (PathBuf::new(), PathBuf::new());

    macro_rules! domain {
        ($field:ident, $ty:ty) => {
            if manifest.has_domain(<$ty>::NAME) {
                let (items, path) = load_domain::<$ty>(dir)?;
                c.$field = items;
                path
            } else {
                unchecked.push(<$ty>::KIND);
                PathBuf::new()
            }
        };
    }
    domain!(factions, Faction);
    paths.0 = domain!(units, UnitArchetype);
    domain!(biomes, Biome);
    domain!(races, Race);
    domain!(behaviors, Behavior);
    paths.1 = domain!(buildings, BuildingDef);
    domain!(crops, Crop);
    let (units_path, buildings_path) = paths;

    let mut ids = KnownIds::default();
    ids.register(&c.factions);
    ids.register(&c.units);
    ids.register(&c.biomes);
    ids.register(&c.races);
    ids.register(&c.behaviors);
    ids.register(&c.buildings);
    ids.register(&c.crops);
    for kind in unchecked {
        ids.mark_unchecked(kind);
    }
    check_refs(&ids, &c.buildings, &buildings_path)?;
    check_refs(&ids, &c.units, &units_path)?;
    Ok(c)
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
            std::fs::copy(src.join(format!("{f}.ron")), dir.join(format!("{f}.ron"))).unwrap();
        }
        std::fs::write(
            dir.join("units.ron"),
            r#"[(id:"x",name:"X",faction:"wanderers",max_health:1.0,move_speed:1.0,race:"human",behavior:"nope")]"#,
        )
        .unwrap();
        let err = load_content(&dir).unwrap_err();
        assert!(err.to_string().contains("unknown behavior 'nope'"));
    }

    #[test]
    fn dangling_crop_reference_fails() {
        let dir = std::env::temp_dir().join("ashen_world_bad_crop_ref");
        std::fs::create_dir_all(&dir).unwrap();
        for f in ["factions", "biomes", "races", "behaviors", "crops", "units"] {
            std::fs::copy(
                assets_dir().join(format!("{f}.ron")),
                dir.join(format!("{f}.ron")),
            )
            .unwrap();
        }
        std::fs::write(
            dir.join("buildings.ron"),
            r#"[(id:"f",name:"F",width:1,depth:1,crop:Some("nope"))]"#,
        )
        .unwrap();
        let err = load_content(&dir).unwrap_err();
        assert!(err.to_string().contains("unknown crop 'nope'"));
    }

    #[test]
    fn default_manifest_matches_load_content() {
        let a = load_content(assets_dir()).unwrap();
        let b = load_content_with(assets_dir(), &Manifest::default()).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn disabled_domains_are_skipped() {
        let m = Manifest {
            content_domains: vec!["factions".into(), "races".into(), "behaviors".into()],
            systems: vec![],
        };
        let c = load_content_with(assets_dir(), &m).unwrap();
        assert!(!c.factions.is_empty() && !c.races.is_empty());
        assert!(c.units.is_empty() && c.buildings.is_empty() && c.crops.is_empty());
    }

    #[test]
    fn missing_dir_reports_path() {
        let err = load_content("/nonexistent/dir").unwrap_err();
        assert!(err.to_string().contains("factions.ron"));
    }

    #[test]
    fn bad_ron_reports_parse_error() {
        let dir = std::env::temp_dir().join("ashen_world_bad_content");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("factions.ron"), "[{").unwrap();
        let err = load_content(&dir).unwrap_err();
        assert!(matches!(err, ContentError::Parse { .. }));
        assert!(err.to_string().contains("invalid RON"));
    }
}
