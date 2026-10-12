use crate::chunk::WorldPos;
use crate::clock::GameClock;
use crate::terrain::{fertility_at, height_at, resource_richness_at, MAX_STEP_HEIGHT};

/// Axis-aligned footprint in world cells: `[x, x + width) x [z, z + depth)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Footprint {
    pub x: i32,
    pub z: i32,
    pub width: i32,
    pub depth: i32,
}

impl Footprint {
    pub fn overlaps(&self, other: &Footprint, margin: i32) -> bool {
        self.x < other.x + other.width + margin
            && other.x < self.x + self.width + margin
            && self.z < other.z + other.depth + margin
            && other.z < self.z + self.depth + margin
    }

    pub fn contains(&self, c: (i32, i32)) -> bool {
        (self.x..self.x + self.width).contains(&c.0) && (self.z..self.z + self.depth).contains(&c.1)
    }
}

/// A point resource on the map (ore deposit, fertile patch, ...).
#[derive(Debug, Clone, PartialEq)]
pub struct ResourceNode {
    pub kind: String,
    pub cell: (i32, i32),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Building {
    /// Building definition id (from content).
    pub def: String,
    pub footprint: Footprint,
    /// Tick at which the current crop was planted, for farms.
    pub planted_tick: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaceError {
    TooSteep,
    Overlaps,
    InProtectedZone,
    MissingResource(String),
    InsufficientResourceRichness(String),
    InsufficientFertility,
}

/// What a building needs to be placed; filled from content by the caller.
#[derive(Debug, Clone, Default)]
pub struct PlacementRules {
    pub requires_resource: Option<String>,
    /// Minimum footprint-average richness for `requires_resource`, in `[0, 1]`.
    pub minimum_resource_richness: Option<f32>,
    /// Minimum footprint-average soil fertility, in `[0, 1]`.
    pub minimum_fertility: Option<f32>,
    /// Extra empty cells required around other buildings.
    pub building_margin: i32,
}

/// Map-wide things placement is checked against.
#[derive(Debug, Default)]
pub struct Buildings {
    pub buildings: Vec<Building>,
    pub resources: Vec<ResourceNode>,
    /// Areas where the player may not build (e.g. a city claim plus buffer).
    pub protected: Vec<Footprint>,
}

impl Buildings {
    /// Check whether `fp` is a legal placement. Returns the first reason it is not.
    pub fn validate(
        &self,
        seed: u64,
        fp: Footprint,
        rules: &PlacementRules,
    ) -> Result<(), PlaceError> {
        let h = |x: i32, z: i32| height_at(seed, x as f32 + 0.5, z as f32 + 0.5);
        let base = h(fp.x, fp.z);
        for z in fp.z..fp.z + fp.depth {
            for x in fp.x..fp.x + fp.width {
                if (h(x, z) - base).abs() > MAX_STEP_HEIGHT * fp.width.max(fp.depth) as f32 {
                    return Err(PlaceError::TooSteep);
                }
            }
        }
        if self.protected.iter().any(|p| p.overlaps(&fp, 0)) {
            return Err(PlaceError::InProtectedZone);
        }
        if self
            .buildings
            .iter()
            .any(|b| b.footprint.overlaps(&fp, rules.building_margin))
        {
            return Err(PlaceError::Overlaps);
        }
        if let Some(kind) = &rules.requires_resource {
            if let Some(minimum) = rules.minimum_resource_richness {
                if average_resource_richness(seed, fp, kind) <= minimum {
                    return Err(PlaceError::InsufficientResourceRichness(kind.clone()));
                }
            } else if !self
                .resources
                .iter()
                .any(|r| &r.kind == kind && fp.contains(r.cell))
            {
                return Err(PlaceError::MissingResource(kind.clone()));
            }
        }
        if rules
            .minimum_fertility
            .is_some_and(|minimum| average_fertility(seed, fp) <= minimum)
        {
            return Err(PlaceError::InsufficientFertility);
        }
        Ok(())
    }

    /// Validate and place; returns the new building's index.
    pub fn place(
        &mut self,
        seed: u64,
        def: &str,
        fp: Footprint,
        rules: &PlacementRules,
    ) -> Result<usize, PlaceError> {
        self.validate(seed, fp, rules)?;
        self.buildings.push(Building {
            def: def.to_string(),
            footprint: fp,
            planted_tick: None,
        });
        Ok(self.buildings.len() - 1)
    }

    /// True if a cell is blocked by a building (for pathfinding obstacles).
    pub fn blocks(&self, cell: (i32, i32)) -> bool {
        self.buildings.iter().any(|b| b.footprint.contains(cell))
    }

    pub fn center(&self, i: usize) -> WorldPos {
        let f = self.buildings[i].footprint;
        WorldPos::new(
            f.x as f32 + f.width as f32 / 2.0,
            0.0,
            f.z as f32 + f.depth as f32 / 2.0,
        )
    }
}

/// Average resource richness across all cells covered by a footprint.
pub fn average_resource_richness(seed: u64, fp: Footprint, resource: &str) -> f32 {
    footprint_average(fp, |x, z| {
        resource_richness_at(seed, resource, x as f32 + 0.5, z as f32 + 0.5)
    })
}

/// Average soil fertility across all cells covered by a footprint.
pub fn average_fertility(seed: u64, fp: Footprint) -> f32 {
    footprint_average(fp, |x, z| {
        fertility_at(seed, x as f32 + 0.5, z as f32 + 0.5)
    })
}

fn footprint_average(fp: Footprint, sample: impl Fn(i32, i32) -> f32) -> f32 {
    let mut total = 0.0;
    let mut count = 0;
    for z in fp.z..fp.z + fp.depth {
        for x in fp.x..fp.x + fp.width {
            total += sample(x, z);
            count += 1;
        }
    }
    if count == 0 {
        0.0
    } else {
        total / count as f32
    }
}

/// Crop growth progress in `[0, 1]`, computed from the clock on demand (no per-tick cost).
pub fn crop_progress(clock: &GameClock, planted_tick: u64, now: u64, growth_days: f32) -> f32 {
    let days = clock.days_elapsed(now.saturating_sub(planted_tick)) as f32;
    (days / growth_days.max(f32::EPSILON)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::TICKS_PER_SECOND;

    fn fp(x: i32, z: i32, w: i32, d: i32) -> Footprint {
        Footprint {
            x,
            z,
            width: w,
            depth: d,
        }
    }

    #[test]
    fn overlap_and_margin() {
        assert!(fp(0, 0, 4, 4).overlaps(&fp(3, 3, 4, 4), 0));
        assert!(!fp(0, 0, 4, 4).overlaps(&fp(4, 0, 4, 4), 0));
        assert!(fp(0, 0, 4, 4).overlaps(&fp(5, 0, 4, 4), 2));
    }

    #[test]
    fn placement_rules() {
        let mut b = Buildings::default();
        let rules = PlacementRules::default();
        assert!(b.place(1, "house", fp(0, 0, 2, 2), &rules).is_ok());
        assert_eq!(
            b.validate(1, fp(1, 1, 2, 2), &rules),
            Err(PlaceError::Overlaps)
        );
        b.protected.push(fp(100, 100, 50, 50));
        assert_eq!(
            b.validate(1, fp(110, 110, 2, 2), &rules),
            Err(PlaceError::InProtectedZone)
        );
        let mine = PlacementRules {
            requires_resource: Some("iron_ore".into()),
            minimum_resource_richness: None,
            minimum_fertility: None,
            building_margin: 0,
        };
        assert_eq!(
            b.validate(1, fp(10, 10, 2, 2), &mine),
            Err(PlaceError::MissingResource("iron_ore".into()))
        );
        b.resources.push(ResourceNode {
            kind: "iron_ore".into(),
            cell: (11, 11),
        });
        assert!(b.validate(1, fp(10, 10, 2, 2), &mine).is_ok());

        let rich_site = PlacementRules {
            requires_resource: Some("iron_ore".into()),
            minimum_resource_richness: Some(0.0),
            minimum_fertility: None,
            building_margin: 0,
        };
        assert!(b.validate(1, fp(10, 10, 1, 1), &rich_site).is_ok());
        let impossible_richness = PlacementRules {
            minimum_resource_richness: Some(1.0),
            ..rich_site.clone()
        };
        assert_eq!(
            b.validate(1, fp(10, 10, 1, 1), &impossible_richness),
            Err(PlaceError::InsufficientResourceRichness("iron_ore".into()))
        );
        let fertile_site = PlacementRules {
            minimum_fertility: Some(0.0),
            ..PlacementRules::default()
        };
        assert!(b.validate(1, fp(10, 10, 1, 1), &fertile_site).is_ok());
        let impossible_fertility = PlacementRules {
            minimum_fertility: Some(1.0),
            ..PlacementRules::default()
        };
        assert_eq!(
            b.validate(1, fp(10, 10, 1, 1), &impossible_fertility),
            Err(PlaceError::InsufficientFertility)
        );
        assert!(b.blocks((1, 1)) && !b.blocks((5, 5)));
    }

    #[test]
    fn crop_growth_follows_clock() {
        let clock = GameClock {
            day_length_seconds: 10.0,
        };
        let day = 10 * TICKS_PER_SECOND as u64;
        assert_eq!(crop_progress(&clock, 0, 0, 3.0), 0.0);
        assert!((crop_progress(&clock, 0, day * 3 / 2, 3.0) - 0.5).abs() < 1e-5);
        assert_eq!(crop_progress(&clock, 0, day * 10, 3.0), 1.0);
    }
}
