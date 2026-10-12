use bytemuck::{Pod, Zeroable};
use core_sim::{height_at, ChunkCoord, Footprint, CHUNK_SIZE};

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

#[derive(Default)]
struct MeshBuilder {
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
}

impl MeshBuilder {
    fn quad(&mut self, points: [[f32; 3]; 4], outward: [f32; 3]) {
        let base = self.vertices.len() as u32;
        let mut points = points;
        let normal = cross(sub(points[1], points[0]), sub(points[2], points[0]));
        if dot(normal, outward) < 0.0 {
            points.swap(1, 3);
        }
        let normal = normalize(cross(sub(points[1], points[0]), sub(points[2], points[0])));
        self.vertices
            .extend(points.map(|pos| Vertex { pos, normal }));
        self.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    fn triangle(&mut self, mut points: [[f32; 3]; 3], outward: [f32; 3]) {
        let base = self.vertices.len() as u32;
        if dot(
            cross(sub(points[1], points[0]), sub(points[2], points[0])),
            outward,
        ) < 0.0
        {
            points.swap(1, 2);
        }
        let normal = normalize(cross(sub(points[1], points[0]), sub(points[2], points[0])));
        self.vertices
            .extend(points.map(|pos| Vertex { pos, normal }));
        self.indices.extend_from_slice(&[base, base + 1, base + 2]);
    }

    fn box_mesh(&mut self, x: (f32, f32), y: (f32, f32), z: (f32, f32)) {
        let (x0, x1) = x;
        let (y0, y1) = y;
        let (z0, z1) = z;
        self.quad(
            [[x1, y0, z0], [x1, y1, z0], [x1, y1, z1], [x1, y0, z1]],
            [1.0, 0.0, 0.0],
        );
        self.quad(
            [[x0, y0, z1], [x0, y1, z1], [x0, y1, z0], [x0, y0, z0]],
            [-1.0, 0.0, 0.0],
        );
        self.quad(
            [[x0, y1, z0], [x0, y1, z1], [x1, y1, z1], [x1, y1, z0]],
            [0.0, 1.0, 0.0],
        );
        self.quad(
            [[x0, y0, z1], [x0, y0, z0], [x1, y0, z0], [x1, y0, z1]],
            [0.0, -1.0, 0.0],
        );
        self.quad(
            [[x1, y0, z1], [x1, y1, z1], [x0, y1, z1], [x0, y0, z1]],
            [0.0, 0.0, 1.0],
        );
        self.quad(
            [[x0, y0, z0], [x0, y1, z0], [x1, y1, z0], [x1, y0, z0]],
            [0.0, 0.0, -1.0],
        );
    }

    fn gable_roof(&mut self, x: (f32, f32), y: (f32, f32), z: (f32, f32)) {
        let (x0, x1) = x;
        let (y0, y1) = y;
        let (z0, z1) = z;
        let xm = (x0 + x1) * 0.5;
        self.triangle([[x0, y0, z0], [x1, y0, z0], [xm, y1, z0]], [0.0, 0.0, -1.0]);
        self.triangle([[x1, y0, z1], [x0, y0, z1], [xm, y1, z1]], [0.0, 0.0, 1.0]);
        self.quad(
            [[x0, y0, z0], [x0, y0, z1], [xm, y1, z1], [xm, y1, z0]],
            [-1.0, 1.0, 0.0],
        );
        self.quad(
            [[x1, y0, z1], [x1, y0, z0], [xm, y1, z0], [xm, y1, z1]],
            [1.0, 1.0, 0.0],
        );
    }

    fn finish(self) -> (Vec<Vertex>, Vec<u32>) {
        (self.vertices, self.indices)
    }
}

/// Faceted, articulated humanoid placeholder, 1.8 m tall and facing toward -Z.
pub fn humanoid_mesh() -> (Vec<Vertex>, Vec<u32>) {
    let mut mesh = MeshBuilder::default();
    mesh.box_mesh((-0.16, -0.025), (0.0, 0.82), (-0.10, 0.12));
    mesh.box_mesh((0.025, 0.16), (0.0, 0.82), (-0.10, 0.12));
    mesh.box_mesh((-0.22, 0.22), (0.78, 1.35), (-0.13, 0.13));
    mesh.box_mesh((-0.32, 0.32), (1.25, 1.48), (-0.13, 0.10));
    mesh.box_mesh((-0.22, 0.22), (1.34, 1.60), (-0.12, 0.10));
    mesh.box_mesh((-0.13, 0.13), (1.60, 1.68), (-0.10, 0.10));
    mesh.box_mesh((-0.36, -0.22), (0.73, 1.28), (-0.10, 0.13));
    mesh.box_mesh((0.22, 0.36), (0.73, 1.28), (-0.10, 0.13));
    mesh.box_mesh((-0.085, 0.085), (1.67, 1.79), (-0.075, 0.075));
    let top = [0.0, 1.82, 0.0];
    let bottom = [0.0, 1.63, 0.0];
    let ring = [
        [0.13, 1.72, 0.0],
        [0.0, 1.72, 0.105],
        [-0.13, 1.72, 0.0],
        [0.0, 1.72, -0.105],
    ];
    for index in 0..4 {
        let next = (index + 1) % 4;
        let face_center = [
            (top[0] + ring[index][0] + ring[next][0]) / 3.0,
            (top[1] + ring[index][1] + ring[next][1]) / 3.0 - 1.72,
            (top[2] + ring[index][2] + ring[next][2]) / 3.0,
        ];
        mesh.triangle([top, ring[index], ring[next]], face_center);
        let face_center = [
            (bottom[0] + ring[next][0] + ring[index][0]) / 3.0,
            (bottom[1] + ring[next][1] + ring[index][1]) / 3.0 - 1.72,
            (bottom[2] + ring[next][2] + ring[index][2]) / 3.0,
        ];
        mesh.triangle([bottom, ring[next], ring[index]], face_center);
    }
    mesh.finish()
}

/// Low-poly building silhouette positioned at its world footprint.
pub fn building_mesh(footprint: Footprint, definition: &str) -> (Vec<Vertex>, Vec<u32>) {
    let width = footprint.width as f32;
    let depth = footprint.depth as f32;
    let origin_x = 0.0;
    let origin_z = 0.0;
    let base_y = 0.0;
    let mut mesh = MeshBuilder::default();

    match definition {
        "farm" => {
            mesh.box_mesh(
                (origin_x + 0.10, origin_x + width - 0.10),
                (base_y, base_y + 0.08),
                (origin_z + 0.10, origin_z + depth - 0.10),
            );
            let row_count = 5;
            for row in 0..row_count {
                let z = origin_z + 0.65 + row as f32 * ((depth - 1.3) / row_count as f32);
                mesh.box_mesh(
                    (origin_x + 0.35, origin_x + width - 0.35),
                    (base_y + 0.08, base_y + 0.22),
                    (z, z + 0.12),
                );
            }
            let shed_x = origin_x + width * 0.5 - 0.8;
            let shed_z = origin_z + depth * 0.5 - 0.7;
            add_shed(&mut mesh, shed_x, base_y + 0.08, shed_z, 1.6, 1.4, 1.7);
        }
        "iron_mine" => {
            let cx = origin_x + width * 0.5;
            let entrance_z = origin_z + depth - 0.75;
            mesh.box_mesh(
                (cx - 1.0, cx - 0.72),
                (base_y, base_y + 1.45),
                (entrance_z, entrance_z + 0.25),
            );
            mesh.box_mesh(
                (cx + 0.72, cx + 1.0),
                (base_y, base_y + 1.45),
                (entrance_z, entrance_z + 0.25),
            );
            mesh.box_mesh(
                (cx - 1.0, cx + 1.0),
                (base_y + 1.35, base_y + 1.65),
                (entrance_z, entrance_z + 0.25),
            );
            mesh.box_mesh(
                (cx - 0.72, cx + 0.72),
                (base_y + 0.05, base_y + 1.30),
                (entrance_z + 0.05, entrance_z + 0.12),
            );
            for brace in 0..3 {
                let x = cx - 0.8 + brace as f32 * 0.8;
                mesh.box_mesh(
                    (x, x + 0.12),
                    (base_y + 0.1, base_y + 1.4),
                    (entrance_z - 0.7, entrance_z - 0.52),
                );
            }
        }
        "house" => add_shed(&mut mesh, origin_x, base_y, origin_z, width, depth, 2.5),
        "workshop" => {
            add_shed(&mut mesh, origin_x, base_y, origin_z, width, depth, 3.0);
            mesh.box_mesh(
                (origin_x + width - 0.9, origin_x + width - 0.4),
                (base_y + 3.0, base_y + 4.1),
                (origin_z + 0.5, origin_z + 1.0),
            );
        }
        "wheat_storage" | "ore_storage" => {
            add_shed(&mut mesh, origin_x, base_y, origin_z, width, depth, 2.0);
            let goods_y = base_y + 0.08;
            let goods_color_shape = if definition == "wheat_storage" {
                0.35
            } else {
                0.55
            };
            for row in 0..2 {
                for column in 0..2 {
                    let x = origin_x + 0.55 + column as f32 * (width * 0.38);
                    let z = origin_z + 0.55 + row as f32 * (depth * 0.38);
                    mesh.box_mesh(
                        (x, x + 0.65),
                        (goods_y, goods_y + goods_color_shape),
                        (z, z + 0.65),
                    );
                }
            }
        }
        _ => add_shed(&mut mesh, origin_x, base_y, origin_z, width, depth, 2.5),
    }
    mesh.finish()
}

fn add_shed(
    mesh: &mut MeshBuilder,
    x: f32,
    base_y: f32,
    z: f32,
    width: f32,
    depth: f32,
    wall_height: f32,
) {
    mesh.box_mesh(
        (x + 0.10, x + width - 0.10),
        (base_y, base_y + wall_height),
        (z + 0.10, z + depth - 0.10),
    );
    mesh.gable_roof(
        (x, x + width),
        (base_y + wall_height, base_y + wall_height + 0.8),
        (z, z + depth),
    );
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalize(vector: [f32; 3]) -> [f32; 3] {
    let length = dot(vector, vector).sqrt().max(f32::EPSILON);
    [vector[0] / length, vector[1] / length, vector[2] / length]
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
    fn humanoid_faces_outward_and_has_expected_height() {
        let (v, i) = humanoid_mesh();
        assert!(i.iter().all(|&x| (x as usize) < v.len()));
        for t in i.chunks(3) {
            let n = tri_normal(&v, t);
            let vn = v[t[0] as usize].normal;
            let dot = n[0] * vn[0] + n[1] * vn[1] + n[2] * vn[2];
            assert!(dot >= -1e-6, "capsule triangle wound inward");
        }
        let top = v.iter().map(|x| x.pos[1]).fold(f32::MIN, f32::max);
        let bottom = v.iter().map(|x| x.pos[1]).fold(f32::MAX, f32::min);
        assert!((top - 1.82).abs() < 1e-4 && bottom.abs() < 1e-4);
    }

    #[test]
    fn procedural_building_meshes_are_nonempty_and_local_to_footprint() {
        for definition in ["house", "farm", "iron_mine", "workshop", "wheat_storage"] {
            let footprint = Footprint {
                x: -20,
                z: 75,
                width: 6,
                depth: 4,
            };
            let (vertices, indices) = building_mesh(footprint, definition);
            assert!(!vertices.is_empty(), "{definition} should have geometry");
            assert!(!indices.is_empty(), "{definition} should have triangles");
            assert!(indices
                .iter()
                .all(|&index| (index as usize) < vertices.len()));
            assert!(vertices.iter().all(|vertex| {
                vertex.pos[0] >= -0.1
                    && vertex.pos[0] <= footprint.width as f32 + 0.1
                    && vertex.pos[2] >= -0.1
                    && vertex.pos[2] <= footprint.depth as f32 + 0.1
            }));
        }
    }
}
