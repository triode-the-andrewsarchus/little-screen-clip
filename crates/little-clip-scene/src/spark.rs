//! Adapts Spark's shared Gaussian-array representation for the native renderer.
use spark_lib::{gsplat::GsplatArray, tsplat::Tsplat};
use wgpu_3dgs_viewer::core::{
    Gaussian,
    glam::{U8Vec4, Vec3},
};

pub(crate) fn gaussians(decoded: &GsplatArray) -> Vec<Gaussian> {
    decoded
        .splats
        .iter()
        .enumerate()
        .map(|(index, splat)| {
            let color = (splat.rgb().to_vec3().extend(splat.opacity()) * 255.0)
                .clamp(Vec3::ZERO.extend(0.0), Vec3::splat(255.0).extend(255.0));
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
                pos: splat.center().to_vec3(),
                rot: splat.quaternion().normalize(),
                scale: splat.scales().to_vec3(),
                color: U8Vec4::from_array(color.to_array().map(|value| value.round() as u8)),
                sh,
            }
        })
        .collect()
}
