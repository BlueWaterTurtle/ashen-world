//! Mod overlays on top of base content.
//!
//! Layout: `<mods_dir>/<mod_id>/mod.ron` (a [`ModDescriptor`]) plus optional `<domain>.ron` files
//! (same format as base). Mods are enabled by id via `Manifest::mods` (or the scenario).
//!
//! Semantics: mods apply in order of `(load_order, id)` after base content. A record whose id
//! already exists replaces it in place (override); a new id is appended. Duplicate ids within one
//! layer are an error. After each mod is merged, the whole result is re-validated (including
//! cross-domain references); a mod that fails to load or validate is skipped and reported, and the
//! content before that mod is kept.

use crate::{
    check_content, check_unique, invalid, load_file, BuildingDef, Content, ContentDomain,
    ContentError, Manifest, UnitArchetype,
};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Contents of a mod's `mod.ron`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ModDescriptor {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: String,
    /// Lower loads first; ties are broken by id.
    #[serde(default)]
    pub load_order: i32,
    /// Mod ids that must be enabled and loaded before this mod.
    #[serde(default)]
    pub dependencies: Vec<String>,
}

/// A mod that was enabled but not applied.
#[derive(Debug, Clone, PartialEq)]
pub struct SkippedMod {
    pub id: String,
    pub reason: String,
}

/// A base (or earlier mod) record replaced by a mod.
#[derive(Debug, Clone, PartialEq)]
pub struct ModOverride {
    pub mod_id: String,
    pub domain: &'static str,
    pub id: String,
}

/// What mod loading did, in application order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModReport {
    pub loaded: Vec<ModDescriptor>,
    pub skipped: Vec<SkippedMod>,
    pub overrides: Vec<ModOverride>,
}

fn valid_mod_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn load_descriptor(mods_dir: &Path, id: &str) -> Result<ModDescriptor, ContentError> {
    let path = mods_dir.join(id).join("mod.ron");
    if !valid_mod_id(id) {
        return Err(invalid(&path, format!("invalid mod id '{id}'")));
    }
    let text = std::fs::read_to_string(&path).map_err(|source| ContentError::Io {
        path: path.clone(),
        source,
    })?;
    let d: ModDescriptor = ron::from_str(&text).map_err(|source| ContentError::Parse {
        path: path.clone(),
        source,
    })?;
    if d.id != id {
        return Err(invalid(
            &path,
            format!("descriptor id '{}' does not match directory '{id}'", d.id),
        ));
    }
    Ok(d)
}

/// Merge `<dir>/<domain>.ron` into `target`: replace by id, else append.
fn merge_domain<T: ContentDomain>(
    dir: &Path,
    mod_id: &str,
    target: &mut Vec<T>,
    overrides: &mut Vec<ModOverride>,
) -> Result<(), ContentError> {
    let path = dir.join(format!("{}.ron", T::NAME));
    if !path.exists() {
        return Ok(());
    }
    let items: Vec<T> = load_file(&path)?;
    check_unique(&path, items.iter().map(|i| i.id()))?;
    for item in &items {
        item.validate().map_err(|m| invalid(&path, m))?;
    }
    for item in items {
        match target.iter().position(|t| t.id() == item.id()) {
            Some(i) => {
                overrides.push(ModOverride {
                    mod_id: mod_id.to_string(),
                    domain: T::NAME,
                    id: item.id().to_string(),
                });
                target[i] = item;
            }
            None => target.push(item),
        }
    }
    Ok(())
}

fn apply_mod(
    dir: &Path,
    d: &ModDescriptor,
    manifest: &Manifest,
    content: &Content,
) -> Result<(Content, Vec<ModOverride>, Vec<String>), ContentError> {
    let mut c = content.clone();
    let mut ov = Vec::new();
    macro_rules! domain {
        ($field:ident, $ty:ty) => {
            if manifest.has_domain(<$ty>::NAME) {
                merge_domain::<$ty>(dir, &d.id, &mut c.$field, &mut ov)?;
            }
        };
    }
    domain!(factions, crate::Faction);
    domain!(units, UnitArchetype);
    domain!(biomes, crate::Biome);
    domain!(races, crate::Race);
    domain!(behaviors, crate::Behavior);
    domain!(buildings, BuildingDef);
    domain!(crops, crate::Crop);
    let warnings = check_content(
        &c,
        manifest,
        &dir.join(format!("{}.ron", UnitArchetype::NAME)),
        &dir.join(format!("{}.ron", BuildingDef::NAME)),
    )?;
    Ok((c, ov, warnings))
}

/// Load base content from `dir` like [`crate::load_content_with_warnings`], then overlay the mods
/// enabled in `manifest.mods` from `mods_dir`. With no mods the result is identical. Only a base
/// content failure is an error; bad mods are skipped and listed in the [`ModReport`].
pub fn load_content_with_mods(
    dir: impl AsRef<Path>,
    manifest: &Manifest,
    mods_dir: impl AsRef<Path>,
) -> Result<(Content, Vec<String>, ModReport), ContentError> {
    let (dir, mods_dir) = (dir.as_ref(), mods_dir.as_ref());
    let (mut content, mut warnings) = crate::load_content_with_warnings(dir, manifest)?;
    let mut report = ModReport::default();

    let mut seen = BTreeSet::new();
    let mut descriptors = Vec::new();
    for id in &manifest.mods {
        if !seen.insert(id.as_str()) {
            continue;
        }
        match load_descriptor(mods_dir, id) {
            Ok(d) => descriptors.push(d),
            Err(e) => report.skipped.push(SkippedMod {
                id: id.clone(),
                reason: e.to_string(),
            }),
        }
    }
    descriptors.sort_by(|a, b| (a.load_order, &a.id).cmp(&(b.load_order, &b.id)));

    for d in descriptors {
        let missing: Vec<&str> = d
            .dependencies
            .iter()
            .filter(|dep| !report.loaded.iter().any(|l| &l.id == *dep))
            .map(String::as_str)
            .collect();
        if !missing.is_empty() {
            report.skipped.push(SkippedMod {
                id: d.id.clone(),
                reason: format!("dependencies not loaded before it: {}", missing.join(", ")),
            });
            continue;
        }
        let mod_dir: PathBuf = mods_dir.join(&d.id);
        match apply_mod(&mod_dir, &d, manifest, &content) {
            Ok((c, ov, w)) => {
                content = c;
                warnings = w;
                report.overrides.extend(ov);
                report.loaded.push(d);
            }
            Err(e) => report.skipped.push(SkippedMod {
                id: d.id.clone(),
                reason: e.to_string(),
            }),
        }
    }
    Ok((content, warnings, report))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assets() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")
    }

    fn manifest(mods: &[&str]) -> Manifest {
        Manifest {
            mods: mods.iter().map(|s| s.to_string()).collect(),
            ..Manifest::default()
        }
    }

    /// Temp mods dir with `(mod_id, [(file, text)])` entries.
    fn mods_dir(name: &str, mods: &[(&str, &[(&str, &str)])]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("ashen_world_mods_{name}"));
        let _ = std::fs::remove_dir_all(&root);
        for (id, files) in mods {
            let d = root.join(id);
            std::fs::create_dir_all(&d).unwrap();
            for (f, t) in *files {
                std::fs::write(d.join(f), t).unwrap();
            }
        }
        root
    }

    fn desc(id: &str, order: i32, deps: &str) -> String {
        format!(
            r#"(id: "{id}", name: "{id}", version: "1", load_order: {order}, dependencies: {deps})"#
        )
    }

    fn unit(id: &str, hp: f32, faction: &str) -> String {
        format!(
            r#"(id: "{id}", name: "{id}", faction: "{faction}", max_health: {hp}, move_speed: 3.0, race: "human", behavior: "stationary")"#
        )
    }

    fn load(m: &Manifest, dir: &Path) -> (Content, Vec<String>, ModReport) {
        load_content_with_mods(assets().join("content"), m, dir).unwrap()
    }

    #[test]
    fn no_mods_equals_base_load() {
        let base =
            crate::load_content_with_warnings(assets().join("content"), &Manifest::default())
                .unwrap();
        let (c, w, r) = load(&Manifest::default(), Path::new("/nonexistent"));
        assert_eq!((c, w), base);
        assert_eq!(r, ModReport::default());
    }

    #[test]
    fn override_and_add() {
        let units = format!(
            "[{}, {}]",
            unit("wanderer", 500.0, "wanderers"),
            unit("scout", 50.0, "wanderers")
        );
        let d = desc("m", 0, "[]");
        let dir = mods_dir("oa", &[("m", &[("mod.ron", &d), ("units.ron", &units)])]);
        let base = crate::load_content(assets().join("content")).unwrap();
        let (c, _, r) = load(&manifest(&["m"]), &dir);
        assert_eq!(c.units.len(), base.units.len() + 1);
        assert_eq!(c.units[0].id, "wanderer");
        assert_eq!(c.units[0].max_health, 500.0);
        assert!(c.units.iter().any(|u| u.id == "scout"));
        assert_eq!(r.loaded.len(), 1);
        assert_eq!(
            r.overrides,
            vec![ModOverride {
                mod_id: "m".into(),
                domain: "units",
                id: "wanderer".into()
            }]
        );
    }

    #[test]
    fn order_is_deterministic_and_later_wins() {
        let (da, db) = (desc("a", 5, "[]"), desc("b", 1, "[]"));
        let ua = format!("[{}]", unit("wanderer", 200.0, "wanderers"));
        let ub = format!("[{}]", unit("wanderer", 300.0, "wanderers"));
        let dir = mods_dir(
            "order",
            &[
                ("a", &[("mod.ron", &da), ("units.ron", &ua)]),
                ("b", &[("mod.ron", &db), ("units.ron", &ub)]),
            ],
        );
        for list in [["a", "b"], ["b", "a"]] {
            let (c, _, r) = load(&manifest(&list), &dir);
            let ids: Vec<_> = r.loaded.iter().map(|d| d.id.as_str()).collect();
            assert_eq!(ids, ["b", "a"]);
            assert_eq!(c.units[0].max_health, 200.0);
        }
    }

    #[test]
    fn missing_invalid_and_dependency_mods_are_skipped() {
        let bad = desc("bad", 0, "[]");
        let dep = desc("dep", 0, r#"["absent"]"#);
        let dir = mods_dir(
            "skip",
            &[
                ("bad", &[("mod.ron", &bad), ("units.ron", "[")]),
                ("dep", &[("mod.ron", &dep)]),
                ("mismatch", &[("mod.ron", &desc("other", 0, "[]"))]),
            ],
        );
        let (c, _, r) = load(
            &manifest(&["ghost", "bad", "dep", "mismatch", "../x"]),
            &dir,
        );
        assert_eq!(c, crate::load_content(assets().join("content")).unwrap());
        assert!(r.loaded.is_empty());
        assert_eq!(r.skipped.len(), 5);
        assert!(r
            .skipped
            .iter()
            .any(|s| s.id == "bad" && s.reason.contains("units.ron")));
        assert!(r
            .skipped
            .iter()
            .any(|s| s.id == "dep" && s.reason.contains("absent")));
    }

    #[test]
    fn dangling_reference_skips_mod_only() {
        let (dg, dk) = (desc("dangling", 0, "[]"), desc("good", 1, "[]"));
        let ug = format!("[{}]", unit("scout", 50.0, "wanderers"));
        let ud = format!("[{}]", unit("ghost", 50.0, "nobody"));
        let dir = mods_dir(
            "dangling",
            &[
                ("dangling", &[("mod.ron", &dg), ("units.ron", &ud)]),
                ("good", &[("mod.ron", &dk), ("units.ron", &ug)]),
            ],
        );
        let (c, _, r) = load(&manifest(&["dangling", "good"]), &dir);
        assert!(c.units.iter().any(|u| u.id == "scout"));
        assert!(!c.units.iter().any(|u| u.id == "ghost"));
        assert_eq!(r.skipped.len(), 1);
        assert_eq!(r.skipped[0].id, "dangling");
        assert!(r.skipped[0].reason.contains("units.ron"));
    }

    #[test]
    fn duplicate_ids_within_a_layer_skip_mod() {
        let d = desc("dup", 0, "[]");
        let u = format!(
            "[{}, {}]",
            unit("x", 1.0, "wanderers"),
            unit("x", 2.0, "wanderers")
        );
        let dir = mods_dir("dup", &[("dup", &[("mod.ron", &d), ("units.ron", &u)])]);
        let (_, _, r) = load(&manifest(&["dup"]), &dir);
        assert!(r.skipped[0].reason.contains("duplicate id"));
    }

    #[test]
    fn mod_can_reference_earlier_mod_content() {
        let (d1, d2) = (
            desc("base_ext", 0, "[]"),
            desc("uses", 1, r#"["base_ext"]"#),
        );
        let f = r#"[(id: "nomads", name: "Nomads")]"#;
        let u = format!("[{}]", unit("n", 10.0, "nomads"));
        let dir = mods_dir(
            "chain",
            &[
                ("base_ext", &[("mod.ron", &d1), ("factions.ron", f)]),
                ("uses", &[("mod.ron", &d2), ("units.ron", &u)]),
            ],
        );
        let (c, _, r) = load(&manifest(&["uses", "base_ext"]), &dir);
        assert!(r.skipped.is_empty(), "{:?}", r.skipped);
        assert!(c.units.iter().any(|u| u.id == "n"));
    }
}
