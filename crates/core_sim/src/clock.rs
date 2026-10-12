use crate::world::TICKS_PER_SECOND;

/// In-game time derived from the tick counter. O(1) per query, no per-tick work.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GameClock {
    /// Real seconds of simulation per in-game day.
    pub day_length_seconds: f32,
}

impl Default for GameClock {
    fn default() -> Self {
        // 24 in-game minutes per day.
        Self {
            day_length_seconds: 24.0 * 60.0,
        }
    }
}

impl GameClock {
    fn ticks_per_day(&self) -> u64 {
        ((self.day_length_seconds.max(1.0) * TICKS_PER_SECOND as f32) as u64).max(1)
    }

    /// Zero-based day number at `tick`.
    pub fn day(&self, tick: u64) -> u64 {
        tick / self.ticks_per_day()
    }

    /// Fraction of the day elapsed in `[0, 1)`; 0 = midnight, 0.5 = noon.
    pub fn time_of_day(&self, tick: u64) -> f32 {
        (tick % self.ticks_per_day()) as f32 / self.ticks_per_day() as f32
    }

    /// Fractional days elapsed, handy for slow processes such as crop growth.
    pub fn days_elapsed(&self, tick: u64) -> f64 {
        tick as f64 / self.ticks_per_day() as f64
    }

    pub fn is_night(&self, tick: u64) -> bool {
        let t = self.time_of_day(tick);
        !(0.25..0.75).contains(&t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_and_time_of_day() {
        let c = GameClock {
            day_length_seconds: 10.0,
        };
        let per_day = 10 * TICKS_PER_SECOND as u64;
        assert_eq!(c.day(0), 0);
        assert_eq!(c.day(per_day), 1);
        assert!((c.time_of_day(per_day / 2) - 0.5).abs() < 1e-6);
        assert!(c.is_night(0) && !c.is_night(per_day / 2));
        assert!((c.days_elapsed(per_day * 3 / 2) - 1.5).abs() < 1e-9);
    }
}
