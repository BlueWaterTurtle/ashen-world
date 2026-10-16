use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use core_sim::{Buildings, Jobs, World, WorldSnapshot};
use serde::{Deserialize, Serialize};

/// Upgrades a save by exactly one format version, in place.
type Migration = fn(&mut SaveEnvelope);

/// Ordered upgrade steps: entry `i` upgrades a version `i + 1` save to version `i + 2`.
///
/// Older saves are deserialized straight into the current types, so a migration can only
/// rely on added fields having `#[serde(default)]`. Fixing up values is fine; removing or
/// retyping a field needs a legacy struct for the old shape first.
const MIGRATIONS: &[Migration] = &[];

/// The version this build writes; it follows from the number of migrations.
pub const FORMAT_VERSION: u32 = MIGRATIONS.len() as u32 + 1;
pub const QUICK_SAVE_PATH: &str = "saves/quicksave.ron";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveMod {
    pub id: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveMetadata {
    pub scenario_selector: String,
    pub scenario_name: String,
    pub manifest_source: String,
    pub content_domains: Vec<String>,
    pub systems: Vec<String>,
    pub mods: Vec<SaveMod>,
    pub timestamp_unix_seconds: u64,
}

impl SaveMetadata {
    pub fn from_summary(summary: &crate::bootstrap::StartupSummary) -> Self {
        Self {
            scenario_selector: summary.scenario_selector.clone(),
            scenario_name: summary.scenario_name.clone(),
            manifest_source: summary.manifest_source.clone(),
            content_domains: summary.content_domains.clone(),
            systems: summary
                .systems
                .iter()
                .map(|system| (*system).to_string())
                .collect(),
            mods: summary
                .mods
                .loaded
                .iter()
                .map(|loaded| SaveMod {
                    id: loaded.id.clone(),
                    version: loaded.version.clone(),
                })
                .collect(),
            timestamp_unix_seconds: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        }
    }

    pub fn differences(&self, current: &Self) -> Vec<String> {
        let mut differences = Vec::new();
        if self.scenario_selector != current.scenario_selector {
            differences.push(format!(
                "scenario selector changed (saved '{}', current '{}')",
                self.scenario_selector, current.scenario_selector
            ));
        }
        if self.scenario_name != current.scenario_name {
            differences.push(format!(
                "scenario changed (saved '{}', current '{}')",
                self.scenario_name, current.scenario_name
            ));
        }
        if self.manifest_source != current.manifest_source {
            differences.push(format!(
                "manifest source changed (saved '{}', current '{}')",
                self.manifest_source, current.manifest_source
            ));
        }
        if self.content_domains != current.content_domains {
            differences.push(format!(
                "content domains changed (saved [{}], current [{}])",
                self.content_domains.join(", "),
                current.content_domains.join(", ")
            ));
        }
        if self.systems != current.systems {
            differences.push(format!(
                "systems changed (saved [{}], current [{}])",
                self.systems.join(", "),
                current.systems.join(", ")
            ));
        }
        if self.mods != current.mods {
            differences.push(format!(
                "loaded mods changed (saved [{}], current [{}])",
                format_mods(&self.mods),
                format_mods(&current.mods)
            ));
        }
        differences
    }
}

fn format_mods(mods: &[SaveMod]) -> String {
    mods.iter()
        .map(|mod_info| format!("{} {}", mod_info.id, mod_info.version))
        .collect::<Vec<_>>()
        .join(", ")
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SaveEnvelope {
    pub format_version: u32,
    pub metadata: SaveMetadata,
    pub world: WorldSnapshot,
    pub buildings: Buildings,
    pub jobs: Jobs,
}

impl SaveEnvelope {
    pub fn capture(
        metadata: SaveMetadata,
        world: &World,
        buildings: &Buildings,
        jobs: &Jobs,
    ) -> Self {
        Self {
            format_version: FORMAT_VERSION,
            metadata,
            world: world.snapshot(),
            buildings: buildings.clone(),
            jobs: jobs.clone(),
        }
    }

    /// Validate this save against the running composition and rebuild the game state.
    pub fn restore(self, current: &SaveMetadata) -> Result<(World, Buildings, Jobs), String> {
        let differences = self.metadata.differences(current);
        if !differences.is_empty() {
            return Err(format!(
                "save composition is incompatible: {}",
                differences.join("; ")
            ));
        }
        let world = World::from_snapshot(self.world)
            .map_err(|error| format!("invalid world snapshot: {error}"))?;
        Ok((world, self.buildings, self.jobs))
    }
}

pub fn resolve_load_path(source: &str) -> PathBuf {
    let path = Path::new(source);
    if path.exists() || path.extension().is_some() || path.components().count() > 1 {
        path.to_path_buf()
    } else {
        Path::new("saves").join(format!("{source}.ron"))
    }
}

pub fn read_save(path: impl AsRef<Path>) -> Result<SaveEnvelope, String> {
    let path = path.as_ref();
    let text = fs::read_to_string(path)
        .map_err(|error| format!("could not read save '{}': {error}", path.display()))?;
    let header: SaveHeader = ron::from_str(&text)
        .map_err(|error| format!("invalid save '{}': {error}", path.display()))?;
    // Reject unknown versions before the full parse, which could fail confusingly.
    check_version(header.format_version, MIGRATIONS)?;
    let mut envelope: SaveEnvelope = ron::from_str(&text)
        .map_err(|error| format!("invalid save '{}': {error}", path.display()))?;
    migrate(&mut envelope, MIGRATIONS)?;
    Ok(envelope)
}

/// Just the version field, readable even when the rest of the save has a different shape.
#[derive(Deserialize)]
struct SaveHeader {
    format_version: u32,
}

fn check_version(version: u32, migrations: &[Migration]) -> Result<(), String> {
    let supported = migrations.len() as u32 + 1;
    if version == 0 || version > supported {
        return Err(format!(
            "unsupported save format version {version} (supported version is {supported})"
        ));
    }
    Ok(())
}

/// Brings `envelope` up to the newest format by applying each pending migration in order.
fn migrate(envelope: &mut SaveEnvelope, migrations: &[Migration]) -> Result<(), String> {
    check_version(envelope.format_version, migrations)?;
    for step in &migrations[(envelope.format_version - 1) as usize..] {
        step(envelope);
    }
    envelope.format_version = migrations.len() as u32 + 1;
    Ok(())
}

pub fn write_save(path: impl AsRef<Path>, envelope: &SaveEnvelope) -> Result<(), String> {
    static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);
    let path = path.as_ref();
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    if let Some(parent) = parent {
        fs::create_dir_all(parent)
            .map_err(|error| format!("could not create save directory: {error}"))?;
    }

    let serialized = ron::ser::to_string_pretty(envelope, ron::ser::PrettyConfig::default())
        .map_err(|error| format!("could not serialize save: {error}"))?;
    let parent = parent.unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .ok_or_else(|| "save path has no file name".to_string())?;
    let (temp_path, mut temp_file) = loop {
        let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let temp_path = parent.join(format!(
            ".{}.{}.{}.tmp",
            file_name.to_string_lossy(),
            std::process::id(),
            id
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
        {
            Ok(file) => break (temp_path, file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("could not create temporary save: {error}")),
        }
    };

    let result = (|| {
        temp_file.write_all(serialized.as_bytes())?;
        temp_file.sync_all()?;
        drop(temp_file);
        fs::rename(&temp_path, path)?;
        Ok::<(), std::io::Error>(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temp_path);
        return Err(format!(
            "could not atomically write save '{}': {error}",
            path.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_sim::{BuildingLogistics, Entity, Footprint, Job, PlacementRules, WorldConfig};

    fn metadata() -> SaveMetadata {
        SaveMetadata {
            scenario_selector: "default".into(),
            scenario_name: "Default".into(),
            manifest_source: "content/manifest.ron".into(),
            content_domains: vec!["units".into()],
            systems: vec!["core_step".into()],
            mods: vec![SaveMod {
                id: "example".into(),
                version: "1.0".into(),
            }],
            timestamp_unix_seconds: 123,
        }
    }

    fn envelope() -> SaveEnvelope {
        let world = World::generate(&WorldConfig {
            agent_count: 4,
            ..Default::default()
        });
        let mut buildings = Buildings::default();
        let building = buildings
            .place_with_logistics(
                world.seed,
                world.terrain_edits(),
                "mine",
                Footprint {
                    x: 0,
                    z: 0,
                    width: 2,
                    depth: 2,
                },
                &PlacementRules::default(),
                BuildingLogistics {
                    worker_slots: 1,
                    input_slots: 2,
                    output_slots: 3,
                    stack_limit: 10,
                    is_mine: true,
                    ..Default::default()
                },
            )
            .unwrap();
        buildings.buildings[building]
            .inputs
            .insert_resource("wood", 4);
        buildings.buildings[building]
            .mine
            .as_mut()
            .unwrap()
            .progress = 1.25;
        let mut jobs = Jobs::default();
        jobs.add_job(Entity(0), Job::Work { building });
        SaveEnvelope::capture(metadata(), &world, &buildings, &jobs)
    }

    #[test]
    fn save_round_trip_and_atomic_write_leave_no_temporary_file() {
        let dir = std::env::temp_dir().join(format!(
            "ashen_world_save_round_trip_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("slot.ron");
        let expected = envelope();
        write_save(&path, &expected).unwrap();
        let loaded = read_save(&path).unwrap();
        assert_eq!(loaded.metadata, expected.metadata);
        assert_eq!(loaded.world, expected.world);
        assert_eq!(loaded.format_version, FORMAT_VERSION);
        assert_eq!(loaded.buildings.buildings.len(), 1);
        assert!(loaded.buildings.sites.is_empty());
        assert_eq!(loaded.buildings.buildings[0].inputs.count_of("wood"), 4);
        assert_eq!(
            loaded.buildings.buildings[0]
                .mine
                .as_ref()
                .unwrap()
                .progress,
            1.25
        );
        assert_eq!(loaded.jobs.jobs(Entity(0)), &[Job::Work { building: 0 }]);
        let files: Vec<_> = fs::read_dir(&dir).unwrap().collect();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].as_ref().unwrap().file_name(), "slot.ron");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn newer_format_version_is_rejected_clearly() {
        let dir =
            std::env::temp_dir().join(format!("ashen_world_save_version_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("newer.ron");
        let mut saved = envelope();
        saved.format_version += 1;
        write_save(&path, &saved).unwrap();
        let error = read_save(&path).unwrap_err();
        assert!(error.contains("unsupported save format version"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn migrations_run_in_order_from_the_saved_version() {
        fn append_a(e: &mut SaveEnvelope) {
            e.metadata.scenario_name.push('a');
        }
        fn append_b(e: &mut SaveEnvelope) {
            e.metadata.scenario_name.push('b');
        }
        let chain: &[Migration] = &[append_a, append_b];

        let mut from_v1 = envelope();
        from_v1.format_version = 1;
        migrate(&mut from_v1, chain).unwrap();
        assert_eq!(from_v1.metadata.scenario_name, "Defaultab");
        assert_eq!(from_v1.format_version, 3);

        let mut from_v2 = envelope();
        from_v2.format_version = 2;
        migrate(&mut from_v2, chain).unwrap();
        assert_eq!(from_v2.metadata.scenario_name, "Defaultb");
        assert_eq!(from_v2.format_version, 3);

        let mut current = envelope();
        current.format_version = 3;
        migrate(&mut current, chain).unwrap();
        assert_eq!(current.metadata.scenario_name, "Default");
    }

    #[test]
    fn migration_rejects_zero_and_future_versions_without_changes() {
        let chain: &[Migration] = &[|e| e.metadata.scenario_name.push('x')];
        for version in [0, 3] {
            let mut saved = envelope();
            saved.format_version = version;
            let error = migrate(&mut saved, chain).unwrap_err();
            assert!(error.contains("unsupported save format version"));
            assert_eq!(saved.metadata.scenario_name, "Default");
        }
    }

    #[test]
    fn checked_in_v1_save_still_loads() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/save_v1.ron");
        let loaded = read_save(path).unwrap();
        assert_eq!(loaded.format_version, FORMAT_VERSION);
        assert_eq!(loaded.buildings.buildings.len(), 1);
        assert!(loaded.buildings.sites.is_empty());
        assert_eq!(loaded.buildings.buildings[0].inputs.count_of("wood"), 4);
        assert_eq!(loaded.jobs.jobs(Entity(0)), &[Job::Work { building: 0 }]);
    }

    #[test]
    fn restore_rejects_incompatible_metadata_and_accepts_matching() {
        let mut current = metadata();
        current.mods.clear();
        let error = envelope().restore(&current).err().unwrap();
        assert!(error.contains("save composition is incompatible"));
        assert!(error.contains("loaded mods changed"));

        let (world, buildings, jobs) = envelope().restore(&metadata()).unwrap();
        assert_eq!(world.snapshot(), envelope().world);
        assert_eq!(buildings.buildings.len(), 1);
        assert_eq!(jobs.jobs(Entity(0)), &[Job::Work { building: 0 }]);
    }

    #[test]
    fn metadata_changes_name_missing_mod_and_version() {
        let mut current = metadata();
        current.content_domains.clear();
        current.mods[0].version = "2.0".into();
        let differences = metadata().differences(&current).join("; ");
        assert!(differences.contains("content domains changed"));
        assert!(differences.contains("loaded mods changed"));
    }

    #[test]
    fn production_chain_survives_save_load_and_continues_identically() {
        use core_sim::{ResourceKind, JOB_CHECK_INTERVAL_TICKS};
        let mut world = World::generate(&WorldConfig {
            agent_count: 2,
            ..Default::default()
        });
        let worker = Entity(0);
        let hauler = Entity(1);
        let mut buildings = Buildings::default();
        let mut place = |x: i32, logistics: BuildingLogistics| {
            buildings
                .place_with_logistics(
                    world.seed,
                    world.terrain_edits(),
                    "b",
                    Footprint {
                        x,
                        z: 0,
                        width: 2,
                        depth: 2,
                    },
                    &PlacementRules::default(),
                    logistics,
                )
                .unwrap()
        };
        let mine = place(
            0,
            BuildingLogistics {
                worker_slots: 1,
                output_slots: 1,
                output_resources: vec!["iron_ore".into()],
                is_mine: true,
                ..Default::default()
            },
        );
        let shop = place(
            10,
            BuildingLogistics {
                worker_slots: 1,
                input_slots: 1,
                output_slots: 1,
                input_resources: vec!["iron_ore".into()],
                output_resources: vec!["iron_ingot".into()],
                required_inputs: vec!["iron_ore".into()],
                is_smelter: true,
                ..Default::default()
            },
        );
        let mut jobs = Jobs::default();
        jobs.add_job(worker, Job::Work { building: mine });
        jobs.add_job(
            hauler,
            Job::Haul {
                from: mine,
                to: shop,
                item: ResourceKind::IronOre,
            },
        );
        let step = |world: &mut World, buildings: &mut Buildings, jobs: &mut Jobs, at: usize| {
            for who in [worker, hauler] {
                let p = buildings.buildings[at].worker_slot_position(0);
                world.set_position(who, p);
            }
            world.tick = JOB_CHECK_INTERVAL_TICKS * (world.tick / JOB_CHECK_INTERVAL_TICKS + 1);
            jobs.update(world, buildings);
            buildings.tick_production(2.0);
        };
        // Mine ore, then pick it up: the hauler is mid-trip carrying ore.
        for _ in 0..3 {
            step(&mut world, &mut buildings, &mut jobs, mine);
        }
        assert!(world.inventory(hauler).unwrap().count_of("iron_ore") > 0);

        let saved = ron::to_string(&SaveEnvelope::capture(
            metadata(),
            &world,
            &buildings,
            &jobs,
        ))
        .unwrap();
        let loaded: SaveEnvelope = ron::from_str(&saved).unwrap();
        let mut world2 = World::from_snapshot(loaded.world).unwrap();
        let (mut buildings2, mut jobs2) = (loaded.buildings, loaded.jobs);
        assert_eq!(buildings2.buildings, buildings.buildings);
        assert_eq!(
            world2.inventory(hauler).unwrap().count_of("iron_ore"),
            world.inventory(hauler).unwrap().count_of("iron_ore")
        );

        // Deliver ore to the workshop and keep going on both timelines.
        for at in [shop, shop, shop, shop] {
            step(&mut world, &mut buildings, &mut jobs, at);
            step(&mut world2, &mut buildings2, &mut jobs2, at);
        }
        assert_eq!(buildings2.buildings, buildings.buildings);
        assert!(buildings.buildings[shop].inputs.count_of("iron_ore") > 0);
    }

    /// Stocked wood store, a requested 15-wood site, and one builder with a Build job.
    fn construction_state() -> (World, Buildings, Jobs, usize, usize) {
        use core_sim::ResourceKind;
        let world = World::generate(&WorldConfig {
            agent_count: 2,
            ..Default::default()
        });
        let mut buildings = Buildings::default();
        let store = buildings
            .place_with_logistics(
                world.seed,
                world.terrain_edits(),
                "wood_storage",
                Footprint {
                    x: 0,
                    z: 0,
                    width: 2,
                    depth: 2,
                },
                &PlacementRules::default(),
                BuildingLogistics {
                    input_slots: 1,
                    output_slots: 1,
                    is_storage: true,
                    ..Default::default()
                },
            )
            .unwrap();
        buildings.buildings[store]
            .outputs
            .insert_resource("wood", 30);
        let site = buildings
            .request_construction(
                world.seed,
                world.terrain_edits(),
                "ore_storage",
                Footprint {
                    x: 10,
                    z: 0,
                    width: 2,
                    depth: 2,
                },
                &PlacementRules::default(),
                BuildingLogistics {
                    input_slots: 1,
                    output_slots: 1,
                    is_storage: true,
                    ..Default::default()
                },
                ResourceKind::Wood,
                15,
            )
            .unwrap();
        let mut jobs = Jobs::default();
        jobs.add_job(Entity(0), Job::Build { site, from: store });
        (world, buildings, jobs, store, site)
    }

    fn construction_step(
        world: &mut World,
        buildings: &mut Buildings,
        jobs: &mut Jobs,
        store: usize,
        site: usize,
    ) {
        use core_sim::JOB_CHECK_INTERVAL_TICKS;
        let w = Entity(0);
        let carrying = world.inventory(w).unwrap().count_of("wood") > 0;
        let at = if carrying || buildings.site_needs(site) == 0 {
            buildings.sites[site].worker_position()
        } else {
            buildings.buildings[store].worker_slot_position(0)
        };
        world.set_position(w, at);
        world.tick = JOB_CHECK_INTERVAL_TICKS * (world.tick / JOB_CHECK_INTERVAL_TICKS + 1);
        jobs.update(world, buildings);
        buildings.tick_production(2.0);
    }

    fn reload(world: &World, buildings: &Buildings, jobs: &Jobs) -> (World, Buildings, Jobs) {
        let saved =
            ron::to_string(&SaveEnvelope::capture(metadata(), world, buildings, jobs)).unwrap();
        let loaded: SaveEnvelope = ron::from_str(&saved).unwrap();
        (
            World::from_snapshot(loaded.world).unwrap(),
            loaded.buildings,
            loaded.jobs,
        )
    }

    #[test]
    fn in_progress_construction_survives_save_load_and_finishes_identically() {
        let (mut world, mut buildings, mut jobs, store, site) = construction_state();
        for _ in 0..4 {
            construction_step(&mut world, &mut buildings, &mut jobs, store, site);
        }
        assert!(buildings.sites[site].delivered > 0);
        assert!(buildings.sites[site].completed.is_none());
        assert_eq!(buildings.sites[site].builder, Some(Entity(0)));
        assert!(buildings.sites[site].progress > 0.0);

        let (mut world2, mut buildings2, mut jobs2) = reload(&world, &buildings, &jobs);
        assert_eq!(buildings2.sites, buildings.sites);
        assert_eq!(buildings2.buildings, buildings.buildings);
        assert_eq!(
            buildings2.sites[site].material,
            core_sim::ResourceKind::Wood
        );
        assert_eq!(buildings2.sites[site].required, 15);
        assert_eq!(
            buildings2.sites[site].delivered,
            buildings.sites[site].delivered
        );
        assert_eq!(buildings2.site_needs(site), buildings.site_needs(site));
        assert_eq!(
            buildings2.sites[site].progress,
            buildings.sites[site].progress
        );
        assert_eq!(jobs2.jobs(Entity(0)), jobs.jobs(Entity(0)));

        for _ in 0..12 {
            construction_step(&mut world, &mut buildings, &mut jobs, store, site);
            construction_step(&mut world2, &mut buildings2, &mut jobs2, store, site);
        }
        assert!(buildings2.sites[site].completed.is_some());
        assert_eq!(buildings2.sites, buildings.sites);
        assert_eq!(buildings2.buildings, buildings.buildings);
    }

    #[test]
    fn completed_construction_survives_save_load() {
        let (mut world, mut buildings, mut jobs, store, site) = construction_state();
        for _ in 0..20 {
            construction_step(&mut world, &mut buildings, &mut jobs, store, site);
        }
        let built = buildings.sites[site].completed.expect("completed");
        let (_, buildings2, jobs2) = reload(&world, &buildings, &jobs);
        assert_eq!(buildings2.sites[site].completed, Some(built));
        assert_eq!(buildings2.buildings[built].def, "ore_storage");
        assert!(buildings2.buildings[built].is_storage);
        assert_eq!(buildings2.buildings, buildings.buildings);
        assert!(jobs2.jobs(Entity(0)).is_empty());
    }
}
