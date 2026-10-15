use crate::world::TICKS_PER_SECOND;
use serde::{Deserialize, Serialize};

/// Simulation-side movement behaviour parameters. Plain data so the sim stays free of
/// content-loading dependencies; the client maps content definitions into this.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Behavior {
    /// Wander destinations are within this many cells (per axis). 0 = stay put.
    pub wander_range: i32,
    pub pause_ticks_min: u32,
    pub pause_ticks_max: u32,
    /// Walking speed in meters per second.
    pub speed: f32,
    /// Whether an agent with this behaviour can be recruited into player control.
    pub recruitable: bool,
}

impl Behavior {
    pub fn from_seconds(wander_range: i32, pause_min_s: f32, pause_max_s: f32, speed: f32) -> Self {
        let t = |s: f32| (s.max(0.0) * TICKS_PER_SECOND as f32).round() as u32;
        let (min, max) = (t(pause_min_s), t(pause_max_s));
        Self {
            wander_range: wander_range.max(0),
            pause_ticks_min: min,
            pause_ticks_max: max.max(min),
            speed: speed.max(0.0),
            recruitable: true,
        }
    }

    pub fn with_recruitable(mut self, recruitable: bool) -> Self {
        self.recruitable = recruitable;
        self
    }
}

impl Default for Behavior {
    fn default() -> Self {
        Self::from_seconds(40, 1.0, 5.0, 3.0)
    }
}
