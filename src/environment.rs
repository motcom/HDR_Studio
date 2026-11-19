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
        let x = u.rem_euclid(1.0) * self.width as f32 - 0.5;
        let y = (v * self.height as f32 - 0.5).clamp(0.0, self.height as f32 - 1.0);
        let x0 = x.floor() as i32;
        let y0 = y.floor() as u32;
        let tx = x - x.floor();
        let ty = y - y.floor();
        let pixel = |x: i32, y: u32| {
            Vec3::from_array(
                self.pixels[(y * self.width + x.rem_euclid(self.width as i32) as u32) as usize],
            )
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
}

impl BlendMode {
    pub const ALL: [Self; 3] = [Self::Add, Self::Multiply, Self::Amplify];

    pub fn label(self) -> &'static str {
        match self {
            Self::Add => "Add",
            Self::Multiply => "Multiply",
            Self::Amplify => "Amplify",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LightShape {
    Rectangle,
    Round,
    Gradient,
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

#[derive(Clone)]
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
pub struct Light {
    pub name: String,
    pub enabled: bool,
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
    pub shape: LightShape,
    pub mapping: LightMapping,
    pub core: [f32; 2],
    pub cut_enabled: bool,
    pub cut_rotation: f32,
    pub cut_amount: f32,
}
impl Light {
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
        };
        inside && cut.is_none_or(|(normal, distance)| normal.dot(p) <= distance)
    }

    pub fn hit(&self, u: f32, v: f32) -> bool {
        self.enabled
            && self
                .projection()
                .local(glam::Vec2::new(u, v), panorama_direction(u, v))
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
            LightShape::Round => self.falloff.bake(false),
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

    fn blend_at(&self, underneath: Vec3, position: glam::Vec2, falloff: &FalloffTable) -> Vec3 {
        let light = Vec3::from_array(self.color) * self.intensity * self.profile(position, falloff);
        match self.blend {
            BlendMode::Add => underneath + light,
            BlendMode::Multiply => underneath * light,
            BlendMode::Amplify => underneath * (Vec3::ONE + light),
        }
    }
}

#[derive(Clone)]
pub struct Document {
    pub base: Option<Arc<HdrImage>>,
    pub source: String,
    pub exposure: f32,
    pub rotation: f32,
    pub lights: Vec<Light>,
    next_id: u32,
}
impl Default for Document {
    fn default() -> Self {
        Self {
            base: None,
            source: "Black environment".into(),
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
            .filter(|light| light.enabled)?
            .clone();
        let falloff = light.bake_falloff();
        let white_level = if light.shape == LightShape::Gradient {
            falloff.samples.iter().copied().fold(0.0_f32, f32::max)
        } else {
            light.profile(glam::Vec2::from_array(light.core), &falloff)
        };
        light.blend = BlendMode::Add;
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
            name: format!("Softbox {id}"),
            enabled: true,
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
            shape: LightShape::Rectangle,
            mapping: LightMapping::Spherical,
            core: [0.0; 2],
            cut_enabled: false,
            cut_rotation: 0.0,
            cut_amount: 0.5,
        });
        self.lights.len() - 1
    }
    pub fn compose(&self, width: u32, height: u32) -> HdrImage {
        let gain = 2.0_f32.powf(self.exposure);
        let layers: Vec<_> = self
            .lights
            .iter()
            .filter(|light| light.enabled)
            .map(|light| {
                (
                    light,
                    light.projection(),
                    light.cut_plane(),
                    light.bake_falloff(),
                )
            })
            .collect();
        let spherical = layers
            .iter()
            .any(|(_, projection, _, _)| matches!(projection, Projection::Spherical { .. }));
        let columns: Vec<_> = (0..width)
            .map(|x| {
                let u = ((x as f32 + 0.5) / width as f32 - self.rotation / 360.0).rem_euclid(1.0);
                let (sin, cos) = if spherical {
                    panorama_angles(u, 0.5).0.sin_cos()
                } else {
                    (0.0, 0.0)
                };
                (u, sin, cos)
            })
            .collect();
        let mut pixels = Vec::with_capacity((width * height) as usize);
        for y in 0..height {
            let v = (y as f32 + 0.5) / height as f32;
            let (sin_lat, cos_lat) = if spherical {
                panorama_angles(0.5, v).1.sin_cos()
            } else {
                (0.0, 0.0)
            };
            for &(u, sin_lon, cos_lon) in &columns {
                let uv = glam::Vec2::new(u, v);
                let direction = Vec3::new(cos_lon * cos_lat, sin_lat, sin_lon * cos_lat);
                let mut rgb = self
                    .base
                    .as_ref()
                    .map_or(Vec3::ZERO, |base| base.sample(u, v));
                for (light, projection, cut, falloff) in &layers {
                    if let Some(position) = projection.local(uv, direction)
                        && light.covers(position, *cut)
                    {
                        // Keep stacked gains within representable RGBE range.
                        rgb = light
                            .blend_at(rgb, position, falloff)
                            .min(Vec3::splat(1.0e30));
                    }
                }
                pixels.push((rgb * gain).to_array());
            }
        }
        HdrImage {
            width,
            height,
            pixels,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangular_falloff_keeps_straight_edges_without_a_diagonal_ridge() {
        let mut doc = Document::default();
        let index = doc.add_light();
        let light = &mut doc.lights[index];
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
                light.blend_at(base, glam::Vec2::ZERO, &light.bake_falloff()),
                expected
            );
            assert!(!light.hit(light.u + 0.5, light.v));
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
