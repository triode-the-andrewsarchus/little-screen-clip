use super::*;
use serde_json::json;
use std::io::Write;
use wgpu_3dgs_viewer::core::glam::Vec3;
use zip::write::SimpleFileOptions;

fn metadata(version: u64, bands: u64) -> Value {
    let mut meta = if version == 2 {
        json!({"version": 2, "count": 1,
            "means": {"mins": [std::f32::consts::LN_2, std::f32::consts::LN_2, std::f32::consts::LN_2], "maxs": [std::f32::consts::LN_2, std::f32::consts::LN_2, std::f32::consts::LN_2], "files": ["means-l.webp", "means-u.webp"]},
            "scales": {"codebook": vec![-2.0f32; 256], "files": ["scales.webp"]},
            "quats": {"files": ["quats.webp"]},
            "sh0": {"codebook": (0..256).map(|i| i as f32 * 0.01).collect::<Vec<_>>(), "files": ["sh0.webp"]}})
    } else {
        json!({"means": {"shape": [1,3], "mins": [std::f32::consts::LN_2, std::f32::consts::LN_2, std::f32::consts::LN_2], "maxs": [std::f32::consts::LN_2, std::f32::consts::LN_2, std::f32::consts::LN_2], "files": ["means-l.webp", "means-u.webp"]},
            "scales": {"mins": [-2.0,-2.0,-2.0], "maxs": [-2.0,-2.0,-2.0], "files": ["scales.webp"]},
            "quats": {"encoding": "quaternion_packed", "files": ["quats.webp"]},
            "sh0": {"mins": [0.1,0.2,0.3,1.0], "maxs": [0.1,0.2,0.3,1.0], "files": ["sh0.webp"]}})
    };
    if bands > 0 {
        meta["shN"] = if version == 2 {
            json!({"bands": bands, "count": 1, "codebook": (0..256).map(|i| i as f32 * 0.01).collect::<Vec<_>>(), "files": ["centroids.webp", "labels.webp"]})
        } else {
            let coefficients = [0, 3, 8, 15][bands as usize];
            json!({"shape": [1,coefficients * 3], "mins": 0.0, "maxs": 2.55, "files": ["centroids.webp", "labels.webp"]})
        };
    }
    meta
}

fn archive(meta: Value, label: [u8; 4]) -> Vec<u8> {
    let cursor = Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(cursor);
    zip.start_file("scene/meta.json", SimpleFileOptions::default())
        .unwrap();
    zip.write_all(&serde_json::to_vec(&meta).unwrap()).unwrap();
    let centroids = (0..15u8)
        .flat_map(|i| [i + 1, i + 2, i + 3, 255])
        .collect::<Vec<_>>();
    for (name, pixels) in [
        ("means-l.webp", vec![0, 0, 0, 255]),
        ("means-u.webp", vec![0, 0, 0, 255]),
        ("scales.webp", vec![0, 0, 0, 255]),
        ("quats.webp", vec![128, 128, 128, 252]),
        ("sh0.webp", vec![10, 20, 30, 200]),
        ("centroids.webp", centroids),
        ("labels.webp", label.to_vec()),
    ] {
        let mut webp = Vec::new();
        image::codecs::webp::WebPEncoder::new_lossless(&mut webp)
            .encode(
                &pixels,
                (pixels.len() / 4) as u32,
                1,
                image::ExtendedColorType::Rgba8,
            )
            .unwrap();
        zip.start_file(format!("scene/{name}"), SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&webp).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

#[test]
fn bundled_sog_versions_preserve_position_scale_rotation_color_and_sh() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("fixture.sog");
    for version in [1, 2] {
        for bands in 0..=3 {
            std::fs::write(&path, archive(metadata(version, bands), [0, 0, 0, 255])).unwrap();
            let scene = crate::SplatScene::load(&path).unwrap();
            let gaussian = &scene.gaussians[0];
            assert!((gaussian.pos - Vec3::ONE).length() < 1e-5);
            assert!((gaussian.scale - Vec3::splat((-2.0f32).exp())).length() < 1e-4);
            assert!(gaussian.rot.w > 0.99);
            assert!((i16::from(gaussian.color.x) - 135).abs() <= 1);
            assert!((i16::from(gaussian.color.y) - 142).abs() <= 1);
            assert!((i16::from(gaussian.color.z) - 149).abs() <= 1);
            assert_eq!(gaussian.color.w, if version == 2 { 200 } else { 186 });
            let coefficients = [0, 3, 8, 15][bands as usize];
            for (index, coefficient) in gaussian.sh.iter().enumerate() {
                let expected = if index < coefficients {
                    Vec3::new(index as f32 + 1.0, index as f32 + 2.0, index as f32 + 3.0) * 0.01
                } else {
                    Vec3::ZERO
                };
                assert!(
                    (*coefficient - expected).length() < 1e-3,
                    "version={version}, bands={bands}, index={index}"
                );
            }
        }
    }
}

#[test]
fn sog_rejects_inconsistent_atlases_dictionaries_labels_versions_and_counts() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("invalid.sog");
    for (field, value) in [
        ("/count", json!(2)),
        ("/count", json!(u64::MAX)),
        ("/version", json!(3)),
        ("/scales/codebook", json!([0.0])),
        ("/shN/bands", json!(4)),
        ("/quats/files", json!(["missing.webp"])),
    ] {
        let mut meta = metadata(2, 3);
        *meta.pointer_mut(field).unwrap() = value;
        std::fs::write(&path, archive(meta, [0, 0, 0, 255])).unwrap();
        assert!(crate::SplatScene::load(&path).is_err(), "{field}");
    }
    std::fs::write(&path, archive(metadata(2, 3), [255, 255, 0, 255])).unwrap();
    assert!(crate::SplatScene::load(&path).is_err());
}

#[test]
fn sog_rejects_metadata_names_that_disagree_with_the_upstream_search() {
    let cursor = Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(cursor);
    for name in ["othermeta.json", "scene/meta.json"] {
        zip.start_file(name, SimpleFileOptions::default()).unwrap();
        zip.write_all(&serde_json::to_vec(&metadata(2, 0)).unwrap())
            .unwrap();
    }
    let bytes = zip.finish().unwrap().into_inner();
    let error = validate_archive(&bytes).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("metadata must be named meta.json")
    );
}

#[test]
#[ignore = "requires a native compute-capable GPU adapter"]
fn sog_bakes_visible_frames_with_the_native_renderer() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("render.sog");
    std::fs::write(&path, archive(metadata(2, 3), [0, 0, 0, 255])).unwrap();
    let scene = crate::SplatScene::load(&path).unwrap();
    let mut renderer = crate::SplatRenderer::new(std::sync::Arc::new(scene)).unwrap();
    let orbit = renderer.bake_orbit(97, 64, || false).unwrap();
    drop(renderer);
    assert_eq!(orbit.frames().len(), 30);
    assert!(
        orbit
            .frames()
            .iter()
            .all(|image| image.pixels().any(|pixel| pixel[0] > 20))
    );
    assert_ne!(orbit.frames()[0], orbit.frames()[15]);
}
