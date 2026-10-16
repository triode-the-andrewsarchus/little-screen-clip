//! Static FBX scene conversion. Texture files and animation are not loaded.
use super::{Triangle, Vec3};
use std::path::Path;

const MAX_TRIANGLES: usize = 2_000_000;
const IMPORT_MEMORY: usize = 256 * 1024 * 1024;

pub(super) fn load(path: &Path) -> Result<Vec<Triangle>, String> {
    let file = little_clip_core::kernel_interface::open_content(path, true)
        .map_err(|error| format!("Failed to open FBX: {error}"))?;
    let opts = ufbx::LoadOpts {
        file_format: ufbx::FileFormat::Fbx,
        ignore_animation: true,
        ignore_embedded: true,
        load_external_files: false,
        evaluate_skinning: true,
        generate_missing_normals: true,
        target_axes: ufbx::CoordinateAxes::right_handed_y_up(),
        target_unit_meters: 1.0,
        temp_allocator: ufbx::AllocatorOpts {
            memory_limit: IMPORT_MEMORY,
            ..Default::default()
        },
        result_allocator: ufbx::AllocatorOpts {
            memory_limit: IMPORT_MEMORY,
            ..Default::default()
        },
        ..Default::default()
    };
    let scene = ufbx::load_stream(ufbx::Stream::File(file), opts)
        .map_err(|error| format!("Failed to parse FBX: {error:?}"))?;
    let mut triangles = Vec::new();
    let mut indices = Vec::new();
    let mut imported = 0usize;
    for node in &scene.nodes {
        let Some(mesh) = node.mesh.as_ref() else {
            continue;
        };
        // Count instances as well as unique meshes to bound flattened geometry.
        imported = imported.saturating_add(mesh.num_triangles);
        if imported > MAX_TRIANGLES {
            return Err("FBX exceeds the two-million-triangle static import limit".into());
        }
        let normal_matrix = ufbx::matrix_for_normals(&node.geometry_to_world);
        for (face_index, face) in mesh.faces.iter().enumerate() {
            let count = ufbx::triangulate_face_vec(&mut indices, mesh, *face) as usize;
            let color = material_color(node, mesh, face_index);
            for corners in indices[..count * 3].chunks_exact(3) {
                if let Some(mut triangle) = triangle(node, mesh, &normal_matrix, corners) {
                    triangle.color = color;
                    triangles.push(triangle);
                }
            }
        }
    }
    Ok(triangles)
}

fn triangle(
    node: &ufbx::Node,
    mesh: &ufbx::Mesh,
    normal_matrix: &ufbx::Matrix,
    corners: &[u32],
) -> Option<Triangle> {
    let positions = &mesh.skinned_position;
    let mut vertices = [Vec3::ZERO; 3];
    for (vertex, &corner) in vertices.iter_mut().zip(corners) {
        let position = positions[corner as usize];
        let world = if mesh.skinned_is_local {
            ufbx::transform_position(&node.geometry_to_world, position)
        } else {
            position
        };
        *vertex = Vec3::new(world.x as f32, world.y as f32, world.z as f32);
        if !vertex.x.is_finite() || !vertex.y.is_finite() || !vertex.z.is_finite() {
            return None;
        }
    }
    let [v1, mut v2, mut v3] = vertices;
    let cross = v2.sub(v1).cross(v3.sub(v1));
    let length = cross.length();
    if !length.is_finite() || length <= 0.0 {
        return None;
    }
    let mut normal = cross.scale(1.0 / length);
    if mesh.skinned_normal.exists {
        let source = mesh.skinned_normal[corners[0] as usize];
        let world = if mesh.skinned_is_local {
            ufbx::transform_direction(normal_matrix, source)
        } else {
            source
        };
        let expected = Vec3::new(world.x as f32, world.y as f32, world.z as f32);
        // Negative-scale instances can invert winding; preserve outward normals.
        if normal.dot(expected) < 0.0 {
            std::mem::swap(&mut v2, &mut v3);
            normal = normal.scale(-1.0);
        }
    }
    Some(Triangle::new(normal, v1, v2, v3))
}

fn material_color(node: &ufbx::Node, mesh: &ufbx::Mesh, face: usize) -> Option<[f32; 4]> {
    let index = mesh.face_material.get(face).copied().unwrap_or(0) as usize;
    let material = node.materials.get(index)?;
    let map = if material.pbr.base_color.has_value {
        &material.pbr.base_color
    } else {
        &material.fbx.diffuse_color
    };
    if !map.has_value {
        return None;
    }
    let value = map.value_vec4;
    let rgb = [value.x as f32, value.y as f32, value.z as f32];
    if !rgb.iter().all(|component| component.is_finite()) {
        return None;
    }
    Some([
        rgb[0].clamp(0.0, 1.0),
        rgb[1].clamp(0.0, 1.0),
        rgb[2].clamp(0.0, 1.0),
        1.0,
    ])
}
