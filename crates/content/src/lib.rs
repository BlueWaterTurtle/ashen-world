//! Serializable content definitions and JSON loading.

use serde::{Deserialize, Serialize};
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

/// Load `factions.json`, `units.json` and `biomes.json` from `dir` and validate them.
pub fn load_content(dir: impl AsRef<Path>) -> Result<Content, ContentError> {
    let dir = dir.as_ref();
    let fp = dir.join("factions.json");
    let up = dir.join("units.json");
    let bp = dir.join("biomes.json");
    let factions: Vec<Faction> = load_file(&fp)?;
    let units: Vec<UnitArchetype> = load_file(&up)?;
    let biomes: Vec<Biome> = load_file(&bp)?;

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
