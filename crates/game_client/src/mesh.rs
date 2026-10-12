use bytemuck::{Pod, Zeroable};
use core_sim::{height_at, ChunkCoord, CHUNK_SIZE};

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
}

/// Meters between terrain mesh vertices.
pub const TERRAIN_STEP: f32 = 2.0;
/// Quads per chunk edge.
pub const TERRAIN_QUADS: usize = (CHUNK_SIZE / TERRAIN_STEP) as usize;

/// Heightmap mesh for one chunk. Heights and normals come straight from the continuous
/// height function, so neighbouring chunks line up with no seams.
pub fn terrain_chunk_mesh(seed: u64, coord: ChunkCoord) -> (Vec<Vertex>, Vec<u32>) {
    let (ox, oz) = coord.origin();
    let n = TERRAIN_QUADS + 1;
    let mut vertices = Vec::with_capacity(n * n);
    for iz in 0..n {
        for ix in 0..n {
            let x = ox + ix as f32 * TERRAIN_STEP;
            let z = oz + iz as f32 * TERRAIN_STEP;
            let e = 1.0;
            let dx = (height_at(seed, x + e, z) - height_at(seed, x - e, z)) / (2.0 * e);
            let dz = (height_at(seed, x, z + e) - height_at(seed, x, z - e)) / (2.0 * e);
            let len = (dx * dx + 1.0 + dz * dz).sqrt();
            vertices.push(Vertex {
                pos: [x, height_at(seed, x, z), z],
                normal: [-dx / len, 1.0 / len, -dz / len],
            });
        }
    }
    let mut indices = Vec::with_capacity(TERRAIN_QUADS * TERRAIN_QUADS * 6);
    let at = |ix: usize, iz: usize| (iz * n + ix) as u32;
    for iz in 0..TERRAIN_QUADS {
        for ix in 0..TERRAIN_QUADS {
            let (a, b, c, d) = (
                at(ix, iz),
                at(ix, iz + 1),
                at(ix + 1, iz),
                at(ix + 1, iz + 1),
            );
            indices.extend_from_slice(&[a, b, c, c, b, d]);
        }
    }
    (vertices, indices)
}

/// Low-poly capsule (a placeholder unit body), 1.8 m tall, standing on y = 0.
pub fn capsule_mesh() -> (Vec<Vertex>, Vec<u32>) {
    const SEGMENTS: usize = 10;
    const HEIGHT: f32 = 1.8;
    const RADIUS: f32 = 0.35;
    let phis_lower = [-90.0f32, -60.0, -30.0, 0.0];
    let phis_upper = [0.0f32, 30.0, 60.0, 90.0];
    let mut rings: Vec<(f32, f32)> = Vec::new();
    rings.extend(phis_lower.iter().map(|p| (p.to_radians(), RADIUS)));
    rings.extend(phis_upper.iter().map(|p| (p.to_radians(), HEIGHT - RADIUS)));
    let mut vertices = Vec::new();
    for &(phi, center_y) in &rings {
        for s in 0..=SEGMENTS {
            let a = s as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
            let (sa, ca) = a.sin_cos();
            let n = [phi.cos() * ca, phi.sin(), phi.cos() * sa];
            vertices.push(Vertex {
                pos: [RADIUS * n[0], center_y + RADIUS * n[1], RADIUS * n[2]],
                normal: n,
            });
        }
    }
    let row = SEGMENTS + 1;
    let mut indices = Vec::new();
    for r in 0..rings.len() - 1 {
        for s in 0..SEGMENTS {
            let p00 = (r * row + s) as u32;
            let p01 = p00 + 1;
            let p10 = ((r + 1) * row + s) as u32;
            let p11 = p10 + 1;
            indices.extend_from_slice(&[p00, p10, p01, p01, p10, p11]);
        }
    }
    (vertices, indices)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tri_normal(v: &[Vertex], i: &[u32]) -> [f32; 3] {
        let p = |k: usize| v[i[k] as usize].pos;
        let (a, b, c) = (p(0), p(1), p(2));
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let w = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        [
            u[1] * w[2] - u[2] * w[1],
            u[2] * w[0] - u[0] * w[2],
            u[0] * w[1] - u[1] * w[0],
        ]
    }

    #[test]
    fn terrain_mesh_counts_and_indices_in_range() {
        let (v, i) = terrain_chunk_mesh(1, ChunkCoord::new(0, 0));
        assert_eq!(v.len(), (TERRAIN_QUADS + 1) * (TERRAIN_QUADS + 1));
        assert_eq!(i.len(), TERRAIN_QUADS * TERRAIN_QUADS * 6);
        assert!(i.iter().all(|&x| (x as usize) < v.len()));
    }

    #[test]
    fn terrain_faces_up_and_normals_are_unit() {
        let (v, i) = terrain_chunk_mesh(7, ChunkCoord::new(-1, 2));
        for t in i.chunks(3) {
            assert!(
                tri_normal(&v, t)[1] > 0.0,
                "triangle must face up (CCW from above)"
            );
        }
        for vert in &v {
            let n = vert.normal;
            assert!(((n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt() - 1.0).abs() < 1e-4);
            assert!(n[1] > 0.0);
        }
    }

    #[test]
    fn neighbouring_chunks_share_border_vertices() {
        let (a, _) = terrain_chunk_mesh(3, ChunkCoord::new(0, 0));
        let (b, _) = terrain_chunk_mesh(3, ChunkCoord::new(1, 0));
        let n = TERRAIN_QUADS + 1;
        for iz in 0..n {
            let right_edge = a[iz * n + TERRAIN_QUADS];
            let left_edge = b[iz * n];
            assert_eq!(right_edge.pos, left_edge.pos);
            assert_eq!(right_edge.normal, left_edge.normal);
        }
    }

    #[test]
    fn capsule_faces_outward() {
        let (v, i) = capsule_mesh();
        assert!(i.iter().all(|&x| (x as usize) < v.len()));
        for t in i.chunks(3) {
            let n = tri_normal(&v, t);
            let vn = v[t[0] as usize].normal;
            let dot = n[0] * vn[0] + n[1] * vn[1] + n[2] * vn[2];
            assert!(dot >= -1e-6, "capsule triangle wound inward");
        }
        let top = v.iter().map(|x| x.pos[1]).fold(f32::MIN, f32::max);
        let bottom = v.iter().map(|x| x.pos[1]).fold(f32::MAX, f32::min);
        assert!((top - 1.8).abs() < 1e-4 && bottom.abs() < 1e-4);
    }
}
