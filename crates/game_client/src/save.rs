use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use core_sim::{Buildings, Jobs, World, WorldSnapshot};
use serde::{Deserialize, Serialize};

pub const FORMAT_VERSION: u32 = 1;
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
    let envelope: SaveEnvelope = ron::from_str(&text)
        .map_err(|error| format!("invalid save '{}': {error}", path.display()))?;
    if envelope.format_version != FORMAT_VERSION {
        return Err(format!(
            "unsupported save format version {} (supported version is {FORMAT_VERSION})",
            envelope.format_version
        ));
    }
    Ok(envelope)
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
    fn metadata_changes_name_missing_mod_and_version() {
        let mut current = metadata();
        current.content_domains.clear();
        current.mods[0].version = "2.0".into();
        let differences = metadata().differences(&current).join("; ");
        assert!(differences.contains("content domains changed"));
        assert!(differences.contains("loaded mods changed"));
    }
}
