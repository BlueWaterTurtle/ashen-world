//! Minimal boundary between simulation systems and the world data they operate on.

use crate::world::World;

/// A unit of simulation logic that runs once per tick against the [`World`] data.
pub trait SimSystem {
    /// Stable name, matching the entries in `assets/content/manifest.ron`.
    fn name(&self) -> &'static str;

    fn run(&mut self, world: &mut World);
}

/// Ordered list of systems executed each tick.
#[derive(Default)]
pub struct Schedule {
    systems: Vec<Box<dyn SimSystem>>,
}

impl Schedule {
    pub fn new() -> Self {
        Self::default()
    }

    /// The built-in schedule reproducing [`World::step`].
    pub fn standard() -> Self {
        let mut s = Self::new();
        s.add(CoreStepSystem);
        s
    }

    /// Build a schedule from system names, in order. Returns the schedule and any names that
    /// are not registered (those are skipped).
    pub fn from_names<S: AsRef<str>>(names: &[S]) -> (Self, Vec<String>) {
        let mut schedule = Self::new();
        let mut unknown = Vec::new();
        for name in names {
            match name.as_ref() {
                "core_step" => {
                    schedule.add(CoreStepSystem);
                }
                other => unknown.push(other.to_string()),
            }
        }
        (schedule, unknown)
    }

    /// Like [`Schedule::from_names`], but falls back to [`Schedule::standard`] when no
    /// listed system is known.
    pub fn from_names_or_standard<S: AsRef<str>>(names: &[S]) -> (Self, Vec<String>) {
        let (schedule, unknown) = Self::from_names(names);
        if schedule.systems.is_empty() {
            (Self::standard(), unknown)
        } else {
            (schedule, unknown)
        }
    }

    pub fn add(&mut self, system: impl SimSystem + 'static) -> &mut Self {
        self.systems.push(Box::new(system));
        self
    }

    pub fn system_names(&self) -> Vec<&'static str> {
        self.systems.iter().map(|s| s.name()).collect()
    }

    /// Run every system once, in insertion order.
    pub fn run(&mut self, world: &mut World) {
        for s in &mut self.systems {
            s.run(world);
        }
    }
}

/// Wraps the existing per-tick world update (terrain edits, LOD, wandering, movement).
pub struct CoreStepSystem;

impl SimSystem for CoreStepSystem {
    fn name(&self) -> &'static str {
        "core_step"
    }

    fn run(&mut self, world: &mut World) {
        world.step();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::WorldConfig;

    struct Counter(u32);
    impl SimSystem for Counter {
        fn name(&self) -> &'static str {
            "counter"
        }
        fn run(&mut self, _: &mut World) {
            self.0 += 1;
        }
    }

    #[test]
    fn standard_schedule_matches_direct_stepping() {
        let cfg = WorldConfig::default();
        let mut a = World::generate(&cfg);
        let mut b = World::generate(&cfg);
        let mut schedule = Schedule::standard();
        for _ in 0..20 {
            schedule.run(&mut a);
            b.step();
        }
        assert_eq!(a.tick, b.tick);
        assert_eq!(schedule.system_names(), vec!["core_step"]);
    }

    #[test]
    fn from_names_builds_in_order_and_reports_unknown() {
        let (s, unknown) = Schedule::from_names(&["core_step", "nope", "core_step"]);
        assert_eq!(s.system_names(), vec!["core_step", "core_step"]);
        assert_eq!(unknown, vec!["nope"]);
    }

    #[test]
    fn empty_or_unknown_names_fall_back_to_standard() {
        let none: [&str; 0] = [];
        let (s, _) = Schedule::from_names_or_standard(&none);
        assert_eq!(s.system_names(), vec!["core_step"]);
        let (s, unknown) = Schedule::from_names_or_standard(&["bogus"]);
        assert_eq!(s.system_names(), vec!["core_step"]);
        assert_eq!(unknown, vec!["bogus"]);
    }

    #[test]
    fn systems_run_in_order() {
        let mut w = World::generate(&WorldConfig::default());
        let mut s = Schedule::new();
        s.add(Counter(0)).add(CoreStepSystem);
        s.run(&mut w);
        assert_eq!(w.tick, 1);
        assert_eq!(s.system_names(), vec!["counter", "core_step"]);
    }
}
