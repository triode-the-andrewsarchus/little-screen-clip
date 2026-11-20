//! Adapts Spark's shared Gaussian-array representation for the native renderer.
use spark_lib::{gsplat::GsplatArray, tsplat::Tsplat};
use wgpu_3dgs_viewer::core::{
    Gaussian,
    glam::{Quat, U8Vec4, Vec3, Vec4},
};

pub(crate) fn gaussians(decoded: &GsplatArray) -> Vec<Gaussian> {
    decoded
        .splats
        .iter()
        .enumerate()
        .map(|(index, splat)| {
            let color = Vec3::from_array(splat.rgb().to_vec3().to_array()).extend(splat.opacity());
            let color = (color * 255.0).clamp(Vec4::ZERO, Vec4::splat(255.0));
            let mut sh = [Vec3::ZERO; 15];
            let bands = [
                decoded.sh1.get(index).map(|band| band.to_array().to_vec()),
                decoded.sh2.get(index).map(|band| band.to_array().to_vec()),
                decoded.sh3.get(index).map(|band| band.to_array().to_vec()),
            ];
            let mut offset = 0;
            for band in bands.into_iter().flatten() {
                for coefficient in band.as_chunks::<3>().0 {
                    sh[offset] = Vec3::from_array(*coefficient);
                    offset += 1;
                }
            }
            Gaussian {
                pos: Vec3::from_array(splat.center().to_vec3().to_array()),
                rot: Quat::from_array(splat.quaternion().to_array()).normalize(),
                scale: Vec3::from_array(splat.scales().to_vec3().to_array()),
                color: U8Vec4::from_array(color.to_array().map(|value| value.round() as u8)),
                sh,
            }
        })
        .collect()
}
