use content::cache::{cache_path, is_cache_valid};
use content::registry::BuildingRegistry;
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

fn building(id: &str, name: &str) -> String {
    format!("(id: \"{id}\", name: \"{name}\", width: 1, depth: 1)")
}

fn write(path: &Path, s: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, s).unwrap();
}

#[test]
fn parses_base_ron() {
    let root = tempfile::tempdir().unwrap();
    write(&root.path().join("assets/content/base/buildings/wall.ron"), &building("building:stone_wall", "Wall"));
    let reg = BuildingRegistry::load_layered(root.path(), "buildings", &root.path().join(".cache/content")).unwrap();
    assert_eq!(reg.len(), 1);
    assert_eq!(reg.get("building:stone_wall").unwrap().name, "Wall");
}

#[test]
fn mod_overrides_and_appends() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    write(&p.join("assets/content/base/buildings/wall.ron"), &building("building:stone_wall", "Wall"));
    write(&p.join("mods/a/buildings/wall.ron"), &building("building:stone_wall", "Modded Wall"));
    write(&p.join("mods/a/buildings/tower.ron"), &building("building:tower", "Tower"));
    let reg = BuildingRegistry::load_layered(p, "buildings", &p.join(".cache/content")).unwrap();
    assert_eq!(reg.len(), 2);
    assert_eq!(reg.get("building:stone_wall").unwrap().name, "Modded Wall");
    assert!(reg.get("building:tower").is_some());
}

#[test]
fn cache_generated_and_stale_detected() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    let cache = p.join(".cache/content");
    let src = p.join("assets/content/base/buildings/wall.ron");
    write(&src, &building("building:stone_wall", "Wall"));
    BuildingRegistry::load_layered(p, "buildings", &cache).unwrap();
    let bin = cache_path(&cache, "base", &src);
    assert!(bin.exists());
    assert!(is_cache_valid(&src, &bin));

    // Make the source newer than the cache: stale, and reload picks up the edit.
    write(&src, &building("building:stone_wall", "Edited"));
    let f = fs::OpenOptions::new().write(true).open(&src).unwrap();
    f.set_modified(SystemTime::now() + Duration::from_secs(10)).unwrap();
    assert!(!is_cache_valid(&src, &bin));
    let reg = BuildingRegistry::load_layered(p, "buildings", &cache).unwrap();
    assert_eq!(reg.get("building:stone_wall").unwrap().name, "Edited");
}
