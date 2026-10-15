use crate::chunk::{ChunkCoord, CHUNK_SIZE};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

const WALKABILITY_CACHE_CHUNKS: usize = 64;

/// Height samples per chunk edge (one sample per meter-cell).
pub const CELLS: usize = CHUNK_SIZE as usize;

/// Deterministic hash of lattice point + seed to `[0, 1)`.
fn lattice(seed: u64, x: i64, z: i64) -> f32 {
    let mut h = seed ^ (x as u64).wrapping_mul(0x9E37_79B1_85EB_CA87);
    h ^= (z as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h = (h ^ (h >> 33)).wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    h = (h ^ (h >> 33)).wrapping_mul(0xC4CE_B9FE_1A85_EC53);
    h ^= h >> 33;
    (h >> 40) as f32 / (1u64 << 24) as f32
}

fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// Smooth value noise at world coords with feature size `scale`, in `[0, 1)`.
fn value_noise(seed: u64, x: f32, z: f32, scale: f32) -> f32 {
    let (fx, fz) = (x / scale, z / scale);
    let (x0, z0) = (fx.floor(), fz.floor());
    let (tx, tz) = (smooth(fx - x0), smooth(fz - z0));
    let (x0, z0) = (x0 as i64, z0 as i64);
    let a = lattice(seed, x0, z0);
    let b = lattice(seed, x0 + 1, z0);
    let c = lattice(seed, x0, z0 + 1);
    let d = lattice(seed, x0 + 1, z0 + 1);
    let top = a + (b - a) * tx;
    let bot = c + (d - c) * tx;
    top + (bot - top) * tz
}

/// Terrain height in meters (three octaves of noise: broad hills, mid relief, fine detail).
pub fn height_at(seed: u64, x: f32, z: f32) -> f32 {
    let broad = value_noise(seed, x, z, 180.0);
    let mid = value_noise(seed.wrapping_add(1), x, z, 52.0);
    let fine = value_noise(seed.wrapping_add(2), x, z, 14.0);
    (broad * 0.72 + mid * 0.22 + fine * 0.06) * 40.0
}

/// Deterministic mineral richness for a resource kind at a world position, in `[0, 1)`.
pub fn resource_richness_at(seed: u64, resource: &str, x: f32, z: f32) -> f32 {
    let resource_seed = resource
        .bytes()
        .fold(seed ^ 0xA076_1D64_78BD_642F, |hash, byte| {
            hash.wrapping_mul(0xE703_7ED1_A0B4_28DB) ^ u64::from(byte)
        });
    let broad = value_noise(resource_seed, x, z, 150.0);
    let local = value_noise(resource_seed.wrapping_add(1), x, z, 42.0);
    broad * 0.65 + local * 0.35
}

/// Deterministic soil fertility at a world position, in `[0, 1)`.
pub fn fertility_at(seed: u64, x: f32, z: f32) -> f32 {
    let broad = value_noise(seed ^ 0xE703_7ED1_A0B4_28DB, x, z, 180.0);
    let local = value_noise(seed ^ 0x8EBC_6AF0_9C88_C6E3, x, z, 54.0);
    broad * 0.6 + local * 0.4
}

/// Maximum height difference (meters) between adjacent cells that can still be walked.
pub const MAX_STEP_HEIGHT: f32 = 0.2;

/// Whether the 1m cell is walkable on unedited terrain.
pub fn is_walkable(seed: u64, cell: (i32, i32)) -> bool {
    is_walkable_with(seed, &TerrainEdits::default(), cell)
}

/// Whether the 1m cell is walkable: no neighbouring cell is too steep a step away.
pub fn is_walkable_with(seed: u64, edits: &TerrainEdits, cell: (i32, i32)) -> bool {
    let h = |c: (i32, i32)| edits.height(seed, c.0 as f32 + 0.5, c.1 as f32 + 0.5);
    let here = h(cell);
    [(1, 0), (-1, 0), (0, 1), (0, -1)]
        .iter()
        .all(|&(dx, dz)| (h((cell.0 + dx, cell.1 + dz)) - here).abs() <= MAX_STEP_HEIGHT)
}

/// Fixed-point edit resolution: height deltas are stored in millimeters.
const MM_PER_METER: f32 = 1000.0;
/// Largest brush radius (in lattice points) accepted by [`TerrainEdit`].
pub const MAX_BRUSH_RADIUS: i32 = 32;
/// Edited terrain is clamped to this height range, in millimeters.
const MIN_HEIGHT_MM: i32 = 0;
const MAX_HEIGHT_MM: i32 = 100_000;

/// A deterministic terrain-shaping command. Positions are integer lattice points (whole
/// meters); heights are millimeters, so applying an edit uses only integer arithmetic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerrainEdit {
    /// Raise terrain around `center`, strongest in the middle and fading to the radius.
    Raise {
        center: (i32, i32),
        radius: i32,
        amount_mm: i32,
    },
    /// Lower terrain around `center`.
    Lower {
        center: (i32, i32),
        radius: i32,
        amount_mm: i32,
    },
    /// Pull terrain toward the absolute height `target_mm`.
    Flatten {
        center: (i32, i32),
        radius: i32,
        target_mm: i32,
    },
    /// Blend each point toward the average of itself and its four neighbours.
    Smooth { center: (i32, i32), radius: i32 },
    /// Ramp the terrain linearly between the current heights at `from` and `to`, within
    /// `half_width` of the segment joining them.
    Slope {
        from: (i32, i32),
        to: (i32, i32),
        half_width: i32,
    },
}

/// Sparse height edits layered over the procedural terrain.
///
/// Deltas live on the integer lattice (one per meter) and are bilinearly interpolated, so
/// the edited surface stays continuous across chunk borders. Every chunk touched by an
/// edit gets a bumped revision, which caches use to detect stale data.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TerrainEdits {
    deltas: BTreeMap<(i32, i32), i32>,
    revisions: BTreeMap<ChunkCoord, u64>,
}

impl TerrainEdits {
    pub fn is_empty(&self) -> bool {
        self.deltas.is_empty()
    }

    /// Edit revision of a chunk; `0` if it was never touched. Changes whenever an edit
    /// could alter that chunk's heights, normals, or walkability.
    pub fn revision(&self, coord: ChunkCoord) -> u64 {
        self.revisions.get(&coord).copied().unwrap_or(0)
    }

    /// Lattice deltas in a stable order, for saving.
    pub fn entries(&self) -> impl Iterator<Item = ((i32, i32), i32)> + '_ {
        self.deltas.iter().map(|(&p, &d)| (p, d))
    }

    /// Rebuild from saved entries. Every chunk near an entry is marked as changed.
    pub fn from_entries(entries: impl IntoIterator<Item = ((i32, i32), i32)>) -> Self {
        let mut edits = Self::default();
        for (p, d) in entries {
            if d != 0 {
                edits.deltas.insert(p, d);
                edits.mark(p, p);
            }
        }
        edits
    }

    fn delta_mm(&self, p: (i32, i32)) -> i32 {
        self.deltas.get(&p).copied().unwrap_or(0)
    }

    /// Edit offset in meters at a world position.
    pub fn delta_at(&self, x: f32, z: f32) -> f32 {
        if self.deltas.is_empty() {
            return 0.0;
        }
        let (fx, fz) = (x.floor(), z.floor());
        let (tx, tz) = (x - fx, z - fz);
        let (x0, z0) = (fx as i32, fz as i32);
        let d = |dx, dz| self.delta_mm((x0 + dx, z0 + dz)) as f32 / MM_PER_METER;
        let top = d(0, 0) + (d(1, 0) - d(0, 0)) * tx;
        let bottom = d(0, 1) + (d(1, 1) - d(0, 1)) * tx;
        top + (bottom - top) * tz
    }

    /// Terrain height in meters: procedural height plus edits.
    pub fn height(&self, seed: u64, x: f32, z: f32) -> f32 {
        height_at(seed, x, z) + self.delta_at(x, z)
    }

    fn lattice_height_mm(&self, seed: u64, p: (i32, i32)) -> i32 {
        base_mm(seed, p) + self.delta_mm(p)
    }

    fn set_height_mm(&mut self, seed: u64, p: (i32, i32), height_mm: i32) {
        let base = base_mm(seed, p);
        let delta = height_mm.clamp(MIN_HEIGHT_MM, MAX_HEIGHT_MM) - base;
        if delta == 0 {
            self.deltas.remove(&p);
        } else {
            self.deltas.insert(p, delta);
        }
    }

    /// Bump the revision of every chunk whose heights, normals, or walkability could
    /// depend on lattice points in the box `lo..=hi`.
    fn mark(&mut self, lo: (i32, i32), hi: (i32, i32)) -> Vec<ChunkCoord> {
        const REACH: i32 = 3;
        let size = CHUNK_SIZE as i32;
        let mut touched = Vec::new();
        for cz in (lo.1 - REACH).div_euclid(size)..=(hi.1 + REACH).div_euclid(size) {
            for cx in (lo.0 - REACH).div_euclid(size)..=(hi.0 + REACH).div_euclid(size) {
                let coord = ChunkCoord::new(cx, cz);
                *self.revisions.entry(coord).or_insert(0) += 1;
                touched.push(coord);
            }
        }
        touched
    }

    /// Apply one edit and return the chunks whose terrain may have changed.
    pub fn apply(&mut self, seed: u64, edit: TerrainEdit) -> Vec<ChunkCoord> {
        match edit {
            TerrainEdit::Raise {
                center,
                radius,
                amount_mm,
            } => self.brush(seed, center, radius, |_, _, cur, w, r2| {
                cur + (i64::from(amount_mm) * w / r2) as i32
            }),
            TerrainEdit::Lower {
                center,
                radius,
                amount_mm,
            } => self.brush(seed, center, radius, |_, _, cur, w, r2| {
                cur - (i64::from(amount_mm) * w / r2) as i32
            }),
            TerrainEdit::Flatten {
                center,
                radius,
                target_mm,
            } => self.brush(seed, center, radius, |_, _, cur, w, r2| {
                cur + (i64::from(target_mm - cur) * w / r2) as i32
            }),
            TerrainEdit::Smooth { center, radius } => {
                self.brush(seed, center, radius, |edits, p, cur, w, r2| {
                    let sum: i64 = [(0, 0), (1, 0), (-1, 0), (0, 1), (0, -1)]
                        .iter()
                        .map(|&(dx, dz)| {
                            i64::from(edits.lattice_height_mm(seed, (p.0 + dx, p.1 + dz)))
                        })
                        .sum();
                    let average = sum / 5;
                    cur + ((average - i64::from(cur)) * w / r2) as i32
                })
            }
            TerrainEdit::Slope {
                from,
                to,
                half_width,
            } => self.slope(seed, from, to, half_width),
        }
    }

    /// Visit lattice points of a disc in row-major order, passing each point's current
    /// height and falloff weight `r2 - d2` (out of `r2`).
    fn brush(
        &mut self,
        seed: u64,
        center: (i32, i32),
        radius: i32,
        new_height: impl Fn(&Self, (i32, i32), i32, i64, i64) -> i32,
    ) -> Vec<ChunkCoord> {
        let radius = radius.clamp(1, MAX_BRUSH_RADIUS);
        let r2 = i64::from(radius) * i64::from(radius);
        let mut results = Vec::new();
        for z in center.1 - radius..=center.1 + radius {
            for x in center.0 - radius..=center.0 + radius {
                let (dx, dz) = (i64::from(x - center.0), i64::from(z - center.1));
                let w = r2 - (dx * dx + dz * dz);
                if w > 0 {
                    let cur = self.lattice_height_mm(seed, (x, z));
                    results.push(((x, z), new_height(self, (x, z), cur, w, r2)));
                }
            }
        }
        for (p, h) in results {
            self.set_height_mm(seed, p, h);
        }
        self.mark(
            (center.0 - radius, center.1 - radius),
            (center.0 + radius, center.1 + radius),
        )
    }

    fn slope(
        &mut self,
        seed: u64,
        from: (i32, i32),
        to: (i32, i32),
        half_width: i32,
    ) -> Vec<ChunkCoord> {
        let half_width = i64::from(half_width.clamp(1, MAX_BRUSH_RADIUS));
        let (ax, az) = (i64::from(from.0), i64::from(from.1));
        let (sx, sz) = (i64::from(to.0) - ax, i64::from(to.1) - az);
        let len2 = sx * sx + sz * sz;
        let h_from = i64::from(self.lattice_height_mm(seed, from));
        let h_to = i64::from(self.lattice_height_mm(seed, to));
        let pad = half_width as i32;
        let lo = (from.0.min(to.0) - pad, from.1.min(to.1) - pad);
        let hi = (from.0.max(to.0) + pad, from.1.max(to.1) + pad);
        let mut results = Vec::new();
        for z in lo.1..=hi.1 {
            for x in lo.0..=hi.0 {
                let (px, pz) = (i64::from(x) - ax, i64::from(z) - az);
                let (along, across) = if len2 == 0 {
                    (0, px * px + pz * pz)
                } else {
                    let cross = px * sz - pz * sx;
                    (px * sx + pz * sz, cross * cross / len2)
                };
                if (0..=len2).contains(&along) && across <= half_width * half_width {
                    let target = if len2 == 0 {
                        h_from
                    } else {
                        h_from + (h_to - h_from) * along / len2
                    };
                    results.push(((x, z), target as i32));
                }
            }
        }
        for (p, h) in results {
            self.set_height_mm(seed, p, h);
        }
        self.mark(lo, hi)
    }
}

fn base_mm(seed: u64, p: (i32, i32)) -> i32 {
    (height_at(seed, p.0 as f32, p.1 as f32) * MM_PER_METER).round() as i32
}

const CHUNK_CELLS: usize = CHUNK_SIZE as usize;
const BITSET_WORDS: usize = CHUNK_CELLS * CHUNK_CELLS / 64;

/// Bounded cache of walkability for cells visited by pathfinding.
///
/// Each chunk is stored as a bitset in a fixed slot array (FIFO replacement), and
/// the most recently used slot is remembered so consecutive lookups in the same
/// chunk (the common case during A*) skip hashing entirely.
#[derive(Debug, Default)]
pub(crate) struct WalkabilityCache {
    slots: Vec<(ChunkCoord, Box<[u64; BITSET_WORDS]>)>,
    index: HashMap<ChunkCoord, usize>,
    next_evict: usize,
    last: Option<(ChunkCoord, usize)>,
}

impl WalkabilityCache {
    #[inline]
    pub(crate) fn is_walkable(
        &mut self,
        seed: u64,
        edits: &TerrainEdits,
        cell: (i32, i32),
    ) -> bool {
        let size = CHUNK_CELLS as i32;
        let coord = ChunkCoord::new(cell.0.div_euclid(size), cell.1.div_euclid(size));
        let slot = match self.last {
            Some((c, slot)) if c == coord => slot,
            _ => {
                let slot = match self.index.get(&coord) {
                    Some(&slot) => slot,
                    None => self.cache_chunk(seed, edits, coord),
                };
                self.last = Some((coord, slot));
                slot
            }
        };
        let x = cell.0.rem_euclid(size) as usize;
        let z = cell.1.rem_euclid(size) as usize;
        let bit = z * CHUNK_CELLS + x;
        self.slots[slot].1[bit / 64] >> (bit % 64) & 1 != 0
    }

    /// Forget cached walkability for `coord` so it is recomputed from current terrain.
    pub(crate) fn invalidate(&mut self, coord: ChunkCoord) {
        if let Some(slot) = self.index.remove(&coord) {
            self.slots[slot].1.fill(0);
            // Park the slot under a coordinate no lookup can reach, until FIFO reuses it.
            self.slots[slot].0 = ChunkCoord::new(i32::MIN, i32::MIN);
        }
        if self.last.is_some_and(|(c, _)| c == coord) {
            self.last = None;
        }
    }

    fn cache_chunk(&mut self, seed: u64, edits: &TerrainEdits, coord: ChunkCoord) -> usize {
        let size = CHUNK_CELLS;
        let samples = size + 2;
        let (origin_x, origin_z) = coord.origin();
        let mut heights = vec![0.0; samples * samples];
        for z in 0..samples {
            for x in 0..samples {
                heights[z * samples + x] =
                    edits.height(seed, origin_x + x as f32 - 0.5, origin_z + z as f32 - 0.5);
            }
        }

        let mut bits = Box::new([0u64; BITSET_WORDS]);
        for z in 0..size {
            for x in 0..size {
                let center = heights[(z + 1) * samples + x + 1];
                let walkable = [
                    heights[(z + 1) * samples + x + 2],
                    heights[(z + 1) * samples + x],
                    heights[(z + 2) * samples + x + 1],
                    heights[z * samples + x + 1],
                ]
                .iter()
                .all(|h| (*h - center).abs() <= MAX_STEP_HEIGHT);
                if walkable {
                    let bit = z * size + x;
                    bits[bit / 64] |= 1 << (bit % 64);
                }
            }
        }

        let slot = if self.slots.len() < WALKABILITY_CACHE_CHUNKS {
            self.slots.push((coord, bits));
            self.slots.len() - 1
        } else {
            let slot = self.next_evict;
            self.next_evict = (slot + 1) % WALKABILITY_CACHE_CHUNKS;
            let old = std::mem::replace(&mut self.slots[slot], (coord, bits)).0;
            self.index.remove(&old);
            slot
        };
        self.index.insert(coord, slot);
        self.last = Some((coord, slot));
        slot
    }
}

/// Heightmap for one chunk.
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkTerrain {
    pub coord: ChunkCoord,
    /// Row-major `CELLS * CELLS` heights; index = `z * CELLS + x`.
    pub heights: Vec<f32>,
}

impl ChunkTerrain {
    pub fn generate(seed: u64, coord: ChunkCoord) -> Self {
        Self::generate_with(seed, &TerrainEdits::default(), coord)
    }

    /// Sample the chunk's heights from procedural terrain plus `edits`.
    pub fn generate_with(seed: u64, edits: &TerrainEdits, coord: ChunkCoord) -> Self {
        let (ox, oz) = coord.origin();
        let mut heights = Vec::with_capacity(CELLS * CELLS);
        for z in 0..CELLS {
            for x in 0..CELLS {
                heights.push(edits.height(seed, ox + x as f32 + 0.5, oz + z as f32 + 0.5));
            }
        }
        Self { coord, heights }
    }

    pub fn height(&self, x: usize, z: usize) -> f32 {
        self.heights[z * CELLS + x]
    }
}

/// Keeps the set of terrain chunks within a radius of a center chunk loaded.
#[derive(Debug, Clone)]
pub struct ChunkStreamer {
    seed: u64,
    pub radius: i32,
    loaded: HashMap<ChunkCoord, ChunkTerrain>,
    revisions: HashMap<ChunkCoord, u64>,
}

impl ChunkStreamer {
    pub fn new(seed: u64, radius: i32) -> Self {
        Self {
            seed,
            radius: radius.max(0),
            loaded: HashMap::new(),
            revisions: HashMap::new(),
        }
    }

    /// Load missing chunks and unload distant ones. Returns `(loaded, unloaded)` counts.
    pub fn update(&mut self, center: ChunkCoord) -> (usize, usize) {
        self.update_with(center, &TerrainEdits::default())
    }

    /// Like [`Self::update`], but also regenerates loaded chunks whose edit revision
    /// changed. Regenerated chunks count as loaded.
    pub fn update_with(&mut self, center: ChunkCoord, edits: &TerrainEdits) -> (usize, usize) {
        let r = self.radius;
        let before = self.loaded.len();
        self.loaded
            .retain(|c, _| (c.x - center.x).abs() <= r && (c.z - center.z).abs() <= r);
        let unloaded = before - self.loaded.len();
        self.revisions.retain(|c, _| self.loaded.contains_key(c));
        let mut loaded = 0;
        for z in center.z - r..=center.z + r {
            for x in center.x - r..=center.x + r {
                let c = ChunkCoord::new(x, z);
                let revision = edits.revision(c);
                if self.loaded.contains_key(&c) && self.revisions.get(&c) == Some(&revision) {
                    continue;
                }
                self.loaded
                    .insert(c, ChunkTerrain::generate_with(self.seed, edits, c));
                self.revisions.insert(c, revision);
                loaded += 1;
            }
        }
        (loaded, unloaded)
    }

    pub fn get(&self, c: ChunkCoord) -> Option<&ChunkTerrain> {
        self.loaded.get(&c)
    }

    pub fn len(&self) -> usize {
        self.loaded.len()
    }

    pub fn is_empty(&self) -> bool {
        self.loaded.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walkability_is_deterministic_and_mixed() {
        let cells: Vec<bool> = (0..50).map(|i| is_walkable(3, (i * 7, i * 5))).collect();
        assert_eq!(
            cells,
            (0..50)
                .map(|i| is_walkable(3, (i * 7, i * 5)))
                .collect::<Vec<_>>()
        );
        assert!(cells.iter().any(|&w| w));
    }

    #[test]
    fn cached_walkability_matches_direct_queries_across_chunk_boundaries() {
        let mut cache = WalkabilityCache::default();
        for z in -66..=66 {
            for x in -66..=66 {
                assert_eq!(
                    cache.is_walkable(29, &TerrainEdits::default(), (x, z)),
                    is_walkable(29, (x, z)),
                    "cell ({x}, {z})"
                );
            }
        }
        assert!(cache.index.len() <= WALKABILITY_CACHE_CHUNKS);
    }

    #[test]
    fn terrain_is_deterministic_and_bounded() {
        let c = ChunkCoord::new(-1, 2);
        let a = ChunkTerrain::generate(5, c);
        assert_eq!(a, ChunkTerrain::generate(5, c));
        assert_ne!(a, ChunkTerrain::generate(6, c));
        assert!(a.heights.iter().all(|h| (0.0..40.0).contains(h)));
    }

    #[test]
    fn richness_and_fertility_are_seeded_continuous_fields() {
        let richness = resource_richness_at(41, "iron_ore", 18.0, 27.0);
        let fertility = fertility_at(41, 18.0, 27.0);
        assert_eq!(richness, resource_richness_at(41, "iron_ore", 18.0, 27.0));
        assert_eq!(fertility, fertility_at(41, 18.0, 27.0));
        assert!((0.0..1.0).contains(&richness));
        assert!((0.0..1.0).contains(&fertility));
        assert_ne!(richness, resource_richness_at(41, "iron_ore", 180.0, -90.0));
        assert_ne!(fertility, fertility_at(41, 180.0, -90.0));
        assert_ne!(richness, resource_richness_at(42, "iron_ore", 18.0, 27.0));
    }

    #[test]
    fn terrain_is_continuous_across_chunk_borders() {
        let a = ChunkTerrain::generate(9, ChunkCoord::new(0, 0));
        let b = ChunkTerrain::generate(9, ChunkCoord::new(1, 0));
        assert!((a.height(CELLS - 1, 3) - b.height(0, 3)).abs() < 2.0);
    }

    #[test]
    fn streamer_loads_and_unloads() {
        let mut s = ChunkStreamer::new(1, 1);
        assert_eq!(s.update(ChunkCoord::new(0, 0)), (9, 0));
        assert_eq!(s.update(ChunkCoord::new(0, 0)), (0, 0));
        assert_eq!(s.update(ChunkCoord::new(1, 0)), (3, 3));
        assert_eq!(s.len(), 9);
        assert!(s.get(ChunkCoord::new(-1, 0)).is_none());
        assert!(s.get(ChunkCoord::new(2, 1)).is_some());
    }

    fn sample_edits() -> Vec<TerrainEdit> {
        vec![
            TerrainEdit::Raise {
                center: (30, 30),
                radius: 8,
                amount_mm: 4000,
            },
            TerrainEdit::Flatten {
                center: (62, 10),
                radius: 6,
                target_mm: 12_000,
            },
            TerrainEdit::Smooth {
                center: (31, 29),
                radius: 5,
            },
            TerrainEdit::Slope {
                from: (0, 0),
                to: (20, 10),
                half_width: 3,
            },
            TerrainEdit::Lower {
                center: (-5, -5),
                radius: 4,
                amount_mm: 1500,
            },
        ]
    }

    #[test]
    fn edits_are_deterministic_and_change_height() {
        let (mut a, mut b) = (TerrainEdits::default(), TerrainEdits::default());
        for e in sample_edits() {
            a.apply(5, e);
            b.apply(5, e);
        }
        assert_eq!(a, b);
        assert_eq!(
            a.entries().collect::<Vec<_>>(),
            b.entries().collect::<Vec<_>>()
        );
        let raised = TerrainEdits::default().height(5, 31.0, 31.0);
        let mut one = TerrainEdits::default();
        one.apply(
            5,
            TerrainEdit::Raise {
                center: (31, 31),
                radius: 4,
                amount_mm: 2000,
            },
        );
        assert!((one.height(5, 31.0, 31.0) - raised - 2.0).abs() < 2e-3);
        assert_eq!(one.height(5, 100.0, 100.0), height_at(5, 100.0, 100.0));
    }

    #[test]
    fn flatten_and_slope_hit_targets() {
        let mut e = TerrainEdits::default();
        e.apply(
            9,
            TerrainEdit::Flatten {
                center: (40, 40),
                radius: 5,
                target_mm: 20_000,
            },
        );
        assert!((e.height(9, 40.0, 40.0) - 20.0).abs() < 2e-3);
        let mut s = TerrainEdits::default();
        let (h0, h1) = (height_at(9, 0.0, 0.0), height_at(9, 10.0, 0.0));
        s.apply(
            9,
            TerrainEdit::Slope {
                from: (0, 0),
                to: (10, 0),
                half_width: 1,
            },
        );
        assert!((s.height(9, 0.0, 0.0) - h0).abs() < 2e-3);
        assert!((s.height(9, 10.0, 0.0) - h1).abs() < 2e-3);
        assert!((s.height(9, 5.0, 0.0) - (h0 + h1) / 2.0).abs() < 2e-3);
    }

    #[test]
    fn edited_surface_is_continuous_across_chunk_borders() {
        let mut e = TerrainEdits::default();
        let border = CHUNK_SIZE as i32;
        e.apply(
            3,
            TerrainEdit::Raise {
                center: (border, 4),
                radius: 6,
                amount_mm: 5000,
            },
        );
        let a = ChunkTerrain::generate_with(3, &e, ChunkCoord::new(0, 0));
        let b = ChunkTerrain::generate_with(3, &e, ChunkCoord::new(1, 0));
        let step =
            (e.height(3, border as f32 - 0.5, 4.5) - e.height(3, border as f32 + 0.5, 4.5)).abs();
        assert!(step < 5.0);
        assert!(
            a.height(CELLS - 1, 4)
                != ChunkTerrain::generate(3, ChunkCoord::new(0, 0)).height(CELLS - 1, 4)
        );
        assert!(b.height(0, 4) != ChunkTerrain::generate(3, ChunkCoord::new(1, 0)).height(0, 4));
    }

    #[test]
    fn edits_bump_revisions_of_neighbouring_chunks_and_invalidate_walkability() {
        let mut e = TerrainEdits::default();
        let border = CHUNK_SIZE as i32;
        let touched = e.apply(
            3,
            TerrainEdit::Raise {
                center: (border, 4),
                radius: 2,
                amount_mm: 3000,
            },
        );
        assert!(
            touched.contains(&ChunkCoord::new(0, 0)) && touched.contains(&ChunkCoord::new(1, 0))
        );
        assert_eq!(e.revision(ChunkCoord::new(5, 5)), 0);
        assert!(e.revision(ChunkCoord::new(1, 0)) > 0);

        let mut cache = WalkabilityCache::default();
        let cells: Vec<(i32, i32)> = (0..8).map(|z| (border, z)).collect();
        let before: Vec<bool> = cells
            .iter()
            .map(|&c| cache.is_walkable(3, &TerrainEdits::default(), c))
            .collect();
        for c in touched {
            cache.invalidate(c);
        }
        let after: Vec<bool> = cells.iter().map(|&c| cache.is_walkable(3, &e, c)).collect();
        let expected: Vec<bool> = cells.iter().map(|&c| is_walkable_with(3, &e, c)).collect();
        assert_eq!(after, expected);
        assert_ne!(before, after);
    }

    #[test]
    fn saved_entries_round_trip() {
        let mut e = TerrainEdits::default();
        for edit in sample_edits() {
            e.apply(5, edit);
        }
        let restored = TerrainEdits::from_entries(e.entries().collect::<Vec<_>>());
        assert_eq!(
            restored.entries().collect::<Vec<_>>(),
            e.entries().collect::<Vec<_>>()
        );
        assert_eq!(restored.height(5, 31.3, 29.7), e.height(5, 31.3, 29.7));
    }
}
