use crate::{
    abc::{self, Node},
    camera::{self, Camera, CameraFile},
    light_paint::PickMesh,
};
use glam::{DVec3, Vec3};
use std::path::{Path, PathBuf};

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
}

pub struct Scene {
    pub path: PathBuf,
    pub nodes: Vec<Node>,
    pub vertices: Vec<Vertex>,
    pub pick_mesh: PickMesh,
    pub cameras: Vec<Camera>,
    pub warnings: Vec<String>,
    pub meshes: usize,
}

pub fn default_sample(path: &Path) -> Result<f64, String> {
    let metadata = path.with_extension("camera.json");
    if !metadata.exists() {
        return Ok(0.0);
    }
    let file: CameraFile =
        serde_json::from_str(&std::fs::read_to_string(metadata).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    Ok(file.sample_seconds)
}

pub fn load(path: &Path, seconds: f64) -> Result<Scene, String> {
    let nodes = abc::load(path, seconds)?;
    let mut scene = Scene {
        path: path.into(),
        nodes,
        vertices: vec![],
        pick_mesh: PickMesh::default(),
        cameras: vec![],
        warnings: vec![],
        meshes: 0,
    };
    let metadata_path = path.with_extension("camera.json");
    if metadata_path.exists() {
        let data = std::fs::read_to_string(&metadata_path).map_err(|e| e.to_string())?;
        let file: CameraFile =
            serde_json::from_str(&data).map_err(|e| format!("Camera JSON: {e}"))?;
        let metadata = std::fs::metadata(path).map_err(|e| e.to_string())?;
        let modified = metadata
            .modified()
            .map_err(|e| e.to_string())?
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        // Python/Windows stat mtime can round submicrosecond digits; allow 1 microsecond.
        if file.version != 1
            || file.abc_size != metadata.len()
            || modified.abs_diff(file.abc_mtime_ns as u128) > 1000
            || (file.sample_seconds - seconds).abs() > 1e-6
        {
            return Err("Camera JSON is stale or for another sample. Re-export the ABC and camera JSON together.".into());
        }
        for c in file.cameras {
            if !scene.nodes.iter().any(|n| n.kind == 3 && n.path == c.path) {
                return Err(format!("Camera JSON path not present in ABC: {}", c.path));
            }
            scene.cameras.push(camera::from_record(&c)?);
        }
    }
    for (index, node) in scene.nodes.iter().enumerate() {
        if node.parent < -1 || (node.parent >= 0 && node.parent as usize >= index) {
            return Err("Invalid scene parent hierarchy".into());
        }
        if node.kind == 3 && !scene.cameras.iter().any(|c| c.path == node.path) {
            match camera::from_abc(node) {
                Ok(c) => scene.cameras.push(c),
                Err(e) => scene.warnings.push(e),
            }
        }
        if node.kind == 4 {
            scene
                .warnings
                .push(format!("Unsupported object: {}", node.path));
        }
        if node.kind != 2 || !node.visible {
            continue;
        }
        scene.meshes += 1;
        append_mesh(node, &mut scene.vertices)?;
    }
    if scene.vertices.is_empty() {
        scene
            .warnings
            .push("No visible polygon meshes in this archive".into());
    }
    if scene.cameras.is_empty() {
        scene
            .warnings
            .push("No usable camera. Export a camera with the mesh.".into());
    }
    scene.pick_mesh = PickMesh::new(&scene.vertices);
    Ok(scene)
}

fn append_mesh(node: &Node, vertices: &mut Vec<Vertex>) -> Result<(), String> {
    let normal_matrix = node.world.inverse().transpose();
    if !normal_matrix.is_finite() {
        return Err(format!("Singular mesh transform: {}", node.path));
    }
    let (positions, _) = node.positions.as_chunks::<3>();
    let point = |index: usize| Vec3::from_array(positions[index]).as_dvec3();
    let mut faces = Vec::with_capacity(node.counts.len());
    // Maya can omit the normal parameter when all edges are smooth.
    let mut smooth = node
        .normals
        .is_empty()
        .then(|| vec![DVec3::ZERO; positions.len()]);
    let mut start = 0usize;
    for &count in &node.counts {
        let count = usize::try_from(count).map_err(|_| "Negative face size")?;
        let end = start.checked_add(count).ok_or("Face size overflow")?;
        if count < 3 || end > node.indices.len() {
            return Err("Invalid polygon topology".into());
        }
        let indices = &node.indices[start..end];
        if indices
            .iter()
            .any(|&i| i < 0 || i as usize >= positions.len())
        {
            return Err("Vertex index out of range".into());
        }
        // Newell normal selects a stable 2D plane, so concave n-gons also triangulate correctly.
        let mut normal = DVec3::ZERO;
        for i in 0..count {
            let a = point(indices[i] as usize);
            let b = point(indices[(i + 1) % count] as usize);
            normal += a.cross(b);
        }
        if normal.length_squared() >= 1e-24 {
            if let Some(smooth) = &mut smooth {
                for &index in indices {
                    smooth[index as usize] += normal;
                }
            }
            faces.push((start, indices, normal.normalize()));
        }
        start = end;
    }
    for (start, indices, normal) in faces {
        let axis = normal.abs().max_element();
        let mut flat = Vec::with_capacity(indices.len() * 2);
        for &index in indices {
            let p = point(index as usize);
            let q = if normal.x.abs() == axis {
                [p.y, p.z]
            } else if normal.y.abs() == axis {
                [p.x, p.z]
            } else {
                [p.x, p.y]
            };
            flat.extend(q);
        }
        let triangles =
            earcutr::earcut(&flat, &[], 2).map_err(|e| format!("Triangulation failed: {e:?}"))?;
        for corner in triangles {
            let index = indices[corner] as usize;
            let local_normal = node
                .normals
                .get((start + corner) * 3..(start + corner) * 3 + 3)
                .map(|v| DVec3::new(v[0] as f64, v[1] as f64, v[2] as f64))
                .or_else(|| smooth.as_ref().and_then(|n| n[index].try_normalize()))
                .unwrap_or(normal);
            let world_normal = normal_matrix
                .transform_vector3(local_normal)
                .normalize_or_zero()
                .as_vec3();
            let world = node.world.transform_point3(point(index)).as_vec3();
            if !world.is_finite() || !world_normal.is_finite() {
                return Err("Non-finite transformed mesh".into());
            }
            vertices.push(Vertex {
                position: world.to_array(),
                normal: world_normal.to_array(),
            });
        }
    }
    Ok(())
}

pub fn bounds(vertices: &[Vertex]) -> (Vec3, Vec3) {
    vertices.iter().fold(
        (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
        |(min, max), v| {
            let p = Vec3::from_array(v.position);
            (min.min(p), max.max(p))
        },
    )
}
