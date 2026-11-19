use crate::abc::Node;
use glam::{DMat3, DMat4, DVec3};
use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct Camera {
    pub path: String,
    pub view: DMat4,
    pub projection: DMat4,
    pub aspect: f64,
    pub source: String,
    pub focal: f64,
}

impl Camera {
    pub fn is_orthographic(&self) -> bool {
        self.projection.w_axis.w.abs() > 0.5
    }
}

#[derive(Deserialize, Serialize)]
pub struct CameraFile {
    pub version: u32,
    pub sample_seconds: f64,
    pub abc_size: u64,
    pub abc_mtime_ns: u64,
    pub cameras: Vec<CameraRecord>,
}
#[derive(Deserialize, Serialize)]
pub struct CameraRecord {
    pub path: String,
    pub view: [f64; 16],
    pub projection: [f64; 16],
    pub width: u32,
    pub height: u32,
    pub pixel_aspect: f64,
    pub focal_mm: f64,
}

// OpenGL right-handed projection: camera looks along -Z, depth is [-1, 1].
pub fn frustum(l: f64, r: f64, b: f64, t: f64, n: f64, f: f64) -> DMat4 {
    DMat4::from_cols_array(&[
        2.0 * n / (r - l),
        0.0,
        0.0,
        0.0,
        0.0,
        2.0 * n / (t - b),
        0.0,
        0.0,
        (r + l) / (r - l),
        (t + b) / (t - b),
        -(f + n) / (f - n),
        -1.0,
        0.0,
        0.0,
        -2.0 * f * n / (f - n),
        0.0,
    ])
}

pub fn from_abc(node: &Node) -> Result<Camera, String> {
    let c = node.camera_core;
    let (focal, h, v, squeeze, near, far) = (c[0], c[1], c[3], c[5], c[14], c[15]);
    if !c.iter().all(|v| v.is_finite())
        || focal <= 0.0
        || h <= 0.0
        || v <= 0.0
        || squeeze <= 0.0
        || near <= 0.0
        || far <= near
    {
        return Err(format!("Invalid camera parameters: {}", node.path));
    }
    if node.world.determinant().abs() < 1e-15 {
        return Err("Singular camera transform".into());
    }
    // Match Alembic's normalized screen-window convention, including film-back ops.
    // This is the archive's film gate; render resolution/film fit are not guessed.
    let aperture_y = squeeze * v / h;
    let offset = glam::DVec2::new(2.0 * c[2] * squeeze / h, 2.0 * c[4] / h);
    let film = DMat3::from_cols_array(&node.film_back);
    let lt = film * DVec3::new(-(1.0 + c[6]) * squeeze, (1.0 + c[8]) * aperture_y, 1.0);
    let rb = film * DVec3::new((1.0 + c[7]) * squeeze, -(1.0 + c[9]) * aperture_y, 1.0);
    // Non-axis-aligned film ops require a full projection from the Maya sidecar.
    if film.x_axis.y.abs() > 1e-10 || film.y_axis.x.abs() > 1e-10 {
        return Err(format!(
            "Film rotation/shear requires Maya camera JSON: {}",
            node.path
        ));
    }
    let scale = h * 10.0 / (2.0 * focal) * near;
    let (l, r, b, t) = (
        (lt.x + offset.x) * scale,
        (rb.x + offset.x) * scale,
        (rb.y + offset.y) * scale,
        (lt.y + offset.y) * scale,
    );
    if r <= l || t <= b {
        return Err("Invalid camera screen window".into());
    }
    Ok(Camera {
        path: node.path.clone(),
        view: node.world.inverse(),
        projection: frustum(l, r, b, t, near, far),
        aspect: (r - l) / (t - b),
        source: "ABC film gate — render framing unverified; export Maya camera JSON".into(),
        focal,
    })
}

pub fn from_record(c: &CameraRecord) -> Result<Camera, String> {
    let view = DMat4::from_cols_array(&c.view);
    let projection = DMat4::from_cols_array(&c.projection);
    if c.width == 0
        || c.height == 0
        || !c.pixel_aspect.is_finite()
        || c.pixel_aspect <= 0.0
        || !view.is_finite()
        || !projection.is_finite()
        || view.determinant().abs() < 1e-15
        || projection.determinant().abs() < 1e-15
    {
        return Err(format!("Invalid Maya projection: {}", c.path));
    }
    Ok(Camera {
        path: c.path.clone(),
        view,
        projection,
        aspect: c.width as f64 * c.pixel_aspect / c.height as f64,
        source: format!(
            "Maya render gate · {} × {} · pixel aspect {}",
            c.width, c.height, c.pixel_aspect
        ),
        focal: c.focal_mm,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn asymmetric_frustum_maps_edges_and_depth() {
        let p = frustum(-0.08, 0.12, -0.05, 0.05, 0.1, 1000.0);
        for (point, expected) in [
            (DVec3::new(-0.08, -0.05, -0.1), DVec3::new(-1., -1., -1.)),
            (DVec3::new(0.12, 0.05, -0.1), DVec3::new(1., 1., -1.)),
        ] {
            assert!((p.project_point3(point) - expected).length() < 1e-10);
        }
        assert!((p.project_point3(DVec3::new(0., 0., -1000.)).z - 1.).abs() < 1e-10);
    }
}
