//! Data-driven composition entry point: which content domains and simulation systems are enabled.

use crate::ContentError;
use serde::Deserialize;
use std::path::Path;

/// Contents of `manifest.ron`. Names refer to [`crate::ContentDomain::NAME`] and to
/// `core_sim` system names; `content` does not depend on `core_sim`, so systems are plain strings.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Manifest {
    #[serde(default = "default_domains")]
    pub content_domains: Vec<String>,
    #[serde(default = "default_systems")]
    pub systems: Vec<String>,
    /// Ids of enabled mods (`assets/mods/<id>/`); omitted = none.
    #[serde(default)]
    pub mods: Vec<String>,
}

fn default_domains() -> Vec<String> {
    use crate::ContentDomain as D;
    [
        crate::Faction::NAME,
        crate::Race::NAME,
        crate::Behavior::NAME,
        crate::Biome::NAME,
        crate::Crop::NAME,
        crate::BuildingDef::NAME,
        crate::UnitArchetype::NAME,
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

fn default_systems() -> Vec<String> {
    vec!["core_step".to_string()]
}

/// The built-in composition: every content domain and the `core_step` system. Used when the
/// manifest is missing or invalid, and for omitted fields.
impl Default for Manifest {
    fn default() -> Self {
        Self {
            content_domains: default_domains(),
            systems: default_systems(),
            mods: Vec::new(),
        }
    }
}

impl Manifest {
    /// Load `path`, falling back to [`Manifest::default`] (with the error returned for
    /// reporting) when it is missing or invalid.
    pub fn load_or_default(path: impl AsRef<Path>) -> (Self, Option<ContentError>) {
        match Self::load(path) {
            Ok(m) => (m, None),
            Err(e) => (Self::default(), Some(e)),
        }
    }

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
    fn missing_or_invalid_manifest_falls_back_to_default() {
        let (m, err) = Manifest::load_or_default("/nonexistent/manifest.ron");
        assert_eq!(m, Manifest::default());
        assert!(err.is_some());

        let dir = std::env::temp_dir().join("ashen_world_bad_manifest");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("manifest.ron");
        std::fs::write(&path, "(content_domains: [").unwrap();
        let (m, err) = Manifest::load_or_default(&path);
        assert_eq!(m, Manifest::default());
        assert!(matches!(err, Some(ContentError::Parse { .. })));
    }

    #[test]
    fn omitted_fields_use_defaults() {
        let m: Manifest = ron::from_str("()").unwrap();
        assert_eq!(m, Manifest::default());
    }

    #[test]
    fn shipped_manifest_matches_default_composition() {
        let m = Manifest::load(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content/manifest.ron"),
        )
        .unwrap();
        assert_eq!(m, Manifest::default());
    }

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
