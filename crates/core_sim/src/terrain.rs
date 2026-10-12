use crate::chunk::{ChunkCoord, CHUNK_SIZE};
use std::collections::{HashMap, VecDeque};

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

/// Maximum height difference (meters) between adjacent cells that can still be walked.
pub const MAX_STEP_HEIGHT: f32 = 0.2;

/// Whether the 1m cell is walkable: no neighbouring cell is too steep a step away.
pub fn is_walkable(seed: u64, cell: (i32, i32)) -> bool {
    let h = |c: (i32, i32)| height_at(seed, c.0 as f32 + 0.5, c.1 as f32 + 0.5);
    let here = h(cell);
    [(1, 0), (-1, 0), (0, 1), (0, -1)]
        .iter()
        .all(|&(dx, dz)| (h((cell.0 + dx, cell.1 + dz)) - here).abs() <= MAX_STEP_HEIGHT)
}

/// Bounded cache of walkability for cells visited by pathfinding.
#[derive(Debug, Default)]
pub(crate) struct WalkabilityCache {
    chunks: HashMap<ChunkCoord, Vec<bool>>,
    insertion_order: VecDeque<ChunkCoord>,
}

impl WalkabilityCache {
    pub(crate) fn is_walkable(&mut self, seed: u64, cell: (i32, i32)) -> bool {
        let size = CHUNK_SIZE as i32;
        let coord = ChunkCoord::new(cell.0.div_euclid(size), cell.1.div_euclid(size));
        if !self.chunks.contains_key(&coord) {
            self.cache_chunk(seed, coord);
        }
        let x = cell.0.rem_euclid(size) as usize;
        let z = cell.1.rem_euclid(size) as usize;
        self.chunks[&coord][z * size as usize + x]
    }

    fn cache_chunk(&mut self, seed: u64, coord: ChunkCoord) {
        let size = CHUNK_SIZE as usize;
        let samples = size + 2;
        let (origin_x, origin_z) = coord.origin();
        let mut heights = vec![0.0; samples * samples];
        for z in 0..samples {
            for x in 0..samples {
                heights[z * samples + x] =
                    height_at(seed, origin_x + x as f32 - 0.5, origin_z + z as f32 - 0.5);
            }
        }

        let mut walkable = vec![false; size * size];
        for z in 0..size {
            for x in 0..size {
                let center = heights[(z + 1) * samples + x + 1];
                walkable[z * size + x] = [
                    heights[(z + 1) * samples + x + 2],
                    heights[(z + 1) * samples + x],
                    heights[(z + 2) * samples + x + 1],
                    heights[z * samples + x + 1],
                ]
                .iter()
                .all(|h| (*h - center).abs() <= MAX_STEP_HEIGHT);
            }
        }

        if self.chunks.len() == WALKABILITY_CACHE_CHUNKS {
            if let Some(oldest) = self.insertion_order.pop_front() {
                self.chunks.remove(&oldest);
            }
        }
        self.chunks.insert(coord, walkable);
        self.insertion_order.push_back(coord);
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
        let (ox, oz) = coord.origin();
        let mut heights = Vec::with_capacity(CELLS * CELLS);
        for z in 0..CELLS {
            for x in 0..CELLS {
                heights.push(height_at(seed, ox + x as f32 + 0.5, oz + z as f32 + 0.5));
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
}

impl ChunkStreamer {
    pub fn new(seed: u64, radius: i32) -> Self {
        Self {
            seed,
            radius: radius.max(0),
            loaded: HashMap::new(),
        }
    }

    /// Load missing chunks and unload distant ones. Returns `(loaded, unloaded)` counts.
    pub fn update(&mut self, center: ChunkCoord) -> (usize, usize) {
        let r = self.radius;
        let before = self.loaded.len();
        self.loaded
            .retain(|c, _| (c.x - center.x).abs() <= r && (c.z - center.z).abs() <= r);
        let unloaded = before - self.loaded.len();
        let mut loaded = 0;
        for z in center.z - r..=center.z + r {
            for x in center.x - r..=center.x + r {
                let c = ChunkCoord::new(x, z);
                if let std::collections::hash_map::Entry::Vacant(e) = self.loaded.entry(c) {
                    e.insert(ChunkTerrain::generate(self.seed, c));
                    loaded += 1;
                }
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
                    cache.is_walkable(29, (x, z)),
                    is_walkable(29, (x, z)),
                    "cell ({x}, {z})"
                );
            }
        }
        assert!(cache.chunks.len() <= WALKABILITY_CACHE_CHUNKS);
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
}
