//! Startup composition: scenario -> manifest -> schedule -> content -> world config.
//! Free of winit/wgpu so it can be tested without a display.

use std::fmt;
use std::path::Path;

use core_sim::{Behavior, Buildings, Jobs, Schedule, World, WorldConfig};

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
    /// Mods applied (in order), mods skipped with reasons, and records they replaced.
    pub mods: content::ModReport,
    pub save_loaded: Option<String>,
    pub save_fallback: Option<String>,
}

impl StartupSummary {
    pub fn any_fallback(&self) -> bool {
        self.scenario_fallback.is_some()
            || self.manifest_fallback.is_some()
            || self.content_fallback.is_some()
            || !self.unknown_systems.is_empty()
            || !self.mods.skipped.is_empty()
            || self.save_fallback.is_some()
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
        if let Some(path) = &self.save_loaded {
            writeln!(f, "startup: save loaded: {path}")?;
        }
        if let Some(reason) = &self.save_fallback {
            writeln!(f, "startup: save fallback: {reason}; using a fresh world")?;
        }
        for n in &self.unknown_systems {
            writeln!(f, "startup: unknown system '{n}' ignored")?;
        }
        if !self.mods.loaded.is_empty() {
            let list: Vec<String> = self
                .mods
                .loaded
                .iter()
                .map(|m| format!("{} {}", m.id, m.version))
                .collect();
            writeln!(f, "startup: mods [{}]", list.join(", "))?;
        }
        for s in &self.mods.skipped {
            writeln!(f, "startup: mod '{}' skipped: {}", s.id, s.reason)?;
        }
        for o in &self.mods.overrides {
            writeln!(
                f,
                "startup: mod '{}' overrides {} '{}'",
                o.mod_id, o.domain, o.id
            )?;
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
    pub loaded_state: Option<LoadedGameState>,
}

pub struct LoadedGameState {
    pub world: World,
    pub buildings: Buildings,
    pub jobs: Jobs,
}

/// Compose startup from `assets_root` (`content/`, `scenarios/` and `mods/`); `selector` defaults to
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
    let mut mods = content::ModReport::default();
    let (mut config, building_defs) =
        match content::load_content_with_mods(&content_dir, &manifest, assets_root.join("mods")) {
            Ok((c, w, report)) => {
                warnings = w;
                mods = report;
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
        mods,
        save_loaded: None,
        save_fallback: None,
    };
    Startup {
        config,
        building_defs,
        schedule,
        summary,
        loaded_state: None,
    }
}

/// Compose and optionally restore a save. An unreadable or incompatible save is
/// reported in the summary and leaves startup with a fresh world.
pub fn bootstrap_with_load(
    assets_root: &Path,
    selector: Option<&str>,
    load_source: Option<&str>,
) -> Startup {
    let Some(load_source) = load_source else {
        return bootstrap(assets_root, selector);
    };
    let path = crate::save::resolve_load_path(load_source);
    let envelope = match crate::save::read_save(&path) {
        Ok(envelope) => envelope,
        Err(error) => {
            let mut startup = bootstrap(assets_root, selector);
            startup.summary.save_fallback = Some(error);
            return startup;
        }
    };

    let mut startup = bootstrap(assets_root, Some(&envelope.metadata.scenario_selector));
    let current = crate::save::SaveMetadata::from_summary(&startup.summary);
    let differences = envelope.metadata.differences(&current);
    if !differences.is_empty() {
        startup.summary.save_fallback = Some(format!(
            "save composition is incompatible: {}",
            differences.join("; ")
        ));
        return startup;
    }

    match World::from_snapshot(envelope.world) {
        Ok(world) => {
            startup.loaded_state = Some(LoadedGameState {
                world,
                buildings: envelope.buildings,
                jobs: envelope.jobs,
            });
            startup.summary.save_loaded = Some(path.display().to_string());
        }
        Err(error) => {
            startup.summary.save_fallback =
                Some(format!("save world snapshot is invalid: {error}"));
        }
    }
    startup
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
        assert!(s.loaded_state.is_none());
        assert!(!s.summary.any_fallback(), "{}", s.summary);
        assert_eq!(s.summary.scenario_name, "Default");
        assert_eq!(s.config.seed, WorldConfig::default().seed);
        assert_eq!(s.config.agent_count, WorldConfig::default().agent_count);
        assert_eq!(s.summary.systems, vec!["core_step"]);
        assert_eq!(s.summary.content_domains.len(), 7);
        assert!(!s.building_defs.is_empty());
        assert!(s.summary.warnings.is_empty());
        assert_eq!(s.summary.mods, content::ModReport::default());
        assert!(s.summary.to_string().contains("fallbacks: none"));
    }

    #[test]
    fn valid_save_restores_the_saved_world_display_free() {
        let fresh = bootstrap(&assets(), None);
        let metadata = crate::save::SaveMetadata::from_summary(&fresh.summary);
        let mut world = World::generate(&fresh.config);
        world.tick = 47;
        let envelope = crate::save::SaveEnvelope::capture(
            metadata,
            &world,
            &Buildings::default(),
            &Jobs::default(),
        );
        let path = std::env::temp_dir().join(format!(
            "ashen_world_bootstrap_load_{}.ron",
            std::process::id()
        ));
        crate::save::write_save(&path, &envelope).unwrap();

        let startup = bootstrap_with_load(&assets(), None, path.to_str());
        assert_eq!(startup.loaded_state.as_ref().unwrap().world.tick, 47);
        assert!(startup.summary.save_loaded.is_some());
        assert!(startup.summary.save_fallback.is_none());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn corrupt_save_falls_back_to_a_fresh_world_with_summary_reason() {
        let path = std::env::temp_dir().join(format!(
            "ashen_world_bootstrap_corrupt_{}.ron",
            std::process::id()
        ));
        std::fs::write(&path, "not RON").unwrap();
        let startup = bootstrap_with_load(&assets(), None, path.to_str());
        assert!(startup.loaded_state.is_none());
        assert!(startup.summary.save_fallback.is_some());
        assert!(startup
            .summary
            .to_string()
            .contains("save fallback: invalid save"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn missing_mod_metadata_is_reported_and_save_is_not_loaded() {
        let fresh = bootstrap(&assets(), None);
        let mut metadata = crate::save::SaveMetadata::from_summary(&fresh.summary);
        metadata.mods.push(crate::save::SaveMod {
            id: "missing_mod".into(),
            version: "1.0".into(),
        });
        let world = World::generate(&fresh.config);
        let envelope = crate::save::SaveEnvelope::capture(
            metadata,
            &world,
            &Buildings::default(),
            &Jobs::default(),
        );
        let path = std::env::temp_dir().join(format!(
            "ashen_world_bootstrap_missing_mod_{}.ron",
            std::process::id()
        ));
        crate::save::write_save(&path, &envelope).unwrap();

        let startup = bootstrap_with_load(&assets(), None, path.to_str());
        assert!(startup.loaded_state.is_none());
        assert!(startup
            .summary
            .save_fallback
            .as_ref()
            .unwrap()
            .contains("loaded mods changed"));
        assert!(startup.summary.to_string().contains("missing_mod"));
        std::fs::remove_file(path).unwrap();
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

    #[test]
    fn example_mod_scenario_overlays_content() {
        let s = bootstrap(&assets(), Some("example_mod"));
        assert!(!s.summary.any_fallback(), "{}", s.summary);
        assert_eq!(s.summary.mods.loaded[0].id, "example_mod");
        assert!(s
            .summary
            .mods
            .overrides
            .iter()
            .any(|o| o.domain == "units" && o.id == "wanderer"));
        let text = s.summary.to_string();
        assert!(text.contains("mods [example_mod 0.1.0]"), "{text}");
        assert!(text.contains("overrides units 'wanderer'"), "{text}");
        let base = bootstrap(&assets(), None);
        assert_ne!(s.config.behaviors, base.config.behaviors);
        assert_eq!(s.config.agent_count, base.config.agent_count);
    }

    #[test]
    fn missing_mod_is_skipped_without_failing_startup() {
        let root = std::env::temp_dir().join("ashen_world_bootstrap_missing_mod");
        std::fs::create_dir_all(root.join("scenarios")).unwrap();
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
        std::fs::write(
            root.join("scenarios/t.ron"),
            r#"(name: "T", mods: Some(["ghost"]))"#,
        )
        .unwrap();
        let s = bootstrap(&root, Some("t"));
        assert!(s.summary.content_fallback.is_none());
        assert_eq!(s.summary.mods.skipped.len(), 1);
        assert!(s.summary.any_fallback());
        assert!(s.summary.to_string().contains("mod 'ghost' skipped"));
        assert!(!s.building_defs.is_empty());
    }
}
