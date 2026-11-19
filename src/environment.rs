use glam::Vec3;
use image::{Rgb, codecs::hdr::HdrEncoder};
use std::{
    f32::consts::{PI, TAU},
    io::Write,
    path::Path,
    sync::Arc,
};

pub const PREVIEW_WIDTH: u32 = 512;
pub const PREVIEW_HEIGHT: u32 = 256;
pub const SHADOW_SAMPLE_COUNT: usize = 64;

pub struct ShadowSamples {
    pub directions: [[f32; 3]; SHADOW_SAMPLE_COUNT],
    pub weights: [[f32; 3]; SHADOW_SAMPLE_COUNT],
}

#[derive(Clone, Debug)]
pub struct HdrImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[f32; 3]>,
}

impl HdrImage {
    pub fn load(path: &Path) -> Result<Self, String> {
        let extension = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();
        if !matches!(extension.as_str(), "hdr" | "exr") {
            return Err("Choose a linear .hdr or .exr equirectangular environment".into());
        }
        let reader = image::ImageReader::open(path)
            .map_err(|e| e.to_string())?
            .with_guessed_format()
            .map_err(|e| e.to_string())?;
        let image = reader
            .decode()
            .map_err(|e| format!("HDR decode: {e}"))?
            .to_rgb32f();
        let (width, height) = image.dimensions();
        if width < 2 || height < 1 || width != height * 2 {
            return Err("The HDR environment must be a 2:1 equirectangular panorama".into());
        }
        let pixels: Vec<[f32; 3]> = image.pixels().map(|p| p.0).collect();
        if pixels
            .iter()
            .flatten()
            .any(|v| !v.is_finite() || v.abs() > 1.0e20)
        {
            return Err("HDR contains non-finite or unsupported extreme radiance values".into());
        }
        // Negative EXR RGB values are not usable as emitted radiance.
        let pixels = pixels.into_iter().map(|p| p.map(|v| v.max(0.0))).collect();
        Ok(Self {
            width,
            height,
            pixels,
        })
    }

    pub fn sample(&self, u: f32, v: f32) -> Vec3 {
        self.sample_color(u, v, true)
    }

    fn sample_color(&self, u: f32, v: f32, linear: bool) -> Vec3 {
        let x = u.rem_euclid(1.0) * self.width as f32 - 0.5;
        let y = (v * self.height as f32 - 0.5).clamp(0.0, self.height as f32 - 1.0);
        let x0 = x.floor() as i32;
        let y0 = y.floor() as u32;
        let tx = x - x.floor();
        let ty = y - y.floor();
        let pixel = |x: i32, y: u32| {
            let rgb =
                self.pixels[(y * self.width + x.rem_euclid(self.width as i32) as u32) as usize];
            Vec3::from_array(if linear {
                rgb
            } else {
                rgb.map(|v| {
                    if v <= 0.04045 {
                        v / 12.92
                    } else {
                        ((v + 0.055) / 1.055).powf(2.4)
                    }
                })
            })
        };
        pixel(x0, y0).lerp(pixel(x0 + 1, y0), tx).lerp(
            pixel(x0, (y0 + 1).min(self.height - 1))
                .lerp(pixel(x0 + 1, (y0 + 1).min(self.height - 1)), tx),
            ty,
        )
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if self
            .pixels
            .iter()
            .flatten()
            .any(|v| !v.is_finite() || *v < 0.0)
        {
            return Err("Cannot export invalid HDR radiance".into());
        }
        let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
        let data: Vec<Rgb<f32>> = self.pixels.iter().map(|p| Rgb(*p)).collect();
        let mut writer = std::io::BufWriter::new(file);
        HdrEncoder::new(&mut writer)
            .encode(&data, self.width as usize, self.height as usize)
            .map_err(|e| e.to_string())?;
        writer.flush().map_err(|e| e.to_string())
    }

    pub fn display_pixels(&self) -> Vec<eframe::egui::Color32> {
        self.pixels
            .iter()
            .map(|p| {
                let c = p.map(|v| {
                    let linear = v / (1.0 + v);
                    let srgb = if linear <= 0.0031308 {
                        12.92 * linear
                    } else {
                        1.055 * linear.powf(1.0 / 2.4) - 0.055
                    };
                    (srgb.clamp(0.0, 1.0) * 255.0).round() as u8
                });
                eframe::egui::Color32::from_rgb(c[0], c[1], c[2])
            })
            .collect()
    }

    /// Lambertian irradiance / PI in real SH order. Constant radiance stays constant.
    pub fn diffuse_sh(&self) -> [[f32; 3]; 9] {
        let mut coefficients = [Vec3::ZERO; 9];
        let delta_phi = TAU / self.width as f32;
        let delta_theta = PI / self.height as f32;
        for y in 0..self.height {
            let theta = (y as f32 + 0.5) * delta_theta;
            let weight = delta_phi * delta_theta * theta.sin();
            for x in 0..self.width {
                let phi = ((x as f32 + 0.5) / self.width as f32 - 0.5) * TAU;
                let direction = Vec3::new(
                    theta.sin() * phi.cos(),
                    theta.cos(),
                    theta.sin() * phi.sin(),
                );
                let rgb = Vec3::from_array(self.pixels[(y * self.width + x) as usize]);
                for (c, b) in coefficients.iter_mut().zip(sh_basis(direction)) {
                    *c += rgb * (b * weight);
                }
            }
        }
        std::array::from_fn(|i| {
            (coefficients[i]
                * if i == 0 {
                    1.0
                } else if i < 4 {
                    2.0 / 3.0
                } else {
                    0.25
                })
            .to_array()
        })
    }

    pub fn shadow_samples(&self) -> Option<ShadowSamples> {
        let width = self.width as usize;
        let height = self.height as usize;
        let mut rows = Vec::with_capacity(height);
        let mut columns = Vec::with_capacity(self.pixels.len());
        let mut sum = 0.0_f64;
        let luma = Vec3::new(0.2126, 0.7152, 0.0722);
        for (y, row) in self.pixels.chunks_exact(width).enumerate() {
            let solid_angle = ((y as f64 + 0.5) * std::f64::consts::PI / height as f64).sin();
            for &pixel in row {
                sum += f64::from(Vec3::from_array(pixel).dot(luma)) * solid_angle;
                columns.push(sum);
            }
            rows.push(sum);
        }
        if sum <= 0.0 {
            return None;
        }

        // Importance sample the rows and columns independently. Weight each
        // selected color by the inverse of the luminance used to select it.
        let mut samples = ShadowSamples {
            directions: [[0.0; 3]; SHADOW_SAMPLE_COUNT],
            weights: [[0.0; 3]; SHADOW_SAMPLE_COUNT],
        };
        for i in 0..SHADOW_SAMPLE_COUNT {
            let row_target = (i as f64 + 0.5) / SHADOW_SAMPLE_COUNT as f64 * sum;
            let y = rows.partition_point(|&bound| bound <= row_target);
            let start = if y == 0 { 0.0 } else { rows[y - 1] };
            let fraction = f64::from((i as u32).reverse_bits()) / (1u64 << 32) as f64
                + 0.5 / SHADOW_SAMPLE_COUNT as f64;
            let column_target = start + fraction * (rows[y] - start);
            let row = &columns[y * width..][..width];
            let x = row
                .partition_point(|&bound| bound <= column_target)
                .min(width - 1);
            let rgb = Vec3::from_array(self.pixels[y * width + x]);
            samples.directions[i] = panorama_direction(
                (x as f32 + 0.5) / self.width as f32,
                (y as f32 + 0.5) / self.height as f32,
            )
            .to_array();
            samples.weights[i] =
                (rgb / Vec3::splat(rgb.dot(luma).max(f32::MIN_POSITIVE))).to_array();
        }
        Some(samples)
    }
}

pub fn sh_basis(n: Vec3) -> [f32; 9] {
    [
        0.282095,
        0.488603 * n.y,
        0.488603 * n.z,
        0.488603 * n.x,
        1.092548 * n.x * n.y,
        1.092548 * n.y * n.z,
        0.315392 * (3.0 * n.z * n.z - 1.0),
        1.092548 * n.x * n.z,
        0.546274 * (n.x * n.x - n.y * n.y),
    ]
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BlendMode {
    Add,
    Multiply,
    Amplify,
    Over,
    LowPass,
}

impl BlendMode {
    pub const ALL: [Self; 5] = [
        Self::Add,
        Self::Multiply,
        Self::Amplify,
        Self::Over,
        Self::LowPass,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Add => "Add",
            Self::Multiply => "Multiply",
            Self::Amplify => "Amplify",
            Self::Over => "Over",
            Self::LowPass => "Low Pass",
        }
    }

    pub fn supports_invert(self) -> bool {
        matches!(self, Self::Add | Self::Amplify | Self::LowPass)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LightShape {
    Rectangle,
    Round,
    Gradient,
    Image,
    CompositeLight,
}

impl LightShape {
    pub const ALL: [Self; 5] = [
        Self::Round,
        Self::Rectangle,
        Self::Gradient,
        Self::Image,
        Self::CompositeLight,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Round => "Round",
            Self::Rectangle => "Rectangle",
            Self::Gradient => "Gradient",
            Self::Image => "Image",
            Self::CompositeLight => "CompositeLight",
        }
    }

    pub fn is_positioned(self) -> bool {
        matches!(self, Self::Rectangle | Self::Round | Self::Gradient)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LightMapping {
    Spherical,
    Flat,
}

#[derive(Clone, Copy)]
pub enum Projection {
    Flat {
        center: glam::Vec2,
        half_size: glam::Vec2,
    },
    Spherical {
        forward: Vec3,
        right: Vec3,
        down: Vec3,
        half_size: glam::Vec2,
    },
}

fn panorama_angles(u: f32, v: f32) -> (f32, f32) {
    ((u - 0.5) * TAU, (0.5 - v) * PI)
}

fn panorama_direction(u: f32, v: f32) -> Vec3 {
    let (longitude, latitude) = panorama_angles(u, v);
    let (sin_lon, cos_lon) = longitude.sin_cos();
    let (sin_lat, cos_lat) = latitude.sin_cos();
    Vec3::new(cos_lon * cos_lat, sin_lat, sin_lon * cos_lat)
}

pub fn panorama_uv(direction: Vec3) -> glam::Vec2 {
    let horizontal = direction.x.hypot(direction.z);
    let longitude = if horizontal < 1e-7 {
        0.0
    } else {
        direction.z.atan2(direction.x)
    };
    let latitude = direction.y.atan2(horizontal);
    glam::Vec2::new((0.5 + longitude / TAU).rem_euclid(1.0), 0.5 - latitude / PI)
}

pub fn panorama_rotation(from: glam::Vec2, to: glam::Vec2) -> glam::Quat {
    let (from_lon, from_lat) = panorama_angles(from.x, from.y);
    let (to_lon, to_lat) = panorama_angles(to.x, to.y);
    if from.y == to.y {
        return glam::Quat::from_rotation_y(from_lon - to_lon);
    }
    // Tilt along the grabbed point's meridian, then turn to the new longitude.
    glam::Quat::from_rotation_y(-to_lon)
        * glam::Quat::from_rotation_z(to_lat - from_lat)
        * glam::Quat::from_rotation_y(from_lon)
}

impl Projection {
    fn local(self, uv: glam::Vec2, direction: Vec3) -> Option<glam::Vec2> {
        match self {
            Self::Flat { center, half_size } => {
                let delta = uv - center;
                Some(glam::Vec2::new((delta.x + 0.5).rem_euclid(1.0) - 0.5, delta.y) / half_size)
            }
            Self::Spherical {
                forward,
                right,
                down,
                half_size,
            } => {
                let depth = direction.dot(forward);
                if depth <= 0.0 {
                    return None;
                }
                let tangent = glam::Vec2::new(direction.dot(right), direction.dot(down));
                Some(tangent / (depth * half_size))
            }
        }
    }

    pub fn panorama(self, local: glam::Vec2) -> glam::Vec2 {
        match self {
            Self::Flat { center, half_size } => center + local * half_size,
            Self::Spherical {
                forward,
                right,
                down,
                half_size,
            } => {
                let local = local * half_size;
                panorama_uv(forward + right * local.x + down * local.y)
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FalloffPreset {
    Softbox,
    Smooth,
    Linear,
    Tight,
    HardEdge,
}

impl FalloffPreset {
    pub const ALL: [Self; 5] = [
        Self::Softbox,
        Self::Smooth,
        Self::Linear,
        Self::Tight,
        Self::HardEdge,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Softbox => "Softbox",
            Self::Smooth => "Smooth",
            Self::Linear => "Linear",
            Self::Tight => "Tight",
            Self::HardEdge => "Hard edge",
        }
    }

    fn points(self) -> &'static [[f32; 2]] {
        match self {
            Self::Softbox => &[[0., 1.], [0.75, 1.], [0.875, 0.5], [1., 0.]],
            Self::Smooth => &[[0., 1.], [0.25, 0.84], [0.75, 0.16], [1., 0.]],
            Self::Linear => &[[0., 1.], [1., 0.]],
            Self::Tight => &[[0., 1.], [0.18, 0.65], [0.50, 0.12], [0.80, 0.], [1., 0.]],
            Self::HardEdge => &[[0., 1.], [1., 1.]],
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct FalloffCurve {
    pub points: Vec<[f32; 2]>,
    pub preset: Option<FalloffPreset>,
    pub box_interpolation: BoxInterpolation,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BoxInterpolation {
    Linear,
    Smooth,
}

impl FalloffCurve {
    pub fn from_preset(preset: FalloffPreset) -> Self {
        Self {
            points: preset.points().to_vec(),
            preset: Some(preset),
            box_interpolation: if preset == FalloffPreset::Linear {
                BoxInterpolation::Linear
            } else {
                BoxInterpolation::Smooth
            },
        }
    }

    pub fn sample(&self, distance: f32, box_ramp: bool) -> f32 {
        let position = distance.clamp(0.0, 1.0);
        let i = self
            .points
            .partition_point(|point| point[0] <= position)
            .saturating_sub(1)
            .min(self.points.len() - 2);
        let [x0, y0] = self.points[i];
        let [x1, y1] = self.points[i + 1];
        let span = x1 - x0;
        let t = (position - x0) / span;
        if box_ramp && self.box_interpolation == BoxInterpolation::Linear {
            return y0 + (y1 - y0) * t;
        }
        let slope = |index: usize| {
            let [x0, y0] = self.points[index];
            let [x1, y1] = self.points[index + 1];
            (y1 - y0) / (x1 - x0)
        };
        let tangent = |index: usize| {
            // Ease in at the mirrored core and out at the edge. Interior
            // anchors on the same ramp share a nonzero tangent.
            if index == 0 {
                return if box_ramp { 0.0 } else { slope(0) };
            }
            if index == self.points.len() - 1 {
                return if box_ramp { 0.0 } else { slope(index - 1) };
            }
            let left = slope(index - 1);
            let right = slope(index);
            if left * right <= 0.0 {
                return 0.0;
            }
            let left_span = self.points[index][0] - self.points[index - 1][0];
            let right_span = self.points[index + 1][0] - self.points[index][0];
            let w1 = 2.0 * right_span + left_span;
            let w2 = right_span + 2.0 * left_span;
            (w1 + w2) / (w1 / left + w2 / right)
        };
        let t_sq = t * t;
        let t_cube = t_sq * t;
        let m0 = tangent(i) * span;
        let m1 = tangent(i + 1) * span;
        ((2.0 * t_cube - 3.0 * t_sq + 1.0) * y0
            + (t_cube - 2.0 * t_sq + t) * m0
            + (-2.0 * t_cube + 3.0 * t_sq) * y1
            + (t_cube - t_sq) * m1)
            .clamp(y0.min(y1), y0.max(y1))
    }

    fn bake(&self, box_ramp: bool) -> FalloffTable {
        FalloffTable {
            samples: std::array::from_fn(|i| self.sample(i as f32 / 511.0, box_ramp)),
            vertical: None,
            gradient_axis: glam::Vec2::ZERO,
        }
    }
}

struct FalloffTable {
    samples: [f32; 512],
    vertical: Option<Box<[f32; 512]>>,
    gradient_axis: glam::Vec2,
}

impl FalloffTable {
    fn sample(&self, distance: f32) -> f32 {
        Self::sample_ramp(&self.samples, distance)
    }

    fn sample_vertical(&self, distance: f32) -> f32 {
        Self::sample_ramp(self.vertical.as_deref().unwrap_or(&self.samples), distance)
    }

    fn sample_ramp(samples: &[f32; 512], distance: f32) -> f32 {
        let index = distance.clamp(0.0, 1.0) * 511.0;
        let left = index as usize;
        let right = (left + 1).min(511);
        samples[left] + (samples[right] - samples[left]) * (index - left as f32)
    }
}

#[derive(Clone)]
pub struct ImageLayer {
    pub image: Option<Arc<HdrImage>>,
    pub source: String,
    pub rotation: f32,
    pub exposure: f32,
    pub alpha: f32,
    pub linear: bool,
    pub blend: BlendMode,
}

impl Default for ImageLayer {
    fn default() -> Self {
        Self {
            image: None,
            source: String::new(),
            rotation: 0.0,
            exposure: 0.0,
            alpha: 1.0,
            linear: true,
            blend: BlendMode::Over,
        }
    }
}

impl PartialEq for ImageLayer {
    fn eq(&self, other: &Self) -> bool {
        let same_image = match (&self.image, &other.image) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        same_image
            && self.source == other.source
            && self.rotation == other.rotation
            && self.exposure == other.exposure
            && self.alpha == other.alpha
            && self.linear == other.linear
            && self.blend == other.blend
    }
}

#[derive(Clone, PartialEq)]
pub struct Light {
    pub id: u32,
    pub name: String,
    pub enabled: bool,
    pub locked: bool,
    pub u: f32,
    pub v: f32,
    pub width_degrees: f32,
    pub height_degrees: f32,
    pub falloff: FalloffCurve,
    pub box_vertical: Option<FalloffCurve>,
    pub box_link_axes: bool,
    pub gradient_curve: FalloffCurve,
    pub gradient_rotation: f32,
    pub intensity: f32,
    pub color: [f32; 3],
    pub blend: BlendMode,
    pub invert: bool,
    pub multiply_mask: bool,
    pub shape: LightShape,
    pub image_layer: ImageLayer,
    pub composite: Option<Box<Document>>,
    composite_rotation: glam::Quat,
    pub mapping: LightMapping,
    pub core: [f32; 2],
    pub cut_enabled: bool,
    pub cut_rotation: f32,
    pub cut_amount: f32,
}
impl Light {
    pub fn blend_mode(&mut self) -> &mut BlendMode {
        if self.shape == LightShape::Image {
            &mut self.image_layer.blend
        } else {
            &mut self.blend
        }
    }

    pub fn position(&self) -> glam::Vec2 {
        if self.shape == LightShape::CompositeLight {
            panorama_uv(self.composite_rotation.conjugate() * Vec3::X)
        } else {
            glam::Vec2::new(self.u, self.v)
        }
    }

    pub fn set_position(&mut self, position: glam::Vec2) {
        if self.shape == LightShape::CompositeLight {
            let rotation = panorama_rotation(self.position(), position);
            self.set_composite_local_rotation(self.composite_rotation * rotation.conjugate());
        } else {
            self.u = position.x;
            self.v = position.y;
        }
    }

    pub fn composite_local_rotation(&self) -> glam::Quat {
        self.composite_rotation
    }

    pub fn set_composite_local_rotation(&mut self, rotation: glam::Quat) {
        self.composite_rotation = rotation.normalize();
    }

    pub fn can_move(&self) -> bool {
        self.enabled && !self.locked && self.shape != LightShape::Image
    }

    fn tangent_half_size(&self) -> glam::Vec2 {
        // The tangent plane approaches infinity at a 180-degree field of view.
        glam::Vec2::new(
            (0.5 * self.width_degrees.min(179.0)).to_radians().tan(),
            (0.5 * self.height_degrees).to_radians().tan(),
        )
    }

    pub fn projection(&self) -> Projection {
        if self.mapping == LightMapping::Flat {
            return Projection::Flat {
                center: glam::Vec2::new(self.u, self.v),
                half_size: glam::Vec2::new(self.width_degrees / 720.0, self.height_degrees / 360.0),
            };
        }
        let (longitude, latitude) = panorama_angles(self.u, self.v);
        let (sin_lon, cos_lon) = longitude.sin_cos();
        let (sin_lat, cos_lat) = latitude.sin_cos();
        Projection::Spherical {
            forward: Vec3::new(cos_lon * cos_lat, sin_lat, sin_lon * cos_lat),
            right: Vec3::new(-sin_lon, 0.0, cos_lon),
            down: Vec3::new(cos_lon * sin_lat, -cos_lat, sin_lon * sin_lat),
            half_size: self.tangent_half_size(),
        }
    }

    fn scaled_direction(&self, angle: f32) -> glam::Vec2 {
        let scale = match self.mapping {
            LightMapping::Flat => glam::Vec2::new(self.width_degrees, self.height_degrees),
            LightMapping::Spherical => self.tangent_half_size(),
        };
        glam::Vec2::from_angle(angle.to_radians()) * scale
    }

    fn cut_plane(&self) -> Option<(glam::Vec2, f32)> {
        if !self.cut_enabled || self.cut_amount <= 0.0 {
            return None;
        }
        let normal = self.scaled_direction(self.cut_rotation).normalize();
        let extent = match self.shape {
            LightShape::Rectangle | LightShape::Gradient => normal.abs().element_sum(),
            LightShape::Round => 1.0,
            LightShape::Image | LightShape::CompositeLight => return None,
        };
        Some((normal, extent * (1.0 - 2.0 * self.cut_amount)))
    }

    pub fn cut_segment(&self) -> Option<[glam::Vec2; 2]> {
        let (normal, distance) = self.cut_plane()?;
        let center = normal * distance;
        let tangent = normal.perp();
        if self.shape == LightShape::Round {
            let half = (1.0 - distance * distance).max(0.0).sqrt();
            return Some([center - tangent * half, center + tangent * half]);
        }
        let mut start = f32::NEG_INFINITY;
        let mut end = f32::INFINITY;
        for axis in 0..2 {
            if tangent[axis].abs() < f32::EPSILON {
                continue;
            }
            let a = (-1.0 - center[axis]) / tangent[axis];
            let b = (1.0 - center[axis]) / tangent[axis];
            start = start.max(a.min(b));
            end = end.min(a.max(b));
        }
        (start <= end).then(|| [center + tangent * start, center + tangent * end])
    }

    fn covers(&self, p: glam::Vec2, cut: Option<(glam::Vec2, f32)>) -> bool {
        let inside = match self.shape {
            LightShape::Rectangle | LightShape::Gradient => p.abs().max_element() < 1.0,
            LightShape::Round => p.length_squared() < 1.0,
            LightShape::Image | LightShape::CompositeLight => false,
        };
        inside && cut.is_none_or(|(normal, distance)| normal.dot(p) <= distance)
    }

    pub fn hit(&self, u: f32, v: f32) -> bool {
        self.enabled && self.hit_visible(u, v)
    }

    pub fn hit_visible(&self, u: f32, v: f32) -> bool {
        if self.locked || self.shape == LightShape::Image {
            return false;
        }
        let uv = glam::Vec2::new(u, v);
        let direction = panorama_direction(u, v);
        if self.shape == LightShape::CompositeLight {
            let Some(content) = self.composite.as_deref() else {
                return false;
            };
            let transform = CompositeTransform::new(self);
            let (uv, direction) = transform.to_local(uv, direction);
            return PreparedDocument::new(content).sample(uv, direction).alpha > 0.01;
        }
        self.projection()
            .local(uv, direction)
            .is_some_and(|position| self.covers(position, self.cut_plane()))
    }

    fn profile(&self, p: glam::Vec2, falloff: &FalloffTable) -> f32 {
        let core = glam::Vec2::from_array(self.core);
        let offset = p - core;
        match self.shape {
            LightShape::Rectangle => {
                let distance = offset.abs() / (glam::Vec2::ONE - core * offset.signum());
                falloff.sample(distance.x) * falloff.sample_vertical(distance.y)
            }
            LightShape::Round => {
                // Distance along the ray from the shifted core to the ellipse boundary.
                let a = offset.length_squared();
                let b = core.dot(offset);
                let c = 1.0 - core.length_squared();
                falloff.sample((b + (b * b + a * c).sqrt()) / c)
            }
            LightShape::Gradient => falloff.sample(0.5 + p.dot(falloff.gradient_axis)),
            LightShape::Image | LightShape::CompositeLight => 0.0,
        }
    }

    fn bake_falloff(&self) -> FalloffTable {
        match self.shape {
            LightShape::Gradient => {
                let axis = self.scaled_direction(self.gradient_rotation);
                let mut falloff = self.gradient_curve.bake(false);
                falloff.gradient_axis = axis * (0.5 / axis.abs().element_sum());
                falloff
            }
            LightShape::Round | LightShape::Image | LightShape::CompositeLight => {
                self.falloff.bake(false)
            }
            LightShape::Rectangle => {
                let mut falloff = self.falloff.bake(true);
                if !self.box_link_axes
                    && let Some(curve) = &self.box_vertical
                {
                    falloff.vertical = Some(Box::new(curve.bake(true).samples));
                }
                falloff
            }
        }
    }
}

impl BlendMode {
    fn apply(self, underneath: Vec3, color: Vec3, profile: f32, invert: bool) -> Vec3 {
        let light = color * profile;
        match self {
            Self::Add if invert => {
                let value = (underneath.max_element() - light.max_element()).max(0.0);
                with_value(underneath, value)
            }
            Self::Add => underneath + light,
            Self::Multiply => underneath * light,
            Self::Amplify if invert => underneath / (Vec3::ONE + light),
            Self::Amplify => underneath * (Vec3::ONE + light),
            Self::Over => underneath.lerp(color, profile),
            Self::LowPass => {
                let base_value = underneath.max_element();
                let limit = color.max_element();
                let target = if invert {
                    base_value.max(limit)
                } else {
                    base_value.min(limit)
                };
                underneath.lerp(with_value(underneath, target), profile)
            }
        }
    }
}

// Scale RGB to the new HSV value while preserving the underlying hue and saturation.
fn with_value(rgb: Vec3, value: f32) -> Vec3 {
    let original = rgb.max_element();
    if value == original {
        rgb
    } else if original > 0.0 {
        (rgb / Vec3::splat(original)) * value
    } else {
        Vec3::splat(value)
    }
}

enum ComposedLayer<'a> {
    Image {
        source: &'a HdrImage,
        light: &'a Light,
        gain: f32,
    },
    Composite {
        light: &'a Light,
        content: PreparedDocument<'a>,
        transform: CompositeTransform,
    },
    Light {
        light: &'a Light,
        projection: Projection,
        cut: Option<(glam::Vec2, f32)>,
        falloff: Box<FalloffTable>,
    },
}

struct CompositeTransform {
    rotation: glam::Quat,
    yaw_offset: Option<f32>,
}

impl CompositeTransform {
    fn new(light: &Light) -> Self {
        let rotation = light.composite_local_rotation();
        let yaw_offset = (rotation.x == 0.0 && rotation.z == 0.0).then(|| light.position().x - 0.5);
        Self {
            rotation,
            yaw_offset,
        }
    }

    fn to_local(&self, mut uv: glam::Vec2, direction: Vec3) -> (glam::Vec2, Vec3) {
        let direction = self.rotation * direction;
        if let Some(yaw_offset) = self.yaw_offset {
            uv.x = (uv.x - yaw_offset).rem_euclid(1.0);
        } else {
            uv = panorama_uv(direction);
        }
        (uv, direction)
    }
}

struct PreparedDocument<'a> {
    layers: Vec<ComposedLayer<'a>>,
    spherical: bool,
    gain: f32,
    rotation: f32,
    rotation_sin: f32,
    rotation_cos: f32,
}

#[derive(Default)]
struct CompositePixel {
    rgb: Vec3,
    alpha: f32,
}

impl CompositePixel {
    fn blend(&mut self, mode: BlendMode, color: Vec3, profile: f32, invert: bool, mask: bool) {
        self.rgb = mode.apply(self.rgb, color, profile, invert);
        if mask {
            self.alpha *= profile;
        } else if matches!(mode, BlendMode::Over) || (mode == BlendMode::Add && !invert) {
            let coverage = profile.clamp(0.0, 1.0);
            self.alpha += (1.0 - self.alpha) * coverage;
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct Document {
    pub exposure: f32,
    pub rotation: f32,
    pub lights: Vec<Light>,
    next_id: u32,
}
impl Default for Document {
    fn default() -> Self {
        Self {
            exposure: 0.0,
            rotation: 0.0,
            lights: vec![],
            next_id: 1,
        }
    }
}
impl Document {
    pub fn falloff_preview(
        &self,
        index: usize,
        width: u32,
        height: u32,
    ) -> Option<Vec<eframe::egui::Color32>> {
        let mut light = self
            .lights
            .get(index)
            .filter(|light| light.enabled && light.shape.is_positioned())?
            .clone();
        let falloff = light.bake_falloff();
        let white_level = if light.shape == LightShape::Gradient {
            falloff.samples.iter().copied().fold(0.0_f32, f32::max)
        } else {
            light.profile(glam::Vec2::from_array(light.core), &falloff)
        };
        light.blend = BlendMode::Add;
        light.invert = false;
        light.color = [1.0; 3];
        light.intensity = 1.0;
        let isolated = Self {
            rotation: self.rotation,
            lights: vec![light],
            ..Self::default()
        };
        let image = isolated.compose(width, height);
        let reference = if white_level > 0.0 {
            white_level
        } else {
            image
                .pixels
                .iter()
                .map(|pixel| pixel[0])
                .fold(0.0_f32, f32::max)
        };
        Some(
            image
                .pixels
                .into_iter()
                .map(|pixel| {
                    let gray = if reference > 0.0 {
                        pixel[0] / reference
                    } else {
                        0.0
                    };
                    eframe::egui::Color32::from_gray((gray.clamp(0.0, 1.0) * 255.0).round() as u8)
                })
                .collect(),
        )
    }

    pub fn add_light(&mut self) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        self.lights.push(Light {
            id,
            name: format!("Round {id}"),
            enabled: true,
            locked: false,
            u: 0.75,
            v: 0.45,
            width_degrees: 40.0,
            height_degrees: 40.0,
            falloff: FalloffCurve::from_preset(FalloffPreset::Smooth),
            box_vertical: None,
            box_link_axes: true,
            gradient_curve: FalloffCurve::from_preset(FalloffPreset::Linear),
            gradient_rotation: 0.0,
            intensity: 10.0,
            color: [1.0; 3],
            blend: BlendMode::Add,
            invert: false,
            multiply_mask: false,
            shape: LightShape::Round,
            image_layer: ImageLayer::default(),
            composite: None,
            composite_rotation: glam::Quat::IDENTITY,
            mapping: LightMapping::Spherical,
            core: [0.0; 2],
            cut_enabled: false,
            cut_rotation: 0.0,
            cut_amount: 0.5,
        });
        self.lights.len() - 1
    }
    pub fn compose(&self, width: u32, height: u32) -> HdrImage {
        self.compose_while(width, height, || true).unwrap()
    }

    pub fn compose_while(
        &self,
        width: u32,
        height: u32,
        running: impl Fn() -> bool,
    ) -> Option<HdrImage> {
        if !running() {
            return None;
        }
        let document = PreparedDocument::new(self);
        let columns: Vec<_> = (0..width)
            .map(|x| {
                let u = ((x as f32 + 0.5) / width as f32 - document.rotation).rem_euclid(1.0);
                let (sin, cos) = if document.spherical {
                    panorama_angles(u, 0.5).0.sin_cos()
                } else {
                    (0.0, 0.0)
                };
                (u, sin, cos)
            })
            .collect();
        let mut pixels = Vec::with_capacity((width * height) as usize);
        for y in 0..height {
            if !running() {
                return None;
            }
            let v = (y as f32 + 0.5) / height as f32;
            let (sin_lat, cos_lat) = if document.spherical {
                panorama_angles(0.5, v).1.sin_cos()
            } else {
                (0.0, 0.0)
            };
            for &(u, sin_lon, cos_lon) in &columns {
                let direction = Vec3::new(cos_lon * cos_lat, sin_lat, sin_lon * cos_lat);
                let pixel = document.sample_layers(glam::Vec2::new(u, v), direction);
                pixels.push(pixel.rgb.to_array());
            }
        }
        Some(HdrImage {
            width,
            height,
            pixels,
        })
    }
}

impl<'a> PreparedDocument<'a> {
    fn new(document: &'a Document) -> Self {
        let layers: Vec<_> = document
            .lights
            .iter()
            .filter(|light| light.enabled)
            .filter_map(|light| match light.shape {
                LightShape::Image => {
                    let image = &light.image_layer;
                    Some(ComposedLayer::Image {
                        source: image.image.as_deref().filter(|_| image.alpha > 0.0)?,
                        light,
                        gain: 2.0_f32.powf(image.exposure),
                    })
                }
                LightShape::CompositeLight => Some(ComposedLayer::Composite {
                    light,
                    content: Self::new(light.composite.as_deref()?),
                    transform: CompositeTransform::new(light),
                }),
                _ => Some(ComposedLayer::Light {
                    light,
                    projection: light.projection(),
                    cut: light.cut_plane(),
                    falloff: Box::new(light.bake_falloff()),
                }),
            })
            .collect();
        let spherical = layers.iter().any(|layer| match layer {
            ComposedLayer::Light { projection, .. } => {
                matches!(projection, Projection::Spherical { .. })
            }
            ComposedLayer::Composite {
                content, transform, ..
            } => content.spherical || transform.yaw_offset.is_none(),
            ComposedLayer::Image { .. } => false,
        });
        let rotation = document.rotation / 360.0;
        let (rotation_sin, rotation_cos) = (rotation * TAU).sin_cos();
        Self {
            layers,
            spherical,
            gain: 2.0_f32.powf(document.exposure),
            rotation,
            rotation_sin,
            rotation_cos,
        }
    }

    fn sample(&self, mut uv: glam::Vec2, direction: Vec3) -> CompositePixel {
        uv.x = (uv.x - self.rotation).rem_euclid(1.0);
        let direction = Vec3::new(
            direction.x * self.rotation_cos + direction.z * self.rotation_sin,
            direction.y,
            direction.z * self.rotation_cos - direction.x * self.rotation_sin,
        );
        self.sample_layers(uv, direction)
    }

    fn sample_layers(&self, uv: glam::Vec2, direction: Vec3) -> CompositePixel {
        let mut pixel = CompositePixel::default();
        for layer in &self.layers {
            match layer {
                ComposedLayer::Image {
                    source,
                    light,
                    gain,
                } => {
                    let image = &light.image_layer;
                    let u = uv.x - image.rotation / 360.0;
                    let sample = if image.linear {
                        source.sample(u, uv.y)
                    } else {
                        source.sample_color(u, uv.y, false)
                    } * *gain;
                    if image.blend == BlendMode::Multiply {
                        pixel.rgb = pixel.rgb.lerp(pixel.rgb * sample, image.alpha);
                        if light.multiply_mask {
                            let coverage = sample.max_element().clamp(0.0, 1.0);
                            pixel.alpha *= 1.0 - image.alpha * (1.0 - coverage);
                        }
                    } else {
                        pixel.blend(image.blend, sample, image.alpha, light.invert, false);
                    }
                }
                ComposedLayer::Composite {
                    light,
                    content,
                    transform,
                } => {
                    let (local_uv, local_direction) = transform.to_local(uv, direction);
                    let source = content.sample(local_uv, local_direction);
                    let mask = light.blend == BlendMode::Multiply && light.multiply_mask;
                    if source.alpha > 0.0 {
                        pixel.blend(
                            light.blend,
                            source.rgb / source.alpha,
                            source.alpha,
                            light.invert,
                            mask,
                        );
                    } else if mask {
                        pixel = CompositePixel::default();
                    }
                }
                ComposedLayer::Light {
                    light,
                    projection,
                    cut,
                    falloff,
                } => {
                    let mask = light.blend == BlendMode::Multiply && light.multiply_mask;
                    if let Some(position) = projection.local(uv, direction)
                        && light.covers(position, *cut)
                    {
                        let color = Vec3::from_array(light.color) * light.intensity;
                        pixel.blend(
                            light.blend,
                            color,
                            light.profile(position, falloff),
                            light.invert,
                            mask,
                        );
                    } else if mask {
                        pixel = CompositePixel::default();
                    }
                }
            }
            // Keep stacked gains within representable RGBE range.
            pixel.rgb = pixel.rgb.min(Vec3::splat(1.0e30));
        }
        pixel.rgb *= self.gain;
        pixel
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moving_a_composite_rotates_its_content_and_hit_region_together() {
        let mut parent = Document::default();
        let composite = parent.add_light();
        let mut content = Document::default();
        let child = content.add_light();
        content.lights[child].u = 0.5;
        content.lights[child].v = 0.5;
        let light = &mut parent.lights[composite];
        light.shape = LightShape::CompositeLight;
        light.composite = Some(Box::new(content));
        let position = glam::Vec2::new(0.75, 0.3);
        light.set_position(position);
        assert!(light.hit(position.x, position.y));
        assert!(!light.hit(0.5, 0.5));
        let moved = parent.compose(10, 5);
        assert!(moved.pixels[17][0] > 9.9);
        parent.lights[composite].locked = true;
        assert!(!parent.lights[composite].hit(position.x, position.y));
        assert_eq!(parent.compose(10, 5).pixels, moved.pixels);
    }

    #[test]
    fn multiply_mask_blacks_out_uncovered_pixels_but_only_in_multiply() {
        let mut doc = Document::default();
        let background = doc.add_light();
        let light = &mut doc.lights[background];
        light.shape = LightShape::Image;
        light.image_layer.image = Some(Arc::new(HdrImage {
            width: 2,
            height: 1,
            pixels: vec![[3.0; 3]; 2],
        }));
        let mask = doc.add_light();
        let light = &mut doc.lights[mask];
        light.u = 0.5;
        light.v = 0.5;
        light.intensity = 0.5;
        light.blend = BlendMode::Multiply;

        let unmasked = doc.compose(9, 5);
        assert_eq!(unmasked.pixels[18], [3.0; 3]);
        assert_eq!(unmasked.pixels[22], [1.5; 3]);
        doc.lights[mask].multiply_mask = true;
        let masked = doc.compose(9, 5);
        assert_eq!(masked.pixels[18], [0.0; 3]);
        assert_eq!(masked.pixels[22], unmasked.pixels[22]);

        doc.lights[mask].blend = BlendMode::Add;
        assert_eq!(doc.compose(9, 5).pixels[18], [3.0; 3]);
        doc.lights[mask].blend = BlendMode::Multiply;
        doc.lights[mask].enabled = false;
        assert_eq!(doc.compose(9, 5).pixels, vec![[3.0; 3]; 45]);
    }

    #[test]
    fn composite_children_are_isolated_and_can_mask_the_parent_stack() {
        let mut doc = Document::default();
        let background = doc.add_light();
        let light = &mut doc.lights[background];
        light.shape = LightShape::Image;
        light.image_layer.image = Some(Arc::new(HdrImage {
            width: 2,
            height: 1,
            pixels: vec![[4.0; 3]; 2],
        }));
        let group = doc.add_light();
        let mut child = Document {
            rotation: 144.0,
            ..Document::default()
        };
        let inner = child.add_light();
        let light = &mut child.lights[inner];
        light.u = 0.5;
        light.v = 0.5;
        light.intensity = 0.5;
        light.blend = BlendMode::Multiply;

        doc.lights[group].shape = LightShape::CompositeLight;
        doc.lights[group].composite = Some(Box::new(child));
        assert_eq!(doc.compose(5, 3).pixels, vec![[4.0; 3]; 15]);
        doc.lights[group].composite.as_mut().unwrap().lights[inner].blend = BlendMode::Add;
        let added = doc.compose(5, 3);
        assert_eq!(added.pixels[7], [4.0; 3]);
        assert_eq!(added.pixels[9], [4.5; 3]);

        doc.lights[group].blend = BlendMode::Multiply;
        assert_eq!(doc.compose(5, 3).pixels[7], [4.0; 3]);
        doc.lights[group].multiply_mask = true;
        let masked = doc.compose(5, 3);
        assert_eq!(masked.pixels[7], [0.0; 3]);
        assert!(Vec3::from_array(masked.pixels[9]).abs_diff_eq(Vec3::splat(2.0), 1e-5));
    }

    #[test]
    fn image_layers_rotate_and_composite_with_independent_color_settings() {
        let mut doc = Document::default();
        let lower = doc.add_light();
        let upper = doc.add_light();
        for index in [lower, upper] {
            doc.lights[index].shape = LightShape::Image;
        }
        doc.lights[lower].image_layer.image = Some(Arc::new(HdrImage {
            width: 4,
            height: 2,
            pixels: vec![[1.0; 3]; 8],
        }));
        let image = &mut doc.lights[upper].image_layer;
        assert!(image.linear);
        image.image = Some(Arc::new(HdrImage {
            width: 4,
            height: 2,
            pixels: [[0.5; 3], [0.0; 3], [2.0; 3], [0.0; 3]].repeat(2),
        }));
        image.rotation = 180.0;
        image.exposure = 1.0;
        image.alpha = 0.5;
        assert_eq!(doc.compose(4, 2).pixels[0], [2.5; 3]);
        assert_eq!(doc.compose(4, 2).pixels[2], [1.0; 3]);

        doc.lights[upper].image_layer.linear = false;
        assert!((doc.compose(4, 2).pixels[2][0] - 0.714_041).abs() < 1e-6);
        doc.lights.swap(lower, upper);
        assert_eq!(doc.compose(4, 2).pixels, vec![[1.0; 3]; 8]);
    }

    #[test]
    fn rectangular_falloff_keeps_straight_edges_without_a_diagonal_ridge() {
        let mut doc = Document::default();
        let index = doc.add_light();
        let light = &mut doc.lights[index];
        light.shape = LightShape::Rectangle;
        light.falloff = FalloffCurve::from_preset(FalloffPreset::Linear);
        light.falloff.box_interpolation = BoxInterpolation::Smooth;
        light.core = [0.2, -0.4];
        let falloff = light.bake_falloff();
        let profile = |x, y| light.profile(glam::Vec2::new(x, y), &falloff);
        for [x, y] in [[-1.0, 0.8], [1.0, -0.4], [0.6, -1.0], [-0.3, 1.0]] {
            assert!(profile(x, y) < 1e-6);
        }
        assert!((profile(0.6, -0.4) - 0.5).abs() < 1e-6);

        // At this diagonal the old maximum had a flat slope on one side and
        // a steep slope on the other. The new profile has a continuous slope.
        let y = 0.3;
        let h = 0.002;
        let before = (profile(0.6, y) - profile(0.6, y - h)) / h;
        let after = (profile(0.6, y + h) - profile(0.6, y)) / h;
        assert!(before < -0.1 && after < -0.1);
        assert!((before - after).abs() < 0.01);
        let center = light.core[1];
        let axis_slope = (profile(0.6, center + h) - profile(0.6, center)) / h;
        assert!(axis_slope.abs() < 0.01);
    }

    #[test]
    fn box_gradient_can_adjust_horizontal_and_vertical_ramps_independently() {
        let mut doc = Document::default();
        let index = doc.add_light();
        let light = &mut doc.lights[index];
        light.shape = LightShape::Rectangle;
        light.falloff = FalloffCurve::from_preset(FalloffPreset::Linear);
        light.box_vertical = Some(FalloffCurve::from_preset(FalloffPreset::HardEdge));
        let point = glam::Vec2::splat(0.5);
        assert!((light.profile(point, &light.bake_falloff()) - 0.25).abs() < 1e-6);

        light.box_link_axes = false;
        let falloff = light.bake_falloff();
        assert!((light.profile(point, &falloff) - 0.5).abs() < 1e-6);
        assert!((light.profile(glam::Vec2::new(0.25, 0.8), &falloff) - 0.75).abs() < 1e-6);
    }

    #[test]
    fn box_linear_preset_is_linear_in_editor_and_render() {
        let mut doc = Document::default();
        let index = doc.add_light();
        let light = &mut doc.lights[index];
        light.shape = LightShape::Rectangle;
        light.falloff = FalloffCurve::from_preset(FalloffPreset::Linear);
        for [distance, expected] in [[0.25, 0.75], [0.5, 0.5], [0.75, 0.25]] {
            assert!((light.falloff.sample(distance, true) - expected).abs() < 1e-6);
            let actual = light.profile(glam::Vec2::new(distance, 0.0), &light.bake_falloff());
            assert!((actual - expected).abs() < 1e-6);
        }
        light.falloff.box_interpolation = BoxInterpolation::Smooth;
        assert!((light.falloff.sample(0.25, true) - 0.84375).abs() < 1e-6);
    }

    #[test]
    fn box_presets_keep_the_slope_through_intermediate_anchors() {
        let mut doc = Document::default();
        let index = doc.add_light();
        let light = &mut doc.lights[index];
        light.shape = LightShape::Rectangle;
        for (preset, anchor) in [
            (FalloffPreset::Softbox, 0.875),
            (FalloffPreset::Smooth, 0.25),
            (FalloffPreset::Tight, 0.50),
        ] {
            light.falloff = FalloffCurve::from_preset(preset);
            let falloff = light.bake_falloff();
            let editor = |x| light.falloff.sample(x, true);
            let render = |x| light.profile(glam::Vec2::new(x, 0.0), &falloff);
            let h = 0.001;
            for sample in [&editor as &dyn Fn(f32) -> f32, &render] {
                let center = sample(anchor);
                let left = (center - sample(anchor - h)) / h;
                let right = (sample(anchor + h) - center) / h;
                assert!(left < -0.3 && right < -0.3);
                assert!((left - right).abs() < 0.08);
            }
            assert!((editor(anchor) - render(anchor)).abs() < 1e-4);
            assert!((0..512).all(|i| editor(i as f32 / 512.) >= editor((i + 1) as f32 / 512.)));
        }
    }

    #[test]
    fn directional_gradient_is_linear_and_rotates_inside_rectangle() {
        let mut doc = Document::default();
        let index = doc.add_light();
        let light = &mut doc.lights[index];
        light.shape = LightShape::Gradient;
        light.mapping = LightMapping::Flat;
        let falloff = light.bake_falloff();
        for y in [-0.75, 0.0, 0.75] {
            assert!((light.profile(glam::Vec2::new(-0.5, y), &falloff) - 0.75).abs() < 1e-6);
            assert!((light.profile(glam::Vec2::new(0.5, y), &falloff) - 0.25).abs() < 1e-6);
        }
        light.gradient_rotation = 90.0;
        let falloff = light.bake_falloff();
        assert!((light.profile(glam::Vec2::new(0.7, -0.5), &falloff) - 0.75).abs() < 1e-6);
        assert!(!light.covers(glam::Vec2::new(0.0, 1.01), None));
    }

    #[test]
    fn falloff_presets_and_custom_points_change_composited_radiance() {
        let mut doc = Document::default();
        let index = doc.add_light();
        let light = &mut doc.lights[index];
        light.mapping = LightMapping::Flat;
        light.shape = LightShape::Round;
        light.u = 180.5 / 360.0;
        light.v = 90.5 / 180.0;
        light.width_degrees = 96.0;
        let sample = |doc: &Document| doc.compose(360, 180).pixels[90 * 360 + 204][0];

        doc.lights[index].falloff = FalloffCurve::from_preset(FalloffPreset::Linear);
        assert!((sample(&doc) - 5.0).abs() < 1e-4);
        doc.lights[index].falloff = FalloffCurve::from_preset(FalloffPreset::HardEdge);
        assert_eq!(sample(&doc), 10.0);
        doc.lights[index].falloff = FalloffCurve {
            points: vec![[0.0, 1.0], [0.5, 0.2], [1.0, 0.0]],
            preset: None,
            box_interpolation: BoxInterpolation::Smooth,
        };
        assert!((sample(&doc) - 2.0).abs() < 0.005);
    }

    #[test]
    fn falloff_preview_shows_linear_gray_independently_of_hdr_exposure_and_color() {
        let mut doc = Document::default();
        let index = doc.add_light();
        let light = &mut doc.lights[index];
        light.mapping = LightMapping::Flat;
        light.shape = LightShape::Round;
        light.u = 180.5 / 360.0;
        light.v = 90.5 / 180.0;
        light.width_degrees = 96.0;
        light.color = [1.0, 0.0, 0.0];
        light.falloff = FalloffCurve::from_preset(FalloffPreset::Linear);
        doc.exposure = 1.0;

        let center = 90 * 360 + 180;
        let halfway = center + 24;
        let actual = doc.falloff_preview(index, 360, 180).unwrap();
        assert_eq!(actual[center], eframe::egui::Color32::WHITE);
        assert!(actual[halfway].r().abs_diff(128) <= 1);
        assert_eq!(actual[halfway].r(), actual[halfway].g());
        assert_eq!(actual[halfway].g(), actual[halfway].b());

        let hdr = doc.compose(360, 180);
        assert!((hdr.pixels[halfway][0] - 10.0).abs() < 1e-4);
        assert!(hdr.display_pixels()[halfway].r() > 230);

        let light = &mut doc.lights[index];
        assert!(light.hit(light.u, light.v));
        light.enabled = false;
        assert!(!light.hit(light.u, light.v));
        assert!(doc.falloff_preview(index, 360, 180).is_none());
        assert_eq!(doc.compose(360, 180).pixels[halfway], [0.0; 3]);
    }
    #[test]
    fn editing_preserves_hdr_energy_and_rotation_wraps() {
        let mut doc = Document::default();
        let i = doc.add_light();
        doc.lights[i].u = 0.0;
        doc.lights[i].v = 0.5;
        doc.lights[i].falloff = FalloffCurve::from_preset(FalloffPreset::HardEdge);
        let image = doc.compose(360, 180);
        assert_eq!(image.pixels[90 * 360], [10.; 3]);
        assert_eq!(image.pixels[90 * 360 + 359], [10.; 3]);
        assert_eq!(image.pixels[90 * 360 + 180], [0.; 3]);
        doc.rotation = 180.0;
        doc.exposure = 1.0;
        assert_eq!(doc.compose(360, 180).pixels[90 * 360 + 180], [20.; 3]);
        doc.lights[i].enabled = false;
        assert!(
            doc.compose(16, 8)
                .pixels
                .iter()
                .flatten()
                .all(|v| *v == 0.0)
        );
    }

    #[test]
    fn blend_modes_use_accumulated_hdr_and_leave_uncovered_pixels_alone() {
        let mut doc = Document::default();
        let index = doc.add_light();
        let light = &mut doc.lights[index];
        light.intensity = 0.5;
        light.color = [1.0, 0.5, 0.0];
        let base = Vec3::new(4.0, 8.0, 2.0);
        for (mode, expected) in [
            (BlendMode::Add, Vec3::new(4.5, 8.25, 2.0)),
            (BlendMode::Multiply, Vec3::new(2.0, 2.0, 0.0)),
            (BlendMode::Amplify, Vec3::new(6.0, 10.0, 2.0)),
        ] {
            light.blend = mode;
            assert_eq!(
                light.blend.apply(
                    base,
                    Vec3::from_array(light.color) * light.intensity,
                    light.profile(glam::Vec2::ZERO, &light.bake_falloff()),
                    light.invert
                ),
                expected
            );
            assert!(!light.hit(light.u + 0.5, light.v));
        }
    }

    #[test]
    fn over_low_pass_and_invert_apply_the_light_profile_to_hdr_layers() {
        let background = [4.0, 8.0, 2.0];
        let mut doc = Document::default();
        let base = doc.add_light();
        doc.lights[base].shape = LightShape::Image;
        doc.lights[base].image_layer.image = Some(Arc::new(HdrImage {
            width: 2,
            height: 1,
            pixels: vec![background; 2],
        }));
        let index = doc.add_light();
        let light = &mut doc.lights[index];
        light.u = 0.5;
        light.v = 0.5;
        light.width_degrees = 160.0;
        light.shape = LightShape::Round;
        light.mapping = LightMapping::Flat;
        light.falloff = FalloffCurve::from_preset(FalloffPreset::Linear);
        light.color = [0.5, 0.25, 1.0];
        let preview = doc.falloff_preview(index, 9, 5);

        for (mode, invert, intensity, center, halfway) in [
            (
                BlendMode::Over,
                false,
                4.0,
                [2.0, 1.0, 4.0],
                [3.0, 4.5, 3.0],
            ),
            (
                BlendMode::LowPass,
                false,
                4.0,
                [2.0, 4.0, 1.0],
                [3.0, 6.0, 1.5],
            ),
            (
                BlendMode::LowPass,
                true,
                12.0,
                [6.0, 12.0, 3.0],
                [5.0, 10.0, 2.5],
            ),
            (BlendMode::Add, true, 4.0, [2.0, 4.0, 1.0], [3.0, 6.0, 1.5]),
            (BlendMode::Add, true, 12.0, [0.0; 3], [1.0, 2.0, 0.5]),
            (
                BlendMode::Amplify,
                true,
                4.0,
                [4.0 / 3.0, 4.0, 0.4],
                [2.0, 8.0 / 1.5, 2.0 / 3.0],
            ),
        ] {
            let light = &mut doc.lights[index];
            light.blend = mode;
            light.invert = invert;
            light.intensity = intensity;
            let image = doc.compose(9, 5);
            assert_eq!(image.pixels[18], background);
            assert!(Vec3::from_array(image.pixels[22]).abs_diff_eq(Vec3::from_array(center), 1e-5));
            assert!(
                Vec3::from_array(image.pixels[21]).abs_diff_eq(Vec3::from_array(halfway), 1e-5)
            );
            assert_eq!(doc.falloff_preview(index, 9, 5), preview);
        }
    }

    #[test]
    fn round_hotspot_shifts_inside_outline_and_hard_cut_masks_at_threshold() {
        let mut doc = Document::default();
        let index = doc.add_light();
        let light = &mut doc.lights[index];
        light.shape = LightShape::Round;
        let falloff = FalloffCurve::from_preset(FalloffPreset::Smooth).bake(false);
        light.core = [0.6, 0.0];
        assert!(!light.covers(glam::Vec2::new(0.8, 0.8), None));
        assert_eq!(
            light.profile(glam::Vec2::from_array(light.core), &falloff),
            1.0
        );
        assert!(light.profile(glam::Vec2::ZERO, &falloff) < 1.0);
        assert!(light.profile(glam::Vec2::X, &falloff) < 1e-6);
        light.cut_enabled = true;
        light.cut_amount = 0.4;
        assert!(light.covers(glam::Vec2::new(0.19, 0.0), light.cut_plane()));
        assert!(!light.covers(glam::Vec2::new(0.21, 0.0), light.cut_plane()));
    }

    #[test]
    fn rotated_cut_follows_canvas_angle_for_both_shapes() {
        let mut doc = Document::default();
        let index = doc.add_light();
        let light = &mut doc.lights[index];
        light.width_degrees = 80.0;
        light.height_degrees = 20.0;
        light.mapping = LightMapping::Flat;
        light.cut_enabled = true;
        light.cut_rotation = 45.0;
        for shape in [LightShape::Rectangle, LightShape::Round] {
            light.shape = shape;
            let cut = light.cut_plane();
            assert!(light.covers(glam::Vec2::new(-0.2, -0.2), cut));
            assert!(!light.covers(glam::Vec2::new(0.2, 0.2), cut));
            let [a, b] = light.cut_segment().unwrap();
            let canvas_delta = (b - a) * glam::Vec2::new(80.0, 20.0);
            assert!((canvas_delta.x + canvas_delta.y).abs() < 1e-4);
        }
        light.cut_amount = 1.0;
        assert!(!light.covers(glam::Vec2::ZERO, light.cut_plane()));
        light.cut_amount = 0.0;
        assert!(light.covers(glam::Vec2::ZERO, light.cut_plane()));
    }

    #[test]
    fn spherical_light_warps_near_pole_and_crosses_seam() {
        let mut doc = Document::default();
        let i = doc.add_light();
        let light = &mut doc.lights[i];
        light.shape = LightShape::Rectangle;
        light.u = 0.99;
        light.v = 0.5;
        light.width_degrees = 45.0;
        light.height_degrees = 30.0;
        light.falloff = FalloffCurve::from_preset(FalloffPreset::Softbox);
        let local = glam::Vec2::new(0.75, 0.0);
        let equator = light.projection().panorama(local);
        light.v = 0.10;
        let projected = light.projection().panorama(local);
        let longitude_span = |u: f32| (u - 0.99).rem_euclid(1.0);
        assert!(longitude_span(projected.x) > 2.0 * longitude_span(equator.x));
        assert!(projected.x < 0.25 && projected.y > light.v);
        assert!(light.hit(projected.x, projected.y));

        let direction = panorama_direction(projected.x, projected.y);
        let recovered = light.projection().local(projected, direction).unwrap();
        assert!((recovered - local).length() < 1e-5);
        let image = doc.compose(360, 180);
        let x = (projected.x * 360.0) as usize;
        let y = (projected.y * 180.0) as usize;
        assert!(image.pixels[y * 360 + x][0] > 5.0);
    }
    #[test]
    fn diffuse_constant_environment_conserves_radiance() {
        let image = HdrImage {
            width: 128,
            height: 64,
            pixels: vec![[2., 3., 4.]; 128 * 64],
        };
        let sh = image.diffuse_sh();
        for n in [Vec3::X, Vec3::Y, Vec3::Z, -Vec3::Y] {
            let evaluated = sh
                .into_iter()
                .zip(sh_basis(n))
                .fold(Vec3::ZERO, |c, (a, b)| c + Vec3::from_array(a) * b);
            assert!((evaluated - Vec3::new(2., 3., 4.)).abs().max_element() < 0.005);
        }
    }

    #[test]
    fn shadow_samples_follow_hdr_radiance_and_solid_angle() {
        let mut image = HdrImage {
            width: 16,
            height: 8,
            pixels: vec![[0.0; 3]; 128],
        };
        assert!(image.shadow_samples().is_none());

        image.pixels[3] = [10.0; 3];
        image.pixels[3 * 16 + 11] = [10.0; 3];
        let samples = image.shadow_samples().unwrap();
        let polar = panorama_direction(3.5 / 16.0, 0.5 / 8.0).to_array();
        let equatorial = panorama_direction(11.5 / 16.0, 3.5 / 8.0).to_array();
        let polar_count = samples
            .directions
            .iter()
            .filter(|&&ray| ray == polar)
            .count();
        let equatorial_count = samples
            .directions
            .iter()
            .filter(|&&ray| ray == equatorial)
            .count();
        assert_eq!(polar_count + equatorial_count, SHADOW_SAMPLE_COUNT);
        assert!((10..=11).contains(&polar_count));
        for weight in samples.weights {
            assert!(Vec3::from_array(weight).abs_diff_eq(Vec3::ONE, 1e-6));
        }
    }
    #[test]
    fn hdr_export_reimport_keeps_values_above_one() {
        let path =
            std::env::temp_dir().join(format!("hdr-studio-roundtrip-{}.hdr", std::process::id()));
        let image = HdrImage {
            width: 8,
            height: 4,
            pixels: vec![[32., 8., 2.]; 32],
        };
        image.save(&path).unwrap();
        let read = HdrImage::load(&path).unwrap();
        assert!((read.pixels[0][0] - 32.).abs() < 0.2);
        assert!((read.pixels[0][1] - 8.).abs() < 0.1);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn exr_import_preserves_linear_color_and_rejects_non_panorama() {
        let path = std::env::temp_dir().join(format!("hdr-studio-exr-{}.exr", std::process::id()));
        let source = image::Rgb32FImage::from_pixel(8, 4, Rgb([6.0, 2.0, 0.5]));
        source
            .save_with_format(&path, image::ImageFormat::OpenExr)
            .unwrap();
        let decoded = HdrImage::load(&path).unwrap();
        assert_eq!(decoded.pixels[0], [6.0, 2.0, 0.5]);
        image::Rgb32FImage::from_pixel(4, 4, Rgb([1.0; 3]))
            .save_with_format(&path, image::ImageFormat::OpenExr)
            .unwrap();
        assert!(HdrImage::load(&path).unwrap_err().contains("2:1"));
        std::fs::remove_file(path).unwrap();
    }
}
