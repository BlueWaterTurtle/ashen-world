use super::ContentDomain;
use serde::{Deserialize, Serialize};

/// A crop: grows over in-game days; the clock drives growth, so there is no per-tick work.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Crop {
    pub id: String,
    pub name: String,
    pub growth_days: f32,
    pub yield_amount: f32,
    pub yield_resource: String,
}

impl ContentDomain for Crop {
    const NAME: &'static str = "crops";
    const KIND: &'static str = "crop";

    fn id(&self) -> &str {
        &self.id
    }

    fn validate(&self) -> Result<(), String> {
        if self.growth_days <= 0.0 || self.yield_amount < 0.0 {
            return Err(format!("crop '{}' has invalid values", self.id));
        }
        Ok(())
    }
}
