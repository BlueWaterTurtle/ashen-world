use super::ContentDomain;
use serde::{Deserialize, Serialize};

/// Data-driven movement behaviour. `wander_range == 0` means the unit stays put.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Behavior {
    pub id: String,
    pub name: String,
    /// Wander destinations are within this many cells (per axis).
    pub wander_range: i32,
    pub pause_seconds_min: f32,
    pub pause_seconds_max: f32,
    /// Multiplier on the unit's move speed.
    pub speed_multiplier: f32,
}

impl ContentDomain for Behavior {
    const NAME: &'static str = "behaviors";
    const KIND: &'static str = "behavior";

    fn id(&self) -> &str {
        &self.id
    }

    fn validate(&self) -> Result<(), String> {
        if self.wander_range < 0
            || self.pause_seconds_min < 0.0
            || self.pause_seconds_max < self.pause_seconds_min
            || self.speed_multiplier < 0.0
        {
            return Err(format!("behavior '{}' has invalid values", self.id));
        }
        Ok(())
    }
}
