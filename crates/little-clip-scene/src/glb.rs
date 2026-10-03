//! Gaussian GLB import using OxideAV's container, validation and accessor readers.

use crate::{SceneError, SplatScene, sh_rotation::ShRotation};
use oxideav_gltf::{
    accessor, glb,
    json_model::{GltfRoot, Node, Primitive},
    quantization, validation,
};
use std::{path::Path, sync::Arc};
use wgpu_3dgs_viewer::core::{
    Gaussian,
    glam::{Mat3, Mat4, Quat, U8Vec4, Vec3},
};

const MAX_BYTES: u64 = 512 * 1024 * 1024;
const MAX_POINTS: usize = 2_000_000;

fn invalid(message: impl ToString) -> SceneError {
    SceneError::Invalid(message.to_string())
}

pub(crate) fn load(path: &Path) -> Result<SplatScene, SceneError> {
    let file = std::fs::File::open(path)?;
    if file.metadata()?.len() > MAX_BYTES {
        return Err(invalid("GLB exceeds the 512 MiB import budget"));
    }
    let bytes = crate::read_bounded(file, MAX_BYTES, "GLB")?;
    decode(&bytes)
}

fn decode(bytes: &[u8]) -> Result<SplatScene, SceneError> {
    let payload = glb::parse(bytes).map_err(invalid)?;
    validation::check_json_byte_length(payload.json).map_err(invalid)?;
    validation::check_json_depth(payload.json).map_err(invalid)?;
    let root: GltfRoot = serde_json::from_slice(payload.json).map_err(invalid)?;
    if !root
        .meshes
        .iter()
        .flat_map(|mesh| &mesh.primitives)
        .any(|primitive| {
            primitive
                .extensions
                .as_ref()
                .is_some_and(|ext| ext.khr_gaussian_splatting.is_some())
        })
    {
        return Err(SceneError::MeshGlb);
    }
    validate(&root)?;
    let bin = payload
        .bin
        .ok_or_else(|| invalid("Gaussian GLB has no embedded buffer"))?;
    if root.buffers.len() != 1
        || root.buffers[0].uri.is_some()
        || root.buffers[0].byte_length as usize > bin.len()
    {
        return Err(invalid("Gaussian GLB requires one valid embedded buffer"));
    }
    let buffers = [Arc::new(bin.to_vec())];
    instantiate_scene(&root, &buffers)
}

fn instantiate_scene(root: &GltfRoot, buffers: &[Arc<Vec<u8>>]) -> Result<SplatScene, SceneError> {
    let scene = root
        .scenes
        .get(root.scene.unwrap_or(0) as usize)
        .ok_or_else(|| invalid("GLB has no selected scene"))?;
    let mut stack: Vec<_> = scene
        .nodes
        .iter()
        .map(|index| (*index, Mat4::IDENTITY, 0usize))
        .collect();
    let mut seen = vec![false; root.nodes.len()];
    let mut gaussians = Vec::new();
    let mut linear = None;
    while let Some((index, parent, depth)) = stack.pop() {
        let node = root
            .nodes
            .get(index as usize)
            .ok_or_else(|| invalid("GLB node index is out of bounds"))?;
        if depth > 256 || std::mem::replace(&mut seen[index as usize], true) {
            return Err(invalid("GLB node hierarchy is too deep or repeats a node"));
        }
        let world = parent * node_matrix(node);
        if let Some(mesh) = node.mesh {
            let mesh = root
                .meshes
                .get(mesh as usize)
                .ok_or_else(|| invalid("GLB mesh index is out of bounds"))?;
            for primitive in &mesh.primitives {
                append_primitive(root, buffers, primitive, world, &mut gaussians, &mut linear)?;
            }
        }
        stack.extend(node.children.iter().map(|child| (*child, world, depth + 1)));
    }
    let mut scene = SplatScene::from_gaussians(gaussians)?;
    scene.linear_color = linear.unwrap_or(false);
    Ok(scene)
}

fn validate(root: &GltfRoot) -> Result<(), SceneError> {
    if root.nodes.len() > 100_000
        || root
            .accessors
            .iter()
            .any(|acc| acc.count as usize > MAX_POINTS)
    {
        return Err(invalid("GLB exceeds the scene import budget"));
    }
    validation::check_asset_version(&root.asset).map_err(invalid)?;
    validation::validate_index_references(root).map_err(invalid)?;
    validation::validate_accessors(&root.accessors).map_err(invalid)?;
    validation::validate_nodes(&root.nodes, &root.animations).map_err(invalid)?;
    validation::validate_extension_stack(root).map_err(invalid)?;
    for (index, view) in root.buffer_views.iter().enumerate() {
        validation::validate_bufferview_fits_buffer(index, view, &root.buffers).map_err(invalid)?;
    }
    for (index, acc) in root.accessors.iter().enumerate() {
        validation::validate_accessor_fits_bufferview(index, acc, &root.buffer_views)
            .map_err(invalid)?;
    }
    Ok(())
}

fn node_matrix(node: &Node) -> Mat4 {
    node.matrix
        .map(|matrix| Mat4::from_cols_array(&matrix))
        .unwrap_or_else(|| {
            Mat4::from_scale_rotation_translation(
                Vec3::from_array(node.scale.unwrap_or([1.0; 3])),
                Quat::from_array(node.rotation.unwrap_or([0.0, 0.0, 0.0, 1.0])),
                Vec3::from_array(node.translation.unwrap_or([0.0; 3])),
            )
        })
}

struct Attributes<'a> {
    root: &'a GltfRoot,
    buffers: &'a [Arc<Vec<u8>>],
    primitive: &'a Primitive,
}
impl Attributes<'_> {
    fn read<T>(
        &self,
        name: &str,
        reader: impl FnOnce(
            &oxideav_gltf::json_model::Accessor,
            &accessor::AccessorView<'_>,
        ) -> oxideav_gltf::Result<T>,
    ) -> Result<T, SceneError> {
        let index = self
            .primitive
            .attributes
            .get(name)
            .ok_or_else(|| invalid(format!("GLB is missing {name}")))?;
        let acc = self
            .root
            .accessors
            .get(*index as usize)
            .ok_or_else(|| invalid("GLB accessor index is out of bounds"))?;
        let bytes = accessor::materialise_accessor(acc, &self.root.buffer_views, self.buffers)
            .map_err(invalid)?;
        let view = accessor::view_from_materialised(acc, &bytes).map_err(invalid)?;
        reader(acc, &view).map_err(invalid)
    }
    fn vectors(&self, name: &str) -> Result<Vec<[f32; 3]>, SceneError> {
        self.read(name, quantization::dequantize_vec3)
    }
}

fn append_primitive(
    root: &GltfRoot,
    buffers: &[Arc<Vec<u8>>],
    primitive: &Primitive,
    world: Mat4,
    output: &mut Vec<Gaussian>,
    linear: &mut Option<bool>,
) -> Result<(), SceneError> {
    check_primitive_color_space(primitive, linear)?;
    let field = read_primitive(root, buffers, primitive)?;
    if output.len().saturating_add(field.len()) > MAX_POINTS {
        return Err(invalid("GLB has more than two million splats"));
    }
    let transform = Transform::new(world)?;
    for splat in field.splats {
        output.push(transform.gaussian(splat)?);
    }
    Ok(())
}

fn check_primitive_color_space(
    primitive: &Primitive,
    linear: &mut Option<bool>,
) -> Result<(), SceneError> {
    let descriptor = primitive
        .extensions
        .as_ref()
        .and_then(|ext| ext.khr_gaussian_splatting.as_ref())
        .ok_or_else(|| invalid("GLB mixes triangle meshes or point clouds with Gaussian splats"))?;
    if descriptor.kernel != "ellipse"
        || descriptor.projection.as_deref().unwrap_or("perspective") != "perspective"
    {
        return Err(invalid("Unsupported GLB Gaussian kernel or projection"));
    }
    let is_linear = match descriptor.color_space.as_str() {
        "srgb_rec709_display" => false,
        "lin_rec709_display" => true,
        _ => return Err(invalid("Unsupported GLB Gaussian color space")),
    };
    if linear.is_some_and(|previous| previous != is_linear) {
        return Err(invalid("GLB mixes Gaussian color spaces"));
    }
    *linear = Some(is_linear);
    Ok(())
}

fn read_primitive(
    root: &GltfRoot,
    buffers: &[Arc<Vec<u8>>],
    primitive: &Primitive,
) -> Result<oxideav_gltf::splatting::SplatField, SceneError> {
    let attributes = Attributes {
        root,
        buffers,
        primitive,
    };
    let position = attributes.vectors("POSITION")?;
    let scale = attributes.vectors("KHR_gaussian_splatting:SCALE")?;
    let rotation = attributes.read(
        "KHR_gaussian_splatting:ROTATION",
        quantization::dequantize_vec4,
    )?;
    let opacity = attributes.read(
        "KHR_gaussian_splatting:OPACITY",
        quantization::dequantize_scalar,
    )?;
    let mut sh = Vec::new();
    for degree in 0..=3 {
        for coefficient in 0..=(2 * degree) {
            let name = format!("KHR_gaussian_splatting:SH_DEGREE_{degree}_COEF_{coefficient}");
            if primitive.attributes.contains_key(&name) {
                sh.push(attributes.vectors(&name)?);
            }
        }
    }
    oxideav_gltf::splatting::SplatField::from_attributes(
        &position, &rotation, &scale, &opacity, &sh,
    )
    .ok_or_else(|| invalid("GLB Gaussian attribute counts disagree"))
}

struct Transform {
    matrix: Mat4,
    linear: Mat3,
    sh: ShRotation,
}
impl Transform {
    fn new(matrix: Mat4) -> Result<Self, SceneError> {
        let linear = Mat3::from_mat4(matrix);
        let scales = Vec3::new(
            linear.x_axis.length(),
            linear.y_axis.length(),
            linear.z_axis.length(),
        );
        let rotation = Mat3::from_cols(
            linear.x_axis / scales.x,
            linear.y_axis / scales.y,
            linear.z_axis / scales.z,
        );
        if !scales.is_finite()
            || !rotation.is_finite()
            || !matrix.is_finite()
            || scales.min_element() <= 0.0
            || (rotation.determinant() - 1.0).abs() > 1e-4
        {
            return Err(invalid(
                "GLB Gaussian node has a non-decomposable or negative transform",
            ));
        }
        Ok(Self {
            matrix,
            linear,
            sh: ShRotation::new(rotation),
        })
    }
    fn gaussian(&self, splat: oxideav_gltf::splatting::Splat) -> Result<Gaussian, SceneError> {
        if !splat.opacity.is_finite()
            || !(0.0..=1.0).contains(&splat.opacity)
            || splat.sh.is_empty()
            || splat.sh.iter().flatten().any(|value| !value.is_finite())
        {
            return Err(invalid("GLB Gaussian opacity or SH is invalid"));
        }
        let local_scale = Vec3::from_array(splat.scale);
        let local_rotation = Quat::from_array(splat.rotation);
        if !local_scale.is_finite()
            || local_scale.min_element() <= 0.0
            || !local_rotation.is_finite()
            || (local_rotation.length_squared() - 1.0).abs() > 0.01
        {
            return Err(invalid("GLB Gaussian scale or quaternion is invalid"));
        }
        let c = self.linear
            * Mat3::from_quat(local_rotation.normalize())
            * Mat3::from_diagonal(local_scale);
        let covariance = c * c.transpose();
        if !covariance.is_finite() {
            return Err(invalid("GLB Gaussian covariance overflows"));
        }
        let eig =
            nalgebra::Matrix3::from_column_slice(&covariance.to_cols_array()).symmetric_eigen();
        if eig
            .eigenvalues
            .iter()
            .any(|value| !value.is_finite() || *value <= 0.0)
        {
            return Err(invalid("GLB Gaussian covariance is degenerate"));
        }
        let mut axes =
            Mat3::from_cols_array(eig.eigenvectors.as_slice().try_into().expect("3x3 matrix"));
        if axes.determinant() < 0.0 {
            axes.x_axis = -axes.x_axis;
        }
        let diffuse = (Vec3::from_array(splat.sh[0]) * 0.2820948 + Vec3::splat(0.5))
            .clamp(Vec3::ZERO, Vec3::ONE)
            * 255.0;
        let mut sh = [Vec3::ZERO; 15];
        for (destination, source) in sh.iter_mut().zip(splat.sh.iter().skip(1)) {
            *destination = Vec3::from_array(*source);
        }
        Ok(Gaussian {
            pos: self
                .matrix
                .transform_point3(Vec3::from_array(splat.position)),
            rot: Quat::from_mat3(&axes).normalize(),
            scale: Vec3::new(
                eig.eigenvalues[0].sqrt(),
                eig.eigenvalues[1].sqrt(),
                eig.eigenvalues[2].sqrt(),
            ),
            color: U8Vec4::new(
                diffuse.x.round() as u8,
                diffuse.y.round() as u8,
                diffuse.z.round() as u8,
                (splat.opacity * 255.0).round() as u8,
            ),
            sh: self.sh.apply(&sh),
        })
    }
}

#[cfg(test)]
mod tests;
