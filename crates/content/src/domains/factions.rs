use super::ContentDomain;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Faction {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub hostile_by_default: bool,
}

impl ContentDomain for Faction {
    const NAME: &'static str = "factions";
    const KIND: &'static str = "faction";

    fn id(&self) -> &str {
        &self.id
    }
}
