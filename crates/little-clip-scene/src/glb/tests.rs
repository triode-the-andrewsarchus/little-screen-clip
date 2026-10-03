use super::*;
use serde_json::{Value, json};

fn fixture(quantized: bool, degree: usize) -> (Value, Vec<u8>) {
    let mut binary = Vec::new();
    let mut views = Vec::new();
    let mut accessors = Vec::new();
    let mut attributes = serde_json::Map::new();
    let mut add = |name: String, kind: &str, component: u32, normalized: bool, bytes: Vec<u8>| {
        while !binary.len().is_multiple_of(4) {
            binary.push(0);
        }
        views.push(json!({"buffer":0,"byteOffset":binary.len(),"byteLength":bytes.len()}));
        let mut accessor =
            json!({"bufferView":views.len()-1,"componentType":component,"type":kind,"count":2});
        if normalized {
            accessor["normalized"] = json!(true);
        }
        if name == "POSITION" {
            accessor["min"] = json!([-0.5, 0.0, 0.0]);
            accessor["max"] = json!([0.5, 0.3, 0.2]);
        }
        attributes.insert(name, json!(accessors.len()));
        accessors.push(accessor);
        binary.extend(bytes);
    };
    let floats = |values: &[f32]| {
        values
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<_>>()
    };
    add(
        "POSITION".into(),
        "VEC3",
        5126,
        false,
        floats(&[-0.5, 0.0, 0.0, 0.5, 0.3, 0.2]),
    );
    if quantized {
        add(
            "KHR_gaussian_splatting:ROTATION".into(),
            "VEC4",
            5120,
            true,
            vec![0, 0, 0, 127, 0, 0, 0, 127],
        );
        add(
            "KHR_gaussian_splatting:SCALE".into(),
            "VEC3",
            5121,
            true,
            vec![30, 50, 40, 30, 50, 40],
        );
        add(
            "KHR_gaussian_splatting:OPACITY".into(),
            "SCALAR",
            5121,
            true,
            vec![240, 240],
        );
    } else {
        add(
            "KHR_gaussian_splatting:ROTATION".into(),
            "VEC4",
            5126,
            false,
            floats(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]),
        );
        add(
            "KHR_gaussian_splatting:SCALE".into(),
            "VEC3",
            5126,
            false,
            floats(&[0.1, 0.2, 0.15, 0.1, 0.2, 0.15]),
        );
        add(
            "KHR_gaussian_splatting:OPACITY".into(),
            "SCALAR",
            5126,
            false,
            floats(&[0.94, 0.94]),
        );
    }
    for band in 0..=degree {
        for coefficient in 0..=2 * band {
            let v = if band == 0 {
                [1.0, -1.0, -0.5]
            } else {
                [0.02 * (coefficient + 1) as f32, 0.01, -0.03]
            };
            add(
                format!("KHR_gaussian_splatting:SH_DEGREE_{band}_COEF_{coefficient}"),
                "VEC3",
                5126,
                false,
                floats(&[v[0], v[1], v[2], v[0], v[1], v[2]]),
            );
        }
    }
    (
        json!({"asset":{"version":"2.0"},"extensionsUsed":["KHR_gaussian_splatting"],
        "scene":0,"scenes":[{"nodes":[0]}],"nodes":[{"mesh":0}],
        "buffers":[{"byteLength":binary.len()}],"bufferViews":views,"accessors":accessors,
        "meshes":[{"primitives":[{"mode":0,"attributes":attributes,"extensions":{"KHR_gaussian_splatting":{"kernel":"ellipse","colorSpace":"srgb_rec709_display"}}}]}]}),
        binary,
    )
}

fn decode_fixture(root: &Value, binary: &[u8]) -> Result<SplatScene, SceneError> {
    decode(&glb::encode(
        &serde_json::to_vec(root).unwrap(),
        Some(binary),
    ))
}

#[test]
fn glb_float_and_normalized_attributes_preserve_gaussians_and_all_sh_bands() {
    for quantized in [false, true] {
        for degree in 0..=3 {
            let (root, binary) = fixture(quantized, degree);
            let scene = decode_fixture(&root, &binary).unwrap();
            assert_eq!(scene.len(), 2);
            let gaussian = &scene.gaussians[0];
            assert_eq!(gaussian.pos, Vec3::new(-0.5, 0.0, 0.0));
            assert!((gaussian.color.w as i32 - 240).abs() <= 1);
            assert!(gaussian.color.x > 190);
            let expected = if quantized {
                Vec3::new(30.0, 50.0, 40.0) / 255.0
            } else {
                Vec3::new(0.1, 0.2, 0.15)
            };
            let r = Mat3::from_quat(gaussian.rot) * Mat3::from_diagonal(gaussian.scale);
            let expected_cov = Mat3::from_diagonal(expected * expected);
            assert!(r.mul_mat3(&r.transpose()).abs_diff_eq(expected_cov, 1e-6));
            let count = (degree + 1) * (degree + 1) - 1;
            assert!(gaussian.sh[..count].iter().all(|v| v.length() > 0.0));
            assert!(gaussian.sh[count..].iter().all(|v| *v == Vec3::ZERO));
        }
    }
}

#[test]
fn glb_node_instances_transform_covariance_positions_and_directional_color() {
    let (mut root, binary) = fixture(false, 3);
    let rotation = Quat::from_rotation_y(0.7);
    root["nodes"] = json!([{"translation":[1.0,2.0,3.0],"rotation":rotation.to_array(),"children":[1,2]},
        {"mesh":0,"scale":[2.0,3.0,4.0]}, {"mesh":0,"translation":[3.0,0.0,0.0]}]);
    let scene = decode_fixture(&root, &binary).unwrap();
    assert_eq!(scene.len(), 4);
    let wanted = Vec3::new(1.0, 2.0, 3.0) + rotation * Vec3::new(-1.0, 0.0, 0.0);
    let gaussian = scene
        .gaussians
        .iter()
        .find(|g| g.pos.abs_diff_eq(wanted, 1e-6))
        .unwrap();
    let c = Mat3::from_quat(gaussian.rot) * Mat3::from_diagonal(gaussian.scale);
    let expected = Mat3::from_quat(rotation) * Mat3::from_diagonal(Vec3::new(0.2, 0.6, 0.6));
    assert!((c * c.transpose()).abs_diff_eq(expected * expected.transpose(), 1e-6));
    let original = decode_fixture(&fixture(false, 3).0, &binary).unwrap();
    for direction in [
        Vec3::X,
        Vec3::Y,
        Vec3::Z,
        Vec3::new(1.0, 2.0, 3.0).normalize(),
    ] {
        let coefficients = |g: &Gaussian| {
            std::iter::once([0.0; 3])
                .chain(g.sh.map(|v| v.to_array()))
                .collect::<Vec<_>>()
        };
        let local = oxideav_gltf::splatting::evaluate(
            &coefficients(&original.gaussians[0]),
            3,
            direction.to_array(),
        );
        let world = oxideav_gltf::splatting::evaluate(
            &coefficients(gaussian),
            3,
            (rotation * direction).to_array(),
        );
        for (a, b) in local.into_iter().zip(world) {
            assert!((a - b).abs() < 1e-5, "{a} != {b}");
        }
    }
}

#[test]
fn glb_rejects_incomplete_attributes_invalid_spans_hierarchy_and_transforms() {
    let (root, binary) = fixture(false, 3);
    let mut bad = root.clone();
    bad["meshes"][0]["primitives"][0]["attributes"]
        .as_object_mut()
        .unwrap()
        .remove("KHR_gaussian_splatting:SH_DEGREE_1_COEF_1");
    assert!(decode_fixture(&bad, &binary).is_err());
    bad = root.clone();
    bad["bufferViews"][0]["byteLength"] = json!(4);
    assert!(decode_fixture(&bad, &binary).is_err());
    bad = root.clone();
    bad["nodes"][0]["children"] = json!([0]);
    assert!(decode_fixture(&bad, &binary).is_err());
    bad = root.clone();
    bad["nodes"][0]["scale"] = json!([-1.0, 1.0, 1.0]);
    assert!(decode_fixture(&bad, &binary).is_err());
    bad = root.clone();
    bad["accessors"][0]["count"] = json!(MAX_POINTS + 1);
    assert!(decode_fixture(&bad, &binary).is_err());
    assert!(decode(&glb::encode(b"{}", None)).is_err());
}

#[test]
#[ignore = "requires a native compute-capable GPU adapter"]
fn glb_loads_from_file_and_bakes_visible_orbit_in_both_color_spaces() {
    let directory = tempfile::tempdir().unwrap();
    for color_space in ["srgb_rec709_display", "lin_rec709_display"] {
        let (mut root, binary) = fixture(true, 3);
        root["meshes"][0]["primitives"][0]["extensions"]["KHR_gaussian_splatting"]["colorSpace"] =
            json!(color_space);
        let path = directory.path().join("scene.glb");
        std::fs::write(
            &path,
            glb::encode(&serde_json::to_vec(&root).unwrap(), Some(&binary)),
        )
        .unwrap();
        let scene = SplatScene::load(&path).unwrap();
        assert_eq!(scene.linear_color, color_space == "lin_rec709_display");
        let mut renderer = crate::SplatRenderer::new(Arc::new(scene)).unwrap();
        let orbit = renderer.bake_orbit(97, 64, || false).unwrap();
        assert_eq!(orbit.frames().len(), 30);
        assert!(orbit.frames()[0].pixels().any(|pixel| pixel[0] > 20));
        assert_ne!(orbit.frames()[0], orbit.frames()[15]);
    }
}
