//! Layered content registry: base content first, then mods override or append.

use serde::{de::DeserializeOwned, Serialize};
use std::collections::BTreeMap;

/// Content types addressable by a string id such as `"building:stone_wall"`.
pub trait HasId {
    fn id(&self) -> &str;
}

macro_rules! impl_has_id {
    ($($t:ty),*) => {$(impl HasId for $t { fn id(&self) -> &str { &self.id } })*};
}
impl_has_id!(
    crate::BuildingDef,
    crate::Crop,
    crate::Faction,
    crate::UnitArchetype
);

/// Registry keyed by string id. Later insertions replace earlier ones with the same id.
#[derive(Debug, Clone, PartialEq)]
pub struct ContentRegistry<T> {
    entries: BTreeMap<String, T>,
}

impl<T> Default for ContentRegistry<T> {
    fn default() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }
}

pub type BuildingRegistry = ContentRegistry<crate::BuildingDef>;

impl<T> ContentRegistry<T> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert an entry; returns the entry it overrode, if any.
    pub fn insert(&mut self, id: impl Into<String>, value: T) -> Option<T> {
        self.entries.insert(id.into(), value)
    }

    pub fn get(&self, id: &str) -> Option<&T> {
        self.entries.get(id)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &T)> {
        self.entries.iter()
    }
}

impl<T: HasId + Serialize + DeserializeOwned> ContentRegistry<T> {
    /// Load `assets/content/base/<kind>` then each `mods/*/<kind>` (sorted by mod
    /// directory name, i.e. load order), using the binary cache in `cache_dir`.
    pub fn load_layered(
        workspace_root: &std::path::Path,
        kind: &str,
        cache_dir: &std::path::Path,
    ) -> Result<Self, crate::cache::LoadError> {
        let mut reg = Self::new();
        let base = workspace_root.join("assets/content/base");
        crate::cache::load_dir_into(&mut reg, &base.join(kind), "base", cache_dir)?;
        let mods_dir = workspace_root.join("mods");
        let mut mods: Vec<_> = match std::fs::read_dir(&mods_dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect(),
            Err(_) => Vec::new(),
        };
        mods.sort();
        for m in mods {
            let name = m
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            crate::cache::load_dir_into(&mut reg, &m.join(kind), &name, cache_dir)?;
        }
        Ok(reg)
    }
}
