//! Mine ore production: a bounded output slot plus a delta-time progress accumulator.

use crate::resource::ResourceKind;
use serde::{Deserialize, Serialize};

/// Worked seconds needed to produce one ore.
pub const ORE_SECONDS_PER_UNIT: f32 = 2.0;

/// Worked seconds needed to smelt one iron ore into one iron ingot.
pub const SMELT_SECONDS_PER_INGOT: f32 = 3.0;

/// A single output slot holding one item type, bounded by that item's `stack_size`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputSlot {
    pub item: ResourceKind,
    pub count: u32,
}

impl OutputSlot {
    pub fn new(item: ResourceKind) -> Self {
        Self { item, count: 0 }
    }

    pub fn is_full(&self) -> bool {
        self.free_space() == 0
    }

    pub fn free_space(&self) -> u32 {
        self.item.stack_size().saturating_sub(self.count)
    }

    /// Insert up to `amount` of `item`; returns how many were inserted.
    pub fn try_insert(&mut self, item: ResourceKind, amount: u32) -> u32 {
        if item != self.item {
            return 0;
        }
        let inserted = amount.min(self.free_space());
        self.count += inserted;
        inserted
    }

    /// Remove up to `amount` items; returns how many were taken.
    pub fn take(&mut self, amount: u32) -> u32 {
        let taken = amount.min(self.count);
        self.count -= taken;
        taken
    }
}

/// Ore production state of a mine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MineProduction {
    pub output: OutputSlot,
    /// Worked seconds accumulated towards the next ore.
    pub progress: f32,
}

impl Default for MineProduction {
    fn default() -> Self {
        Self {
            output: OutputSlot::new(ResourceKind::IronOre),
            progress: 0.0,
        }
    }
}

impl MineProduction {
    /// Advance by `dt` seconds. Progress only accrues while `worked`; when the
    /// output is full production pauses and progress is capped at one unit.
    /// Returns the amount of ore produced.
    pub fn advance(&mut self, dt: f32, worked: bool) -> u32 {
        if !worked || dt <= 0.0 || dt.is_nan() {
            return 0;
        }
        if self.output.is_full() {
            self.progress = self.progress.min(ORE_SECONDS_PER_UNIT);
            return 0;
        }
        self.progress += dt;
        let ready = (self.progress / ORE_SECONDS_PER_UNIT).floor() as u32;
        let inserted = self.output.try_insert(self.output.item, ready);
        self.progress -= inserted as f32 * ORE_SECONDS_PER_UNIT;
        if self.output.is_full() {
            self.progress = self.progress.min(ORE_SECONDS_PER_UNIT);
        }
        inserted
    }
}

/// Smelting state of a workshop: a delta-time progress accumulator.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct SmeltProduction {
    /// Worked seconds accumulated towards the next ingot.
    pub progress: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn produces_after_two_seconds_worked() {
        let mut m = MineProduction::default();
        assert_eq!(m.advance(2.0, true), 1);
        assert_eq!(m.output.count, 1);
    }

    #[test]
    fn nothing_under_two_seconds() {
        let mut m = MineProduction::default();
        assert_eq!(m.advance(1.9, true), 0);
        assert_eq!(m.output.count, 0);
    }

    #[test]
    fn accumulates_across_small_ticks() {
        let mut m = MineProduction::default();
        let mut total = 0;
        for _ in 0..100 {
            total += m.advance(0.05, true);
        }
        assert_eq!(total, 2);
        assert_eq!(m.output.count, 2);
    }

    #[test]
    fn idle_mine_does_not_accrue() {
        let mut m = MineProduction::default();
        assert_eq!(m.advance(10.0, false), 0);
        assert_eq!(m.progress, 0.0);
        assert_eq!(m.advance(1.0, true), 0);
        assert_eq!(m.advance(1.0, true), 1);
    }

    #[test]
    fn large_delta_yields_multiple_and_carries_remainder() {
        let mut m = MineProduction::default();
        assert_eq!(m.advance(7.0, true), 3);
        assert!((m.progress - 1.0).abs() < 1e-6);
        assert_eq!(m.advance(1.0, true), 1);
    }

    #[test]
    fn stops_at_stack_limit_and_resumes() {
        let mut m = MineProduction::default();
        let cap = ResourceKind::IronOre.stack_size();
        m.advance(ORE_SECONDS_PER_UNIT * (cap + 10) as f32, true);
        assert_eq!(m.output.count, cap);
        assert!(m.output.is_full());
        assert!(m.progress <= ORE_SECONDS_PER_UNIT);
        assert_eq!(m.advance(100.0, true), 0);
        assert_eq!(m.output.count, cap);
        assert!(m.progress <= ORE_SECONDS_PER_UNIT);

        assert_eq!(m.output.take(3), 3);
        assert_eq!(m.output.free_space(), 3);
        assert_eq!(m.advance(0.1, true), 1);
        assert_eq!(m.output.count, cap - 2);
    }

    #[test]
    fn slot_helpers() {
        let mut s = OutputSlot::new(ResourceKind::IronOre);
        assert_eq!(s.try_insert(ResourceKind::Wheat, 5), 0);
        assert_eq!(
            s.try_insert(ResourceKind::IronOre, 1000),
            s.item.stack_size()
        );
        assert!(s.is_full());
        assert_eq!(s.take(1000), s.item.stack_size());
        assert_eq!(s.count, 0);
    }
}
