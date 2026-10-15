//! Data-driven scenario / game mode: selects composition and startup overrides on top of the
//! manifest. `content` does not depend on `core_sim`, so world overrides are plain optional values.

use crate::{ContentError, Manifest};
use serde::Deserialize;
use std::path::Path;

/// Optional startup overrides; `None` keeps the engine default.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct WorldOverrides {
    pub seed: Option<u64>,
    pub agent_count: Option<usize>,
    pub chunk_radius: Option<i32>,
    pub max_path_requests_per_tick: Option<usize>,
    pub starting_player_characters: Option<usize>,
}

/// Contents of a scenario `.ron` file. The default scenario overrides nothing.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Scenario {
    pub name: String,
    pub description: String,
    /// Manifest file relative to the content directory; `None` uses `manifest.ron`.
    pub manifest: Option<String>,
    /// Inline overrides applied on top of the manifest.
    pub content_domains: Option<Vec<String>>,
    pub systems: Option<Vec<String>>,
    /// Enabled mod ids; replaces the manifest's `mods`. `None` keeps the manifest's (default none).
    pub mods: Option<Vec<String>>,
    pub world: WorldOverrides,
}

impl Scenario {
    pub fn builtin_default() -> Self {
        Self {
            name: "Default".into(),
            ..Self::default()
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

    /// Load `path`, falling back to [`Scenario::builtin_default`] (returning the error) on failure.
    pub fn load_or_default(path: impl AsRef<Path>) -> (Self, Option<ContentError>) {
        match Self::load(path) {
            Ok(s) => (s, None),
            Err(e) => (Self::builtin_default(), Some(e)),
        }
    }

    /// Resolve the scenario's file from a name (`assets/scenarios/<name>.ron`) or an explicit
    /// path (anything with a separator or `.ron` extension).
    pub fn resolve_path(scenarios_dir: &Path, selector: &str) -> std::path::PathBuf {
        if selector.contains(['/', '\\']) || selector.ends_with(".ron") {
            std::path::PathBuf::from(selector)
        } else {
            scenarios_dir.join(format!("{selector}.ron"))
        }
    }

    /// Path of the manifest file this scenario selects (`manifest.ron` when unset).
    pub fn manifest_path(&self, content_dir: &Path) -> std::path::PathBuf {
        content_dir.join(self.manifest.as_deref().unwrap_or("manifest.ron"))
    }

    /// Apply inline `content_domains`/`systems`, which replace (not merge with) the manifest's lists.
    pub fn apply_overrides(&self, mut manifest: Manifest) -> Manifest {
        if let Some(d) = &self.content_domains {
            manifest.content_domains = d.clone();
        }
        if let Some(s) = &self.systems {
            manifest.systems = s.clone();
        }
        if let Some(m) = &self.mods {
            manifest.mods = m.clone();
        }
        manifest
    }

    /// Build the effective manifest: the scenario's (or default) manifest file, then inline
    /// overrides. A missing/invalid manifest falls back to the default; the error is returned.
    pub fn resolve_manifest(&self, content_dir: &Path) -> (Manifest, Option<ContentError>) {
        let (m, err) = Manifest::load_or_default(self.manifest_path(content_dir));
        (self.apply_overrides(m), err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assets() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")
    }

    #[test]
    fn shipped_scenarios_load() {
        let s = Scenario::load(assets().join("scenarios/default.ron")).unwrap();
        assert_eq!(s.name, "Default");
        let s = Scenario::load(assets().join("scenarios/small_world.ron")).unwrap();
        assert_eq!(s.world.seed, Some(42));
        assert_eq!(s.world.agent_count, Some(50));
    }

    #[test]
    fn default_scenario_preserves_default_behavior() {
        let s = Scenario::load(assets().join("scenarios/default.ron")).unwrap();
        assert_eq!(s.world, WorldOverrides::default());
        let (m, err) = s.resolve_manifest(&assets().join("content"));
        assert!(err.is_none());
        assert_eq!(m, Manifest::default());
    }

    #[test]
    fn inline_overrides_apply_to_manifest() {
        let s: Scenario =
            ron::from_str(r#"(content_domains: Some(["factions"]), systems: Some(["core_step"]))"#)
                .unwrap();
        let (m, _) = s.resolve_manifest(&assets().join("content"));
        assert_eq!(m.content_domains, vec!["factions".to_string()]);
    }

    #[test]
    fn resolution_steps_are_independent() {
        let content = Path::new("c");
        let mut s = Scenario::default();
        assert_eq!(s.manifest_path(content), content.join("manifest.ron"));
        s.manifest = Some("m.ron".into());
        assert_eq!(s.manifest_path(content), content.join("m.ron"));
        s.systems = Some(vec!["x".into()]);
        let m = s.apply_overrides(Manifest::default());
        assert_eq!(m.systems, vec!["x".to_string()]);
        assert_eq!(m.content_domains, Manifest::default().content_domains);
    }

    #[test]
    fn missing_or_invalid_scenario_falls_back() {
        let (s, err) = Scenario::load_or_default("/nonexistent/s.ron");
        assert_eq!(s, Scenario::builtin_default());
        assert!(err.is_some());
        let dir = std::env::temp_dir().join("ashen_world_bad_scenario");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("s.ron");
        std::fs::write(&p, "(world: (").unwrap();
        let (s, err) = Scenario::load_or_default(&p);
        assert_eq!(s, Scenario::builtin_default());
        assert!(matches!(err, Some(ContentError::Parse { .. })));
    }

    #[test]
    fn selector_resolves_names_and_paths() {
        let d = Path::new("scn");
        assert_eq!(Scenario::resolve_path(d, "foo"), d.join("foo.ron"));
        assert_eq!(
            Scenario::resolve_path(d, "x/y.ron"),
            std::path::PathBuf::from("x/y.ron")
        );
    }
}
