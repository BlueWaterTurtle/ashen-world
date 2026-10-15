use super::ContentDomain;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Biome {
    pub id: String,
    pub name: String,
    /// Relative harshness in `[0, 1]`.
    pub harshness: f32,
}

impl ContentDomain for Biome {
    const NAME: &'static str = "biomes";
    const KIND: &'static str = "biome";

    fn id(&self) -> &str {
        &self.id
    }

    fn validate(&self) -> Result<(), String> {
        if !(0.0..=1.0).contains(&self.harshness) {
            return Err(format!("biome '{}' harshness must be in [0, 1]", self.id));
        }
        Ok(())
    }
}
