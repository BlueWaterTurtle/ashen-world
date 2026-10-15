//! Startup composition: scenario -> manifest -> schedule -> content -> world config.
//! Free of winit/wgpu so it can be tested without a display.

use std::fmt;
use std::path::Path;

use core_sim::{Behavior, Schedule, WorldConfig};

/// What was selected at boot and which fallbacks were taken.
#[derive(Debug, Clone, PartialEq)]
pub struct StartupSummary {
    pub scenario_name: String,
    pub scenario_selector: String,
    /// The requested scenario failed to load and the built-in default was used.
    pub scenario_fallback: Option<String>,
    pub manifest_source: String,
    pub manifest_fallback: Option<String>,
    pub content_domains: Vec<String>,
    pub systems: Vec<&'static str>,
    pub unknown_systems: Vec<String>,
    pub content_fallback: Option<String>,
    pub warnings: Vec<String>,
}

impl StartupSummary {
    pub fn any_fallback(&self) -> bool {
        self.scenario_fallback.is_some()
            || self.manifest_fallback.is_some()
            || self.content_fallback.is_some()
            || !self.unknown_systems.is_empty()
    }
}

impl fmt::Display for StartupSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let source = if self.scenario_fallback.is_some() {
            "fallback default"
        } else {
            "requested"
        };
        writeln!(
            f,
            "startup: scenario '{}' ({source}: {}), manifest: {}",
            self.scenario_name, self.scenario_selector, self.manifest_source
        )?;
        writeln!(f, "startup: domains [{}]", self.content_domains.join(", "))?;
        writeln!(f, "startup: systems [{}]", self.systems.join(", "))?;
        writeln!(
            f,
            "startup: fallbacks: {}",
            if self.any_fallback() { "yes" } else { "none" }
        )?;
        if let Some(e) = &self.scenario_fallback {
            writeln!(f, "startup: scenario fallback: {e}")?;
        }
        if let Some(e) = &self.manifest_fallback {
            writeln!(f, "startup: manifest fallback: {e}")?;
        }
        if let Some(e) = &self.content_fallback {
            writeln!(f, "startup: content load failed: {e}; using defaults")?;
        }
        for n in &self.unknown_systems {
            writeln!(f, "startup: unknown system '{n}' ignored")?;
        }
        for w in &self.warnings {
            writeln!(f, "startup: warning: {w}")?;
        }
        Ok(())
    }
}

pub struct Startup {
    pub config: WorldConfig,
    pub building_defs: Vec<content::BuildingDef>,
    pub schedule: Schedule,
    pub summary: StartupSummary,
}

/// Compose startup from `assets_root` (`content/` and `scenarios/`); `selector` defaults to
/// `default`. Never fails: every step falls back and records it in the summary.
pub fn bootstrap(assets_root: &Path, selector: Option<&str>) -> Startup {
    let content_dir = assets_root.join("content");
    let selector = selector.unwrap_or("default");
    let scenario_path = content::Scenario::resolve_path(&assets_root.join("scenarios"), selector);
    let (scenario, scenario_err) = content::Scenario::load_or_default(scenario_path);
    let (manifest, manifest_err) = scenario.resolve_manifest(&content_dir);
    let (schedule, unknown_systems) = Schedule::from_names_or_standard(&manifest.systems);

    let mut warnings = Vec::new();
    let mut content_fallback = None;
    let (mut config, building_defs) =
        match content::load_content_with_warnings(&content_dir, &manifest) {
            Ok((c, w)) => {
                warnings = w;
                (
                    WorldConfig {
                        behaviors: behaviors_from_content(&c),
                        ..WorldConfig::default()
                    },
                    c.buildings,
                )
            }
            Err(e) => {
                content_fallback = Some(e.to_string());
                (WorldConfig::default(), Vec::new())
            }
        };
    apply_world_overrides(&mut config, &scenario.world);

    let manifest_source = scenario.manifest_path(&content_dir).display().to_string();
    let summary = StartupSummary {
        scenario_name: scenario.name.clone(),
        scenario_selector: selector.to_string(),
        scenario_fallback: scenario_err.map(|e| e.to_string()),
        manifest_source,
        manifest_fallback: manifest_err.map(|e| e.to_string()),
        content_domains: manifest.content_domains.clone(),
        systems: schedule.system_names(),
        unknown_systems,
        content_fallback,
        warnings,
    };
    Startup {
        config,
        building_defs,
        schedule,
        summary,
    }
}

/// One sim behaviour per unit archetype: the archetype's behaviour definition, with
/// its speed scaled from the archetype's `move_speed` and recruitability mapped from archetype.
pub fn behaviors_from_content(c: &content::Content) -> Vec<Behavior> {
    let behaviors: Vec<Behavior> = c
        .units
        .iter()
        .filter_map(|u| {
            let b = c.behaviors.iter().find(|b| b.id == u.behavior)?;
            Some(
                Behavior::from_seconds(
                    b.wander_range,
                    b.pause_seconds_min,
                    b.pause_seconds_max,
                    u.move_speed * b.speed_multiplier,
                )
                .with_recruitable(u.recruitable),
            )
        })
        .collect();
    if behaviors.is_empty() {
        vec![Behavior::default()]
    } else {
        behaviors
    }
}

pub fn apply_world_overrides(config: &mut WorldConfig, o: &content::WorldOverrides) {
    if let Some(v) = o.seed {
        config.seed = v;
    }
    if let Some(v) = o.agent_count {
        config.agent_count = v;
    }
    if let Some(v) = o.chunk_radius {
        config.chunk_radius = v;
    }
    if let Some(v) = o.max_path_requests_per_tick {
        config.max_path_requests_per_tick = v;
    }
    if let Some(v) = o.starting_player_characters {
        config.starting_player_characters = v;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn assets() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets")
    }

    #[test]
    fn world_overrides_apply_and_default_is_noop() {
        let mut c = WorldConfig::default();
        apply_world_overrides(&mut c, &content::WorldOverrides::default());
        assert_eq!(c, WorldConfig::default());
        apply_world_overrides(
            &mut c,
            &content::WorldOverrides {
                seed: Some(9),
                agent_count: Some(3),
                ..Default::default()
            },
        );
        assert_eq!((c.seed, c.agent_count), (9, 3));
        assert_eq!(c.chunk_radius, WorldConfig::default().chunk_radius);
    }

    #[test]
    fn default_scenario_preserves_default_behavior() {
        let s = bootstrap(&assets(), None);
        assert!(!s.summary.any_fallback(), "{}", s.summary);
        assert_eq!(s.summary.scenario_name, "Default");
        assert_eq!(s.config.seed, WorldConfig::default().seed);
        assert_eq!(s.config.agent_count, WorldConfig::default().agent_count);
        assert_eq!(s.summary.systems, vec!["core_step"]);
        assert_eq!(s.summary.content_domains.len(), 7);
        assert!(!s.building_defs.is_empty());
        assert!(s.summary.warnings.is_empty());
        assert!(s.summary.to_string().contains("fallbacks: none"));
    }

    #[test]
    fn small_world_applies_overrides() {
        let s = bootstrap(&assets(), Some("small_world"));
        assert!(!s.summary.any_fallback(), "{}", s.summary);
        assert_eq!(s.summary.scenario_name, "Small World");
        assert_eq!(s.config.seed, 42);
        assert_eq!(s.config.agent_count, 50);
        assert_eq!(s.config.chunk_radius, 1);
        assert!(!s.building_defs.is_empty());
    }

    #[test]
    fn missing_scenario_falls_back_to_default() {
        let s = bootstrap(&assets(), Some("does_not_exist"));
        assert!(s.summary.scenario_fallback.is_some());
        assert!(s.summary.any_fallback());
        assert_eq!(s.summary.scenario_name, "Default");
        assert_eq!(s.config.agent_count, WorldConfig::default().agent_count);
        assert!(s.summary.to_string().contains("fallback default"));
    }

    #[test]
    fn bad_manifest_and_unknown_system_are_reported() {
        let root = std::env::temp_dir().join("ashen_world_bootstrap_bad_manifest");
        let scen = root.join("scenarios");
        std::fs::create_dir_all(&scen).unwrap();
        std::fs::create_dir_all(root.join("content")).unwrap();
        for f in [
            "factions",
            "units",
            "biomes",
            "races",
            "behaviors",
            "buildings",
            "crops",
        ] {
            let name = format!("{f}.ron");
            std::fs::copy(
                assets().join("content").join(&name),
                root.join("content").join(&name),
            )
            .unwrap();
        }
        std::fs::write(root.join("content/bad.ron"), "(content_domains: [").unwrap();
        std::fs::write(
            scen.join("t.ron"),
            r#"(name: "T", manifest: Some("bad.ron"), systems: Some(["nope"]), world: (seed: Some(7)))"#,
        )
        .unwrap();
        let s = bootstrap(&root, Some("t"));
        assert!(s.summary.manifest_fallback.is_some());
        assert_eq!(s.summary.unknown_systems, vec!["nope".to_string()]);
        assert_eq!(s.summary.systems, Schedule::standard().system_names());
        assert_eq!(s.summary.content_domains.len(), 7);
        assert_eq!(s.config.seed, 7);
        assert!(s.summary.any_fallback());
    }

    #[test]
    fn content_failure_keeps_world_overrides() {
        let root = std::env::temp_dir().join("ashen_world_bootstrap_no_content");
        std::fs::create_dir_all(root.join("scenarios")).unwrap();
        std::fs::write(
            root.join("scenarios/t.ron"),
            r#"(name: "T", world: (agent_count: Some(5)))"#,
        )
        .unwrap();
        let s = bootstrap(&root, Some("t"));
        assert!(s.summary.content_fallback.is_some());
        assert!(s.building_defs.is_empty());
        assert_eq!(s.config.agent_count, 5);
    }

    #[test]
    fn disabled_domain_warnings_reach_summary() {
        let root = std::env::temp_dir().join("ashen_world_bootstrap_warn");
        std::fs::create_dir_all(root.join("scenarios")).unwrap();
        std::fs::create_dir_all(root.join("content")).unwrap();
        for f in ["units", "buildings"] {
            let name = format!("{f}.ron");
            std::fs::copy(
                assets().join("content").join(&name),
                root.join("content").join(&name),
            )
            .unwrap();
        }
        std::fs::write(
            root.join("scenarios/t.ron"),
            r#"(name: "T", content_domains: Some(["units", "buildings"]))"#,
        )
        .unwrap();
        let s = bootstrap(&root, Some("t"));
        assert!(!s.summary.warnings.is_empty());
        assert!(s.summary.to_string().contains("warning:"));
    }
}
