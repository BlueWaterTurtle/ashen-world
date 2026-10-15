use serde::{Deserialize, Serialize};

/// Edge length of a chunk in world units (meters).
pub const CHUNK_SIZE: f32 = 64.0;

/// Integer index of a chunk on the XZ (ground) plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ChunkCoord {
    pub x: i32,
    pub z: i32,
}

impl ChunkCoord {
    pub const fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }

    /// World-space position of this chunk's minimum corner.
    pub fn origin(self) -> (f32, f32) {
        (self.x as f32 * CHUNK_SIZE, self.z as f32 * CHUNK_SIZE)
    }
}

/// A world position in 3D (y is up) in world units.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct WorldPos {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl WorldPos {
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    /// Chunk containing this position.
    pub fn chunk(self) -> ChunkCoord {
        ChunkCoord::new(
            (self.x / CHUNK_SIZE).floor() as i32,
            (self.z / CHUNK_SIZE).floor() as i32,
        )
    }

    /// Position within its chunk, each axis in `[0, CHUNK_SIZE)` (y unchanged).
    pub fn local(self) -> WorldPos {
        let c = self.chunk();
        let (ox, oz) = c.origin();
        WorldPos::new(self.x - ox, self.y, self.z - oz)
    }

    /// Rebuild a world position from a chunk and a local position.
    pub fn from_chunk_local(chunk: ChunkCoord, local: WorldPos) -> Self {
        let (ox, oz) = chunk.origin();
        WorldPos::new(ox + local.x, local.y, oz + local.z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positive_and_negative_conversion() {
        assert_eq!(WorldPos::new(0.0, 0.0, 0.0).chunk(), ChunkCoord::new(0, 0));
        assert_eq!(
            WorldPos::new(63.9, 0.0, 64.0).chunk(),
            ChunkCoord::new(0, 1)
        );
        assert_eq!(
            WorldPos::new(-0.1, 0.0, -64.1).chunk(),
            ChunkCoord::new(-1, -2)
        );
    }

    #[test]
    fn local_is_within_chunk_and_roundtrips() {
        let p = WorldPos::new(-10.0, 3.0, 130.5);
        let l = p.local();
        assert!((0.0..CHUNK_SIZE).contains(&l.x));
        assert!((0.0..CHUNK_SIZE).contains(&l.z));
        assert_eq!(l.y, 3.0);
        assert_eq!(WorldPos::from_chunk_local(p.chunk(), l), p);
    }
}
