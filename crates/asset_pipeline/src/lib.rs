use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use bytemuck::{Pod, Zeroable};
use glam::{Mat3, Mat4, Quat, Vec3};
use serde::Deserialize;

static EXTERNAL_TEXTURE_CACHE: OnceLock<Mutex<HashMap<PathBuf, Arc<TextureData>>>> =
    OnceLock::new();

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

#[derive(Debug, Clone)]
pub struct TextureData {
    pub width: u32,
    pub height: u32,
    pub rgba8: Arc<[u8]>,
}

#[derive(Debug, Clone)]
pub struct Material {
    pub base_color: [f32; 4],
    pub base_color_texture: Option<Arc<TextureData>>,
}

#[derive(Debug, Clone, Copy)]
pub struct Surface {
    pub first_index: u32,
    pub index_count: u32,
    pub material_index: usize,
}

#[derive(Debug, Clone, Default)]
pub struct Model {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub materials: Vec<Material>,
    pub surfaces: Vec<Surface>,
}

impl Model {
    pub fn append_transformed(&mut self, mut other: Model, transform: Mat4) -> Result<(), String> {
        if !transform.is_finite() {
            return Err("prefab part transform must be finite".to_owned());
        }
        let linear = Mat3::from_mat4(transform);
        let determinant = linear.determinant();
        if !determinant.is_finite() || determinant.abs() <= f32::EPSILON {
            return Err("prefab part transform must not be singular".to_owned());
        }
        let normal_transform = linear.inverse().transpose();
        for vertex in &mut other.vertices {
            vertex.pos = transform
                .transform_point3(Vec3::from_array(vertex.pos))
                .to_array();
            vertex.normal = (normal_transform * Vec3::from_array(vertex.normal))
                .normalize_or_zero()
                .to_array();
        }
        if determinant < 0.0 {
            for triangle in other.indices.chunks_exact_mut(3) {
                triangle.swap(1, 2);
            }
        }

        let vertex_offset = self.vertices.len() as u32;
        let index_offset = self.indices.len() as u32;
        let material_remap = other
            .materials
            .iter()
            .map(|material| {
                self.materials
                    .iter()
                    .position(|existing| {
                        existing.base_color == material.base_color
                            && match (&existing.base_color_texture, &material.base_color_texture) {
                                (Some(existing), Some(incoming)) => Arc::ptr_eq(existing, incoming),
                                (None, None) => true,
                                _ => false,
                            }
                    })
                    .unwrap_or_else(|| {
                        self.materials.push(material.clone());
                        self.materials.len() - 1
                    })
            })
            .collect::<Vec<_>>();
        self.vertices.extend(other.vertices);
        self.indices
            .extend(other.indices.into_iter().map(|index| vertex_offset + index));
        for mut surface in other.surfaces {
            surface.first_index += index_offset;
            surface.material_index = material_remap[surface.material_index];
            if let Some(previous) = self.surfaces.last_mut().filter(|previous| {
                previous.material_index == surface.material_index
                    && previous.first_index + previous.index_count == surface.first_index
            }) {
                previous.index_count += surface.index_count;
            } else {
                self.surfaces.push(surface);
            }
        }
        Ok(())
    }

    pub fn fit_height(&mut self, height: f32) -> Result<(), String> {
        if !height.is_finite() || height <= 0.0 || self.vertices.is_empty() {
            return Err(
                "target height must be positive and model geometry must be non-empty".to_owned(),
            );
        }
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for vertex in &self.vertices {
            let position = Vec3::from_array(vertex.pos);
            min = min.min(position);
            max = max.max(position);
        }
        let source_height = max.y - min.y;
        if !source_height.is_finite() || source_height <= f32::EPSILON {
            return Err("model has no usable vertical extent".to_owned());
        }
        let scale = height / source_height;
        let center = Vec3::new((min.x + max.x) * 0.5, min.y, (min.z + max.z) * 0.5);
        for vertex in &mut self.vertices {
            vertex.pos = ((Vec3::from_array(vertex.pos) - center) * scale).to_array();
        }
        Ok(())
    }

    pub fn scale(&mut self, scale: [f32; 3]) -> Result<(), String> {
        if scale
            .iter()
            .any(|value| !value.is_finite() || *value == 0.0)
        {
            return Err("model scale must contain finite, non-zero values".to_owned());
        }
        for vertex in &mut self.vertices {
            for ((position, normal), axis_scale) in vertex
                .pos
                .iter_mut()
                .zip(vertex.normal.iter_mut())
                .zip(scale)
            {
                *position *= axis_scale;
                *normal /= axis_scale;
            }
            let length = vertex
                .normal
                .iter()
                .map(|component| component * component)
                .sum::<f32>()
                .sqrt();
            for component in &mut vertex.normal {
                *component /= length;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct PrefabDefinition {
    parts: Vec<PrefabPart>,
}

#[derive(Debug, Deserialize)]
struct PrefabPart {
    asset: String,
    #[serde(default)]
    translation: [f32; 3],
    #[serde(default)]
    rotation_y_degrees: f32,
    #[serde(default = "unit_scale")]
    scale: [f32; 3],
}

fn unit_scale() -> [f32; 3] {
    [1.0; 3]
}

pub fn load_prefab(
    asset_root: impl AsRef<Path>,
    definition_path: impl AsRef<Path>,
) -> Result<Model, String> {
    let asset_root = asset_root.as_ref();
    let definition_path = definition_path.as_ref();
    let definition_text = fs::read_to_string(definition_path)
        .map_err(|error| format!("{}: {error}", definition_path.display()))?;
    let definition: PrefabDefinition = serde_json::from_str(&definition_text)
        .map_err(|error| format!("{}: {error}", definition_path.display()))?;
    if definition.parts.is_empty() {
        return Err(format!(
            "{}: prefab contains no parts",
            definition_path.display()
        ));
    }

    let mut loaded_assets = HashMap::<PathBuf, Model>::new();
    let mut result = Model::default();
    for part in definition.parts {
        let relative_path = Path::new(&part.asset);
        if relative_path.is_absolute()
            || relative_path
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return Err(format!(
                "prefab asset path must stay under asset root: {}",
                part.asset
            ));
        }
        if !part.rotation_y_degrees.is_finite()
            || part.translation.iter().any(|value| !value.is_finite())
            || part
                .scale
                .iter()
                .any(|value| !value.is_finite() || *value == 0.0)
        {
            return Err(format!("prefab part has invalid transform: {}", part.asset));
        }
        let asset_path = asset_root.join(relative_path);
        let source = if let Some(model) = loaded_assets.get(&asset_path) {
            model.clone()
        } else {
            let model = load_gltf(&asset_path)?;
            loaded_assets.insert(asset_path.clone(), model.clone());
            model
        };
        let transform = Mat4::from_scale_rotation_translation(
            Vec3::from_array(part.scale),
            Quat::from_rotation_y(part.rotation_y_degrees.to_radians()),
            Vec3::from_array(part.translation),
        );
        result
            .append_transformed(source, transform)
            .map_err(|error| format!("{}: {error}", asset_path.display()))?;
    }
    Ok(result)
}

pub fn load_obj(path: impl AsRef<Path>) -> Result<Model, String> {
    let path = path.as_ref();
    let contents =
        fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    parse_obj(&contents).map_err(|error| format!("{}: {error}", path.display()))
}

pub fn load_gltf(path: impl AsRef<Path>) -> Result<Model, String> {
    let path = path.as_ref();
    let gltf = gltf::Gltf::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let document = gltf.document;
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let buffers = document
        .buffers()
        .map(|buffer| match buffer.source() {
            gltf::buffer::Source::Bin => gltf
                .blob
                .clone()
                .ok_or_else(|| "glTF references a missing embedded binary buffer".to_owned()),
            gltf::buffer::Source::Uri(uri) if uri.starts_with("data:") => {
                Err("data-URI glTF buffers are not yet supported".to_owned())
            }
            gltf::buffer::Source::Uri(uri) => fs::read(base_dir.join(uri))
                .map_err(|error| format!("could not read buffer {uri}: {error}")),
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let scene = document
        .default_scene()
        .or_else(|| document.scenes().next())
        .ok_or_else(|| format!("{}: glTF has no scenes", path.display()))?;
    let mut model = Model::default();
    let mut material_indices = HashMap::new();
    let mut texture_cache = HashMap::new();
    for node in scene.nodes() {
        append_gltf_node(
            node,
            Mat4::IDENTITY,
            &buffers,
            base_dir,
            &mut material_indices,
            &mut texture_cache,
            &mut model,
        )
        .map_err(|error| format!("{}: {error}", path.display()))?;
    }
    if model.indices.is_empty() {
        return Err(format!(
            "{}: glTF scene contains no triangle geometry",
            path.display()
        ));
    }
    Ok(model)
}

fn append_gltf_node(
    node: gltf::Node<'_>,
    parent_transform: Mat4,
    buffers: &[Vec<u8>],
    base_dir: &Path,
    material_indices: &mut HashMap<Option<usize>, usize>,
    texture_cache: &mut HashMap<usize, Arc<TextureData>>,
    model: &mut Model,
) -> Result<(), String> {
    let local_transform = Mat4::from_cols_array_2d(&node.transform().matrix());
    let transform = parent_transform * local_transform;
    if !transform.is_finite() {
        return Err(format!("node {} has a non-finite transform", node.index()));
    }

    if let Some(mesh) = node.mesh() {
        for primitive in mesh.primitives() {
            if primitive.mode() != gltf::mesh::Mode::Triangles {
                return Err(format!(
                    "mesh {} primitive uses unsupported {:?} topology",
                    mesh.index(),
                    primitive.mode()
                ));
            }
            let reader = primitive.reader(|buffer| buffers.get(buffer.index()).map(Vec::as_slice));
            let positions = reader
                .read_positions()
                .ok_or_else(|| format!("mesh {} has no positions", mesh.index()))?
                .collect::<Vec<_>>();
            if positions.is_empty() {
                continue;
            }
            let source_normals = reader
                .read_normals()
                .map(|normals| normals.collect::<Vec<_>>());
            let tex_coords = reader
                .read_tex_coords(0)
                .map(|coords| coords.into_f32().collect::<Vec<_>>())
                .unwrap_or_else(|| vec![[0.0; 2]; positions.len()]);
            if source_normals
                .as_ref()
                .is_some_and(|normals| normals.len() != positions.len())
                || tex_coords.len() != positions.len()
            {
                return Err(format!(
                    "mesh {} has mismatched vertex attribute counts",
                    mesh.index()
                ));
            }
            let mut indices = reader
                .read_indices()
                .map(|indices| indices.into_u32().collect::<Vec<_>>())
                .unwrap_or_else(|| (0..positions.len() as u32).collect());
            if indices.len() % 3 != 0
                || indices
                    .iter()
                    .any(|&index| index as usize >= positions.len())
            {
                return Err(format!(
                    "mesh {} has invalid triangle indices",
                    mesh.index()
                ));
            }

            let base = model.vertices.len() as u32;
            let first_index = model.indices.len() as u32;
            let linear = Mat3::from_mat4(transform);
            let normal_transform = linear.inverse().transpose();
            let transformed_positions = positions
                .iter()
                .map(|position| transform.transform_point3(Vec3::from_array(*position)))
                .collect::<Vec<_>>();
            let mut normals = source_normals.map(|normals| {
                normals
                    .iter()
                    .map(|normal| {
                        (normal_transform * Vec3::from_array(*normal))
                            .normalize_or_zero()
                            .to_array()
                    })
                    .collect::<Vec<_>>()
            });
            if linear.determinant() < 0.0 {
                for triangle in indices.chunks_exact_mut(3) {
                    triangle.swap(1, 2);
                }
            }
            if normals.is_none() {
                let mut generated = vec![Vec3::ZERO; positions.len()];
                for triangle in indices.chunks_exact(3) {
                    let a = transformed_positions[triangle[0] as usize];
                    let b = transformed_positions[triangle[1] as usize];
                    let c = transformed_positions[triangle[2] as usize];
                    let face = (b - a).cross(c - a).normalize_or_zero();
                    for &index in triangle {
                        generated[index as usize] += face;
                    }
                }
                normals = Some(
                    generated
                        .into_iter()
                        .map(|normal| normal.normalize_or_zero().to_array())
                        .collect(),
                );
            }
            let normals = normals.unwrap();
            if transformed_positions
                .iter()
                .any(|position| !position.is_finite())
                || normals
                    .iter()
                    .any(|normal| normal.iter().any(|component| !component.is_finite()))
            {
                return Err(format!("mesh {} has non-finite geometry", mesh.index()));
            }
            model.vertices.extend(
                transformed_positions
                    .into_iter()
                    .zip(normals)
                    .zip(tex_coords)
                    .map(|((position, normal), uv)| Vertex {
                        pos: position.to_array(),
                        normal,
                        uv,
                    }),
            );
            model
                .indices
                .extend(indices.into_iter().map(|index| base + index));
            let source_material = primitive.material();
            let material_index = if let Some(index) = material_indices.get(&source_material.index())
            {
                *index
            } else {
                let pbr = source_material.pbr_metallic_roughness();
                let texture = pbr
                    .base_color_texture()
                    .map(|info| load_base_color_texture(info, buffers, base_dir, texture_cache))
                    .transpose()?;
                let index = model.materials.len();
                model.materials.push(Material {
                    base_color: pbr.base_color_factor(),
                    base_color_texture: texture,
                });
                material_indices.insert(source_material.index(), index);
                index
            };
            model.surfaces.push(Surface {
                first_index,
                index_count: model.indices.len() as u32 - first_index,
                material_index,
            });
        }
    }
    for child in node.children() {
        append_gltf_node(
            child,
            transform,
            buffers,
            base_dir,
            material_indices,
            texture_cache,
            model,
        )?;
    }
    Ok(())
}

fn load_base_color_texture(
    info: gltf::texture::Info<'_>,
    buffers: &[Vec<u8>],
    base_dir: &Path,
    texture_cache: &mut HashMap<usize, Arc<TextureData>>,
) -> Result<Arc<TextureData>, String> {
    let image = info.texture().source();
    if let Some(texture) = texture_cache.get(&image.index()) {
        return Ok(Arc::clone(texture));
    }
    let texture = match image.source() {
        gltf::image::Source::Uri { uri, .. } => {
            return load_external_texture(&base_dir.join(uri)).inspect(|texture| {
                texture_cache.insert(image.index(), Arc::clone(texture));
            });
        }
        gltf::image::Source::View { view, .. } => {
            let buffer = buffers
                .get(view.buffer().index())
                .ok_or_else(|| "base-color image references a missing buffer".to_owned())?;
            let start = view.offset();
            let end = start
                .checked_add(view.length())
                .filter(|&end| end <= buffer.len())
                .ok_or_else(|| "base-color image buffer range is invalid".to_owned())?;
            decode_texture(&buffer[start..end])?
        }
    };
    texture_cache.insert(image.index(), Arc::clone(&texture));
    Ok(texture)
}

fn load_external_texture(path: &Path) -> Result<Arc<TextureData>, String> {
    let cache_path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let cache = EXTERNAL_TEXTURE_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(texture) = cache
        .lock()
        .map_err(|_| "external texture cache lock poisoned".to_owned())?
        .get(&cache_path)
        .cloned()
    {
        return Ok(texture);
    }
    let encoded = fs::read(path).map_err(|error| {
        format!(
            "could not read base-color texture {}: {error}",
            path.display()
        )
    })?;
    let texture = decode_texture(&encoded)?;
    cache
        .lock()
        .map_err(|_| "external texture cache lock poisoned".to_owned())?
        .insert(cache_path, Arc::clone(&texture));
    Ok(texture)
}

fn decode_texture(encoded: &[u8]) -> Result<Arc<TextureData>, String> {
    let decoded = image::load_from_memory(encoded)
        .map_err(|error| format!("could not decode base-color texture: {error}"))?
        .to_rgba8();
    Ok(Arc::new(TextureData {
        width: decoded.width(),
        height: decoded.height(),
        rgba8: Arc::from(decoded.into_raw()),
    }))
}

pub fn load_texture_file(path: impl AsRef<Path>) -> Result<Arc<TextureData>, String> {
    load_external_texture(path.as_ref())
}

pub fn parse_obj(contents: &str) -> Result<Model, String> {
    let mut positions = Vec::new();
    let mut model = Model::default();

    for (line_number, line) in contents.lines().enumerate() {
        let line_number = line_number + 1;
        let mut fields = line.split_whitespace();
        match fields.next() {
            Some("v") => {
                let mut values = fields.map(str::parse::<f32>);
                let x = parse_coordinate(values.next(), line_number)?;
                let y = parse_coordinate(values.next(), line_number)?;
                let z = parse_coordinate(values.next(), line_number)?;
                if !x.is_finite() || !y.is_finite() || !z.is_finite() {
                    return Err(format!(
                        "non-finite vertex coordinate on line {line_number}"
                    ));
                }
                positions.push([x, y, z]);
            }
            Some("f") => {
                let face = fields
                    .take_while(|field| !field.starts_with('#'))
                    .map(|field| resolve_vertex_index(field, positions.len(), line_number))
                    .collect::<Result<Vec<_>, _>>()?;
                if face.len() < 3 {
                    return Err(format!(
                        "face has fewer than three vertices on line {line_number}"
                    ));
                }
                for corner in 1..face.len() - 1 {
                    let points = [
                        positions[face[0]],
                        positions[face[corner]],
                        positions[face[corner + 1]],
                    ];
                    let normal = face_normal(points, line_number)?;
                    let base = model.vertices.len() as u32;
                    model.vertices.extend(points.map(|pos| Vertex {
                        pos,
                        normal,
                        uv: [0.0; 2],
                    }));
                    model.indices.extend_from_slice(&[base, base + 1, base + 2]);
                }
            }
            _ => {}
        }
    }

    if model.indices.is_empty() {
        return Err("OBJ contains no faces".to_owned());
    }
    Ok(model)
}

fn parse_coordinate(
    value: Option<Result<f32, std::num::ParseFloatError>>,
    line: usize,
) -> Result<f32, String> {
    value
        .ok_or_else(|| format!("missing vertex coordinate on line {line}"))?
        .map_err(|error| format!("invalid vertex coordinate on line {line}: {error}"))
}

fn resolve_vertex_index(field: &str, position_count: usize, line: usize) -> Result<usize, String> {
    let raw_index = field
        .split('/')
        .next()
        .unwrap_or_default()
        .parse::<isize>()
        .map_err(|error| format!("invalid face index on line {line}: {error}"))?;
    let index = if raw_index > 0 {
        raw_index - 1
    } else {
        position_count as isize + raw_index
    };
    if index < 0 || index >= position_count as isize {
        return Err(format!("face index out of range on line {line}"));
    }
    Ok(index as usize)
}

fn face_normal(points: [[f32; 3]; 3], line: usize) -> Result<[f32; 3], String> {
    let a = subtract(points[1], points[0]);
    let b = subtract(points[2], points[0]);
    let cross = [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ];
    let length = cross
        .iter()
        .map(|component| component * component)
        .sum::<f32>()
        .sqrt();
    if !length.is_finite() || length <= f32::EPSILON {
        return Err(format!("degenerate face on line {line}"));
    }
    Ok([cross[0] / length, cross[1] / length, cross[2] / length])
}

fn subtract(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_faces_with_uv_fields_and_relative_indices() {
        let model =
            parse_obj("v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nf -4/1 -3/2 -2/3 -1/4\n").unwrap();
        assert_eq!(model.vertices.len(), 6);
        assert_eq!(model.indices, [0, 1, 2, 3, 4, 5]);
        assert!(model
            .vertices
            .iter()
            .all(|vertex| vertex.normal == [0.0, 0.0, 1.0]));
    }

    #[test]
    fn rejects_out_of_range_face_indices() {
        let error = parse_obj("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 4\n").unwrap_err();
        assert!(error.contains("out of range"));
    }

    #[test]
    fn imports_downloaded_modular_building_gltf() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/models/buildings/Prop_Crate.gltf");
        let model = load_gltf(path).unwrap();
        assert!(!model.vertices.is_empty());
        assert!(!model.indices.is_empty());
        assert!(model
            .indices
            .iter()
            .all(|&index| (index as usize) < model.vertices.len()));
        assert!(model.vertices.iter().all(|vertex| {
            vertex.pos.iter().all(|value| value.is_finite())
                && vertex.normal.iter().all(|value| value.is_finite())
        }));
    }

    #[test]
    fn loads_existing_terrain_noise_texture() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/models/textures/T_Noise_Terrain.png");
        let texture = load_texture_file(path).unwrap();
        assert!(texture.width > 0 && texture.height > 0);
        assert_eq!(
            texture.rgba8.len(),
            texture.width as usize * texture.height as usize * 4
        );
    }

    #[test]
    fn imports_full_body_character_gltf_and_fits_height() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/models/Humanoids/Superhero_Male_FullBody.gltf");
        let mut model = load_gltf(path).unwrap();
        assert_eq!(model.materials.len(), 3);
        assert!(model
            .materials
            .iter()
            .all(|material| material.base_color_texture.is_some()));
        assert!(model.surfaces.iter().all(|surface| {
            surface.material_index < model.materials.len()
                && surface.first_index + surface.index_count <= model.indices.len() as u32
        }));
        assert!(model
            .vertices
            .iter()
            .all(|vertex| { vertex.uv.iter().all(|coordinate| coordinate.is_finite()) }));
        model.fit_height(1.8).unwrap();
        let min_y = model
            .vertices
            .iter()
            .map(|vertex| vertex.pos[1])
            .fold(f32::INFINITY, f32::min);
        let max_y = model
            .vertices
            .iter()
            .map(|vertex| vertex.pos[1])
            .fold(f32::NEG_INFINITY, f32::max);
        assert!(min_y.abs() < 1e-4);
        assert!((max_y - 1.8).abs() < 1e-3);
    }

    #[test]
    fn composed_building_prefabs_have_valid_geometry_and_material_ranges() {
        let asset_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let footprints = [
            ("house", [4.0, 4.0]),
            ("workshop", [6.0, 5.0]),
            ("wheat_storage", [4.0, 4.0]),
            ("ore_storage", [4.0, 4.0]),
            ("iron_mine", [4.0, 4.0]),
        ];
        for (name, footprint) in footprints {
            let recipe = asset_root.join(format!("prefabs/buildings/{name}.json"));
            let model = load_prefab(&asset_root, recipe)
                .unwrap_or_else(|error| panic!("could not compose {name}: {error}"));
            assert!(!model.vertices.is_empty(), "{name} has no vertices");
            assert!(!model.indices.is_empty(), "{name} has no indices");
            assert!(model
                .indices
                .iter()
                .all(|&index| (index as usize) < model.vertices.len()));
            assert!(model.surfaces.iter().all(|surface| {
                surface.material_index < model.materials.len()
                    && surface.first_index + surface.index_count <= model.indices.len() as u32
            }));
            assert!(model.vertices.iter().all(|vertex| {
                vertex.pos.iter().all(|value| value.is_finite())
                    && vertex.normal.iter().all(|value| value.is_finite())
                    && vertex.uv.iter().all(|value| value.is_finite())
            }));
            let max_x = model
                .vertices
                .iter()
                .map(|vertex| vertex.pos[0])
                .fold(f32::NEG_INFINITY, f32::max);
            let min_x = model
                .vertices
                .iter()
                .map(|vertex| vertex.pos[0])
                .fold(f32::INFINITY, f32::min);
            let max_z = model
                .vertices
                .iter()
                .map(|vertex| vertex.pos[2])
                .fold(f32::NEG_INFINITY, f32::max);
            let min_z = model
                .vertices
                .iter()
                .map(|vertex| vertex.pos[2])
                .fold(f32::INFINITY, f32::min);
            assert!(min_x >= -2.0, "{name} exceeds west overhang: min_x={min_x}");
            assert!(
                max_x <= footprint[0] + 2.0,
                "{name} exceeds width envelope: max_x={max_x}"
            );
            assert!(
                min_z >= -2.0,
                "{name} exceeds north overhang: min_z={min_z}"
            );
            assert!(
                max_z <= footprint[1] + 2.0,
                "{name} exceeds depth envelope: max_z={max_z}"
            );
        }
    }
}
