use crate::{camera, scene};
use glam::{DVec3, Vec3};
use std::path::Path;

#[test]
fn reads_real_ogawa_hierarchy_geometry_camera() {
    let scene = scene::load(Path::new("tests/fixtures/basic.abc"), 0.0).unwrap();
    assert_eq!(scene.nodes.len(), 5);
    assert_eq!(scene.meshes, 1);
    assert_eq!(scene.vertices.len(), 36);
    let mesh = scene.nodes.iter().find(|n| n.kind == 2).unwrap();
    assert_eq!(mesh.path, "/asset/model/cubeShape");
    assert_eq!(scene.nodes[mesh.parent as usize].path, "/asset/model");
    let (min, max) = scene::bounds(&scene.vertices);
    assert_eq!(min, Vec3::new(1., -1.5, -1.));
    assert_eq!(max, Vec3::new(3., 1.5, 1.));
    let camera = &scene.cameras[0];
    assert_eq!(camera.focal, 50.);
    assert!((camera.aspect - 16. / 9.).abs() < 1e-10);
    let ndc = (camera.projection * camera.view).project_point3(DVec3::new(2., 0., 0.));
    assert!(
        (ndc.x + 0.1).abs() < 1e-10,
        "film offset must shift the image"
    );
}

#[test]
fn invalid_file_returns_error_without_crashing() {
    assert!(scene::load(Path::new("Cargo.toml"), 0.).is_err());
    assert!(scene::load(Path::new("tests/fixtures/not-found.abc"), 0.).is_err());
}

#[test]
#[ignore = "Run mayapy tests/maya_integration.py to generate Maya fixtures first"]
fn maya_smooth_mesh_without_normals_and_hard_mesh_with_normals() {
    let path = Path::new("tests/output/maya_shading.abc");
    let scene = scene::load(path, scene::default_sample(path).unwrap()).unwrap();
    let mesh = |name: &str| scene.nodes.iter().find(|n| n.path.ends_with(name)).unwrap();
    assert!(mesh("/smoothSphereShape").normals.is_empty());
    assert!(!mesh("/hardSphereShape").normals.is_empty());

    let mut shared = std::collections::HashMap::new();
    let mut hard_split = false;
    for vertex in &scene.vertices {
        let position = Vec3::from_array(vertex.position);
        let normal = Vec3::from_array(vertex.normal);
        if position.x < 0.0 {
            let radial = (position + Vec3::X * 1.25).normalize();
            assert!(radial.dot(normal).abs() > 0.98);
        }
        let other = shared
            .entry(vertex.position.map(f32::to_bits))
            .or_insert(normal);
        let same = normal.abs_diff_eq(*other, 1e-6);
        if position.x < 0.0 {
            assert!(same, "a smooth edge must share its normal");
        } else {
            hard_split |= !same;
        }
    }
    assert!(hard_split, "authored hard edges must retain split normals");
}

#[test]
fn rejects_invalid_camera_matrices() {
    let record = camera::CameraRecord {
        path: "/camera".into(),
        view: [0.; 16],
        projection: [0.; 16],
        width: 1920,
        height: 1080,
        pixel_aspect: 1.,
        focal_mm: 50.,
    };
    assert!(camera::from_record(&record).is_err());
}

#[test]
#[ignore = "Run mayapy tests/maya_selection.py first"]
fn selected_export_excludes_unselected_siblings() {
    for name in ["maya_selected.abc", "maya_selected_group.abc"] {
        let path = Path::new("tests/output").join(name);
        let scene = scene::load(&path, scene::default_sample(&path).unwrap()).unwrap();
        assert_eq!(scene.meshes, 1);
        assert_eq!(scene.vertices.len(), 36);
        assert!(
            scene
                .nodes
                .iter()
                .any(|n| n.path == "/sceneRoot/selectedGroup/chosen/chosenShape")
        );
        assert!(!scene.nodes.iter().any(|n| n.path.contains("excluded")));
        let (min, max) = scene::bounds(&scene.vertices);
        assert_eq!(min, Vec3::new(12., 1., -1.));
        assert_eq!(max, Vec3::new(14., 3., 1.));
        assert_eq!(scene.cameras.len(), 1);
        assert!(scene.cameras[0].source.starts_with("Maya render gate"));
        assert!(
            scene.cameras[0]
                .path
                .starts_with("/sceneRoot/cameraRig/shotCamera")
        );
        assert_eq!(
            scene.cameras[0].view.inverse().w_axis.truncate(),
            DVec3::new(10., 2., 8.)
        );
    }
}

#[test]
fn unicode_path_and_repeated_load_release_dll_memory() {
    let dir = std::env::temp_dir().join(format!("hdr-abc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("照明モデル.abc");
    std::fs::copy("tests/fixtures/basic.abc", &path).unwrap();
    for _ in 0..8 {
        assert_eq!(scene::load(&path, 0.).unwrap().vertices.len(), 36);
    }
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(dir).unwrap();
}

#[test]
#[ignore = "Run mayapy tests/maya_integration.py to generate Maya fixtures first"]
fn maya_render_camera_reference() {
    let base = Path::new("tests/output");
    let cases: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(base.join("maya_reference.json")).unwrap())
            .unwrap();
    for case in cases.as_array().unwrap() {
        let path = base.join(case["file"].as_str().unwrap());
        let seconds = scene::default_sample(&path).unwrap();
        let scene = scene::load(&path, seconds).unwrap();
        assert_eq!(scene.vertices.len(), 36);
        let c = &scene.cameras[0];
        assert!(c.source.starts_with("Maya render gate"));
        assert!(
            (c.projection.z_axis.w + 1.).abs() < 1e-6,
            "Perspective camera must divide by -Z"
        );
        assert!((c.aspect - 16. / 9.).abs() < 1e-6);
        let mesh = scene.nodes.iter().find(|n| n.kind == 2).unwrap();
        let abc_camera = scene.nodes.iter().find(|n| n.kind == 3).unwrap();
        assert!(
            (abc_camera.world.inverse() - c.view)
                .to_cols_array()
                .iter()
                .all(|x| x.abs() < 1e-6)
        );
        assert!(mesh.world.is_finite());
        for probe in case["probes"].as_array().unwrap() {
            let world = probe["world"].as_array().unwrap();
            let world = DVec3::new(
                world[0].as_f64().unwrap(),
                world[1].as_f64().unwrap(),
                world[2].as_f64().unwrap(),
            );
            let actual = (c.projection * c.view).project_point3(world).to_array();
            let expected = probe["ndc"].as_array().unwrap();
            for i in 0..3 {
                assert!(
                    (actual[i] - expected[i].as_f64().unwrap()).abs() < 1e-6,
                    "Maya projection mismatch: {path:?}"
                );
            }
        }
        assert!(
            scene::load(&path, seconds + 1.).is_err(),
            "stale time must be rejected"
        );
    }
    let path = base.join("maya_ortho.abc");
    let ortho = scene::load(&path, scene::default_sample(&path).unwrap()).unwrap();
    assert!((ortho.cameras[0].projection.w_axis.w - 1.).abs() < 1e-6);
    let path = base.join("maya_pixel_aspect.abc");
    let anamorphic = scene::load(&path, scene::default_sample(&path).unwrap()).unwrap();
    assert!((anamorphic.cameras[0].aspect - 8. / 3.).abs() < 1e-6);
}

#[test]
#[ignore = "Run mayapy tests/maya_integration.py to generate rendered references first"]
fn matches_maya_software_render_pixels() {
    let base = Path::new("tests/output");
    let cases: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(base.join("maya_render_reference.json")).unwrap(),
    )
    .unwrap();
    for case in cases.as_array().unwrap() {
        let path = base.join(case["abc"].as_str().unwrap());
        let scene = scene::load(&path, scene::default_sample(&path).unwrap()).unwrap();
        let c = &scene.cameras[0];
        let image = image::open(case["image"].as_str().unwrap())
            .unwrap()
            .to_rgb8();
        let (width, height) = image.dimensions();
        for (index, position) in case["positions"].as_array().unwrap().iter().enumerate() {
            let p = position.as_array().unwrap();
            let world = DVec3::new(
                p[0].as_f64().unwrap(),
                p[1].as_f64().unwrap(),
                p[2].as_f64().unwrap(),
            );
            let ndc = (c.projection * c.view).project_point3(world);
            let expected_x = (ndc.x + 1.) * 0.5 * width as f64 - 0.5;
            let expected_y = (1. - ndc.y) * 0.5 * height as f64 - 0.5;
            let mut sum_x = 0.;
            let mut sum_y = 0.;
            let mut count = 0.;
            for (x, y, pixel) in image.enumerate_pixels() {
                if pixel[index] > 180 && pixel[(index + 1) % 3] < 50 && pixel[(index + 2) % 3] < 50
                {
                    sum_x += x as f64;
                    sum_y += y as f64;
                    count += 1.;
                }
            }
            assert!(count > 4., "Missing marker {index} in {path:?}");
            let error = ((sum_x / count - expected_x).powi(2)
                + (sum_y / count - expected_y).powi(2))
            .sqrt();
            println!(
                "{} marker {}: {:.3} pixel error",
                path.display(),
                index,
                error
            );
            assert!(
                error < 1.5,
                "Camera framing differs from Maya Software by {error:.3}px ({path:?}, marker {index})"
            );
        }
    }
}
