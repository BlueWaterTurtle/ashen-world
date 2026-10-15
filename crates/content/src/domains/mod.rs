//! One module per content domain. To add a domain: create a module with the data type and
//! a [`ContentDomain`] impl, re-export it here, then register it in `Content` and
//! `load_content` (and add it to `assets/content/manifest.ron`).

use std::collections::{BTreeMap, HashSet};

pub mod behaviors;
pub mod biomes;
pub mod buildings;
pub mod crops;
pub mod factions;
pub mod races;
pub mod units;

pub use behaviors::Behavior;
pub use biomes::Biome;
pub use buildings::BuildingDef;
pub use crops::Crop;
pub use factions::Faction;
pub use races::{effective_stat, Race, StatModifier};
pub use units::UnitArchetype;

/// A declared cross-reference from one record to a record of another domain.
#[derive(Debug, Clone, Copy)]
pub struct Reference<'a> {
    /// [`ContentDomain::KIND`] of the referenced domain, e.g. `"race"`.
    pub target: &'static str,
    pub id: &'a str,
}

impl<'a> Reference<'a> {
    pub fn new(target: &'static str, id: &'a str) -> Self {
        Self { target, id }
    }
}

/// A self-describing, independently validated content domain.
pub trait ContentDomain: serde::de::DeserializeOwned {
    /// Domain name; the file is `<NAME>.ron` and the manifest lists it by this name.
    const NAME: &'static str;
    /// Singular label used in references and error messages.
    const KIND: &'static str;

    fn id(&self) -> &str;

    /// Intra-record validation (value ranges and the like).
    fn validate(&self) -> Result<(), String> {
        Ok(())
    }

    /// Cross-domain references this record declares; each must resolve.
    fn references(&self) -> Vec<Reference<'_>> {
        Vec::new()
    }
}

/// Ids known per domain [`ContentDomain::KIND`], used to resolve declared references.
#[derive(Default)]
pub struct KnownIds<'a> {
    ids: BTreeMap<&'static str, HashSet<&'a str>>,
    unchecked: HashSet<&'static str>,
}

impl<'a> KnownIds<'a> {
    pub fn register<T: ContentDomain>(&mut self, items: &'a [T]) {
        self.ids
            .entry(T::KIND)
            .or_default()
            .extend(items.iter().map(|i| i.id()));
    }

    /// Skip references to `kind` (its domain is disabled in the manifest, so ids are unknown).
    pub fn mark_unchecked(&mut self, kind: &'static str) {
        self.unchecked.insert(kind);
    }

    /// Check every declared reference of `items`; returns the first dangling one as a message.
    pub fn check_references<T: ContentDomain>(&self, items: &[T]) -> Result<(), String> {
        for item in items {
            for r in item.references() {
                if self.unchecked.contains(r.target) {
                    continue;
                }
                if !self.ids.get(r.target).is_some_and(|ids| ids.contains(r.id)) {
                    return Err(format!(
                        "{} '{}' references unknown {} '{}'",
                        T::KIND,
                        item.id(),
                        r.target,
                        r.id
                    ));
                }
            }
        }
        Ok(())
    }
}
