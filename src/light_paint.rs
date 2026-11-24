use crate::{camera::Camera, scene::Vertex};
use glam::{DVec2, DVec3};

#[derive(Clone, Copy, PartialEq, Eq, Default, serde::Deserialize, serde::Serialize)]
pub enum LightPaint {
    #[default]
    Reflection,
    Diffuse,
}

impl LightPaint {
    pub fn direction(
        self,
        camera: &Camera,
        ndc: DVec2,
        mesh: &PickMesh,
        vertices: &[Vertex],
    ) -> Option<DVec3> {
        let inverse = (camera.projection * camera.view).inverse();
        let near = inverse.project_point3(ndc.extend(-1.0));
        let far = inverse.project_point3(ndc.extend(1.0));
        let segment = far - near;
        let direction = segment.try_normalize()?;
        let ray = Ray {
            origin: near,
            direction,
        };
        let mut closest = segment.length();
        let mut hit = None;
        mesh.visit(0, &ray, &mut closest, &mut |index, closest| {
            let triangle = &vertices[index * 3..][..3];
            let a = position(&triangle[0]);
            let ab = position(&triangle[1]) - a;
            let ac = position(&triangle[2]) - a;
            let cross = direction.cross(ac);
            let determinant = ab.dot(cross);
            if determinant.abs() <= 1e-12 * ab.length() * ac.length() {
                return;
            }
            let offset = near - a;
            let b = offset.dot(cross) / determinant;
            let second_cross = offset.cross(ab);
            let c = direction.dot(second_cross) / determinant;
            if b < 0.0 || c < 0.0 || b + c > 1.0 {
                return;
            }
            let distance = ac.dot(second_cross) / determinant;
            if distance < 0.0 || distance >= *closest {
                return;
            }
            let normal = |index: usize| glam::Vec3::from_array(triangle[index].normal).as_dvec3();
            let interpolated = normal(0) * (1.0 - b - c) + normal(1) * b + normal(2) * c;
            if let Some(normal) = interpolated
                .try_normalize()
                .or_else(|| ab.cross(ac).try_normalize())
            {
                *closest = distance;
                hit = Some((near + direction * distance, normal));
            }
        });
        let (point, mut normal) = hit?;
        let world_camera = camera.view.inverse();
        let toward_view = if camera.is_orthographic() {
            world_camera.z_axis.truncate()
        } else {
            world_camera.w_axis.truncate() - point
        }
        .try_normalize()?;
        // Match the two-sided lighting used in the preview shader.
        if normal.dot(toward_view) < 0.0 {
            normal = -normal;
        }
        match self {
            Self::Diffuse => Some(normal),
            Self::Reflection => {
                (2.0 * normal.dot(toward_view) * normal - toward_view).try_normalize()
            }
        }
    }
}

fn position(vertex: &Vertex) -> DVec3 {
    glam::Vec3::from_array(vertex.position).as_dvec3()
}

struct Ray {
    origin: DVec3,
    direction: DVec3,
}

#[derive(Clone, Copy)]
struct Bounds {
    min: DVec3,
    max: DVec3,
}

impl Bounds {
    fn intersects(self, ray: &Ray, mut end: f64) -> bool {
        let mut start: f64 = 0.0;
        for axis in 0..3 {
            let origin = ray.origin[axis];
            let direction = ray.direction[axis];
            if direction == 0.0 {
                if origin < self.min[axis] || origin > self.max[axis] {
                    return false;
                }
            } else {
                let a = (self.min[axis] - origin) / direction;
                let b = (self.max[axis] - origin) / direction;
                start = start.max(a.min(b));
                end = end.min(a.max(b));
                if start > end {
                    return false;
                }
            }
        }
        true
    }
}

enum PickNode {
    Leaf {
        bounds: Bounds,
        start: usize,
        end: usize,
    },
    Branch {
        bounds: Bounds,
        left: usize,
        right: usize,
    },
}

#[derive(Default)]
pub struct PickMesh {
    nodes: Vec<PickNode>,
    triangles: Vec<usize>,
}

impl PickMesh {
    #[cfg(test)]
    pub fn new(vertices: &[Vertex]) -> Self {
        Self::build_while(vertices, &|| true).unwrap()
    }

    pub fn build_while(vertices: &[Vertex], running: &impl Fn() -> bool) -> Option<Self> {
        let mut bounds = Vec::with_capacity(vertices.len() / 3);
        for batch in vertices.as_chunks::<3>().0.chunks(4096) {
            if !running() {
                return None;
            }
            bounds.extend(batch.iter().map(|triangle| {
                let [a, b, c] = triangle.each_ref().map(position);
                Bounds {
                    min: a.min(b).min(c),
                    max: a.max(b).max(c),
                }
            }));
        }
        let mut triangles: Vec<_> = (0..bounds.len()).collect();
        let mut nodes = Vec::new();
        if !triangles.is_empty() {
            Self::build(&mut nodes, &mut triangles, &bounds, 0, running)?;
        }
        Some(Self { nodes, triangles })
    }

    pub fn ray_texel_count(&self) -> usize {
        self.nodes.len() * 2 + self.triangles.len() * 3
    }

    pub fn write_ray_texels(
        &self,
        vertices: &[Vertex],
        texels: &mut [[u32; 4]],
        running: &impl Fn() -> bool,
    ) -> Option<i32> {
        let node_count = self.nodes.len();
        if !self.write_ray_node(0, node_count, texels, running) {
            return None;
        }
        // GPU leaf ranges use the same triangle ordering as the CPU BVH.
        let (triangles, _) = texels[node_count * 2..].as_chunks_mut::<3>();
        for (offset, (&index, triangle)) in self.triangles.iter().zip(triangles).enumerate() {
            if offset.is_multiple_of(4096) && !running() {
                return None;
            }
            for (i, vertex) in vertices[index * 3..][..3].iter().enumerate() {
                let [x, y, z] = vertex.position.map(f32::to_bits);
                let [nx, ny, nz] = vertex
                    .normal
                    .map(|value| (value.clamp(-1.0, 1.0) * 511.0).round() as i32 as u32 & 0x3ff);
                triangle[i] = [x, y, z, nx | (ny << 10) | (nz << 20)];
            }
        }
        Some(node_count as i32)
    }

    fn write_ray_node(
        &self,
        index: usize,
        escape: usize,
        texels: &mut [[u32; 4]],
        running: &impl Fn() -> bool,
    ) -> bool {
        if index.is_multiple_of(1024) && !running() {
            return false;
        }
        let (bounds, start, end) = match self.nodes[index] {
            PickNode::Leaf { bounds, start, end } => (bounds, start as u32, end as u32),
            PickNode::Branch {
                bounds,
                left,
                right,
            } => {
                if !self.write_ray_node(left, right, texels, running)
                    || !self.write_ray_node(right, escape, texels, running)
                {
                    return false;
                }
                (bounds, u32::MAX, escape as u32)
            }
        };
        let [x, y, z] = bounds.min.as_vec3().to_array().map(f32::to_bits);
        texels[index * 2] = [x, y, z, start];
        let [x, y, z] = bounds.max.as_vec3().to_array().map(f32::to_bits);
        texels[index * 2 + 1] = [x, y, z, end];
        true
    }

    fn build(
        nodes: &mut Vec<PickNode>,
        triangles: &mut [usize],
        boxes: &[Bounds],
        start: usize,
        running: &impl Fn() -> bool,
    ) -> Option<usize> {
        if !running() {
            return None;
        }
        let bounds = triangles.iter().fold(boxes[triangles[0]], |b, &i| Bounds {
            min: b.min.min(boxes[i].min),
            max: b.max.max(boxes[i].max),
        });
        let index = nodes.len();
        nodes.push(PickNode::Leaf {
            bounds,
            start,
            end: start + triangles.len(),
        });
        if triangles.len() > 8 {
            let axis = (bounds.max - bounds.min).max_position();
            let mid = triangles.len() / 2;
            triangles.select_nth_unstable_by(mid, |&a, &b| {
                (boxes[a].min[axis] + boxes[a].max[axis])
                    .total_cmp(&(boxes[b].min[axis] + boxes[b].max[axis]))
            });
            let (left, right) = triangles.split_at_mut(mid);
            let left = Self::build(nodes, left, boxes, start, running)?;
            let right = Self::build(nodes, right, boxes, start + mid, running)?;
            nodes[index] = PickNode::Branch {
                bounds,
                left,
                right,
            };
        }
        Some(index)
    }

    fn visit(
        &self,
        index: usize,
        ray: &Ray,
        closest: &mut f64,
        hit: &mut impl FnMut(usize, &mut f64),
    ) {
        match self.nodes.get(index) {
            Some(&PickNode::Leaf { bounds, start, end }) if bounds.intersects(ray, *closest) => {
                for &triangle in &self.triangles[start..end] {
                    hit(triangle, closest);
                }
            }
            Some(&PickNode::Branch {
                bounds,
                left,
                right,
            }) if bounds.intersects(ray, *closest) => {
                self.visit(left, ray, closest, hit);
                self.visit(right, ray, closest, hit);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DMat4;

    fn camera(projection: DMat4) -> Camera {
        Camera {
            path: String::new(),
            view: DMat4::IDENTITY,
            projection,
            aspect: 1.0,
            source: String::new(),
            focal: 50.0,
        }
    }

    fn triangle(z: f32, normal: [f32; 3]) -> [Vertex; 3] {
        [[-4.0, -3.0, z], [4.0, -3.0, z], [0.0, 4.0, z]].map(|position| Vertex { position, normal })
    }

    #[test]
    fn chooses_nearest_visible_triangle_and_reflects_the_perspective_view() {
        let camera = camera(crate::camera::frustum(-0.1, 0.1, -0.1, 0.1, 0.1, 20.0));
        let mut vertices = triangle(-6.0, [0., 1., 0.]).to_vec();
        vertices.extend(triangle(-2.0, [0., 0., -1.]));
        vertices.extend(triangle(-0.05, [1., 0., 0.]));
        // Force a tree split and ensure distant triangles do not mask the visible hit.
        for i in 0..16 {
            vertices.extend(triangle(-1., [1., 0., 0.]).map(|mut vertex| {
                vertex.position[0] += 20.0 + 10.0 * i as f32;
                vertex
            }));
        }
        let mesh = PickMesh::new(&vertices);
        let ndc = DVec2::new(0.5, 0.0);
        let diffuse = LightPaint::Diffuse
            .direction(&camera, ndc, &mesh, &vertices)
            .unwrap();
        assert_eq!(diffuse, DVec3::Z);
        let reflection = LightPaint::Reflection
            .direction(&camera, ndc, &mesh, &vertices)
            .unwrap();
        assert!(reflection.abs_diff_eq(DVec3::new(0.5, 0.0, 1.0).normalize(), 1e-12));
        assert!(
            LightPaint::Diffuse
                .direction(&camera, DVec2::new(0.9, 0.9), &mesh, &vertices)
                .is_none()
        );
    }

    #[test]
    fn orthographic_uses_constant_view_direction_and_interpolated_normals() {
        let camera = camera(DMat4::orthographic_rh_gl(-4., 4., -4., 4., 0.1, 20.));
        let mut vertices = triangle(-2., [0., 0., 1.]);
        vertices[2].normal = [0., 1., 0.];
        let mesh = PickMesh::new(&vertices);
        let ndc = DVec2::new(0., 0.125);
        let diffuse = LightPaint::Diffuse
            .direction(&camera, ndc, &mesh, &vertices)
            .unwrap();
        assert!(diffuse.abs_diff_eq(DVec3::new(0., 1., 1.).normalize(), 1e-12));
        let reflection = LightPaint::Reflection
            .direction(&camera, ndc, &mesh, &vertices)
            .unwrap();
        assert!(reflection.abs_diff_eq(DVec3::Y, 1e-12));
    }
}
