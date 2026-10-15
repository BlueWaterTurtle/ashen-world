//! Data-driven composition entry point: which content domains and simulation systems are enabled.

use crate::ContentError;
use serde::Deserialize;
use std::path::Path;

/// Contents of `manifest.ron`. Names refer to [`crate::ContentDomain::NAME`] and to
/// `core_sim` system names; `content` does not depend on `core_sim`, so systems are plain strings.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Manifest {
    #[serde(default)]
    pub content_domains: Vec<String>,
    #[serde(default)]
    pub systems: Vec<String>,
}

impl Manifest {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ContentError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|source| ContentError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        ron::from_str(&text).map_err(|source| ContentError::Parse {
            path: path.to_path_buf(),
            source,
        })
    }

    pub fn has_domain(&self, name: &str) -> bool {
        self.content_domains.iter().any(|d| d == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ContentDomain;

    #[test]
    fn shipped_manifest_lists_every_domain() {
        let m = Manifest::load(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content/manifest.ron"),
        )
        .unwrap();
        for name in [
            crate::Faction::NAME,
            crate::UnitArchetype::NAME,
            crate::Biome::NAME,
            crate::Race::NAME,
            crate::Behavior::NAME,
            crate::BuildingDef::NAME,
            crate::Crop::NAME,
        ] {
            assert!(m.has_domain(name), "manifest missing domain {name}");
        }
        assert!(m.systems.iter().any(|s| s == "core_step"));
    }
}
