use crate::{
    camera::Camera,
    environment::{HdrImage, SHADOW_SAMPLE_COUNT, ShadowSamples},
    light_paint::PickMesh,
    scene::Vertex,
};
use eframe::glow::{self, HasContext};

pub const SHADOW_SAMPLE_OPTIONS: [u32; 4] = [4, 8, 16, 32];
pub const PREVIEW_SCALE_OPTIONS: [u32; 4] = [25, 50, 75, 100];

pub struct Renderer {
    program: glow::Program,
    background_program: glow::Program,
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    count: i32,
    environment: glow::Texture,
    ray_mesh: Option<(glow::Texture, i32)>,
    ray_mesh_error: Option<String>,
    shadow_samples: Option<ShadowSamples>,
    floor_height: f32,
    diffuse_sh: [[f32; 3]; 9],
    max_lod: f32,
}

#[derive(Clone, Copy, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct Lighting {
    pub canvas_exposure_ev: f32,
    pub render_preview_exposure_ev: f32,
    pub default_light: bool,
    pub environment: bool,
    pub background: bool,
    pub roughness: f32,
    pub metallic: f32,
    pub ray_reflections: bool,
    pub soft_shadows: bool,
    pub floor_shadows: bool,
    pub shadow_samples: u32,
    pub preview_scale: u32,
    pub idle_shadow_samples: u32,
    pub idle_preview_scale: u32,
}
impl Default for Lighting {
    fn default() -> Self {
        Self {
            canvas_exposure_ev: 0.0,
            render_preview_exposure_ev: 0.0,
            default_light: true,
            environment: true,
            background: true,
            roughness: 0.25,
            metallic: 0.0,
            ray_reflections: false,
            soft_shadows: true,
            floor_shadows: false,
            shadow_samples: 4,
            preview_scale: 50,
            idle_shadow_samples: 32,
            idle_preview_scale: 100,
        }
    }
}

fn ray_texture_size(count: usize, max_size: usize, max_layers: usize) -> Option<[usize; 3]> {
    if count == 0 || max_size == 0 || max_layers == 0 || u32::try_from(count).is_err() {
        return None;
    }
    let max_page_size = max_size.saturating_mul(max_size);
    // Prefer 2048-square pages, increasing their size if the GPU has too few array layers.
    let page_size = (2048 * 2048)
        .min(max_page_size)
        .max(count.div_ceil(max_layers));
    if page_size > max_page_size {
        return None;
    }
    let target_layers = count.div_ceil(page_size);
    let texels_per_layer = count.div_ceil(target_layers);
    let width = texels_per_layer.isqrt().next_power_of_two().min(max_size);
    let height = texels_per_layer.div_ceil(width);
    Some([width, height, count.div_ceil(width * height)])
}

unsafe fn program(
    gl: &glow::Context,
    vertex: &str,
    fragment: &str,
) -> Result<glow::Program, String> {
    unsafe {
        let program = gl.create_program()?;
        let mut shaders = Vec::new();
        for (kind, source) in [
            (glow::VERTEX_SHADER, vertex),
            (glow::FRAGMENT_SHADER, fragment),
        ] {
            let shader = gl.create_shader(kind)?;
            gl.shader_source(shader, source);
            gl.compile_shader(shader);
            if !gl.get_shader_compile_status(shader) {
                let error = gl.get_shader_info_log(shader);
                gl.delete_shader(shader);
                for shader in shaders {
                    gl.delete_shader(shader);
                }
                gl.delete_program(program);
                return Err(error);
            }
            gl.attach_shader(program, shader);
            shaders.push(shader);
        }
        gl.link_program(program);
        for shader in shaders {
            gl.detach_shader(program, shader);
            gl.delete_shader(shader);
        }
        if !gl.get_program_link_status(program) {
            let error = gl.get_program_info_log(program);
            gl.delete_program(program);
            return Err(error);
        }
        Ok(program)
    }
}

impl Renderer {
    pub fn new(gl: &glow::Context) -> Result<Self, String> {
        unsafe {
            let ray_code = format!("#define SHADOW_SAMPLES {SHADOW_SAMPLE_COUNT}\n{RAY_TRACE}");
            let program = program(gl, VERTEX, &format!("{COMMON}\n{ray_code}\n{FRAGMENT}"))?;
            let background_program = self::program(
                gl,
                BACKGROUND_VERTEX,
                &format!("{COMMON}\n{ray_code}\n{BACKGROUND_FRAGMENT}"),
            )?;
            let vao = gl.create_vertex_array()?;
            let vbo = gl.create_buffer()?;
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            for index in 0..2 {
                gl.enable_vertex_attrib_array(index);
                gl.vertex_attrib_pointer_f32(index, 3, glow::FLOAT, false, 24, (index * 12) as i32);
            }
            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            let environment = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(environment));
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::REPEAT as i32);
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_T,
                glow::CLAMP_TO_EDGE as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::LINEAR_MIPMAP_LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::LINEAR as i32,
            );
            gl.bind_texture(glow::TEXTURE_2D, None);
            Ok(Self {
                program,
                background_program,
                vao,
                vbo,
                count: 0,
                environment,
                ray_mesh: None,
                ray_mesh_error: None,
                shadow_samples: None,
                floor_height: 0.0,
                diffuse_sh: [[0.; 3]; 9],
                max_lod: 0.0,
            })
        }
    }
    pub fn upload(&mut self, gl: &glow::Context, vertices: &[Vertex]) -> Result<(), String> {
        self.count = i32::try_from(vertices.len()).map_err(|_| "Mesh exceeds GPU draw size")?;
        self.ray_mesh_error = None;
        self.floor_height = vertices
            .iter()
            .map(|vertex| vertex.position[1])
            .reduce(f32::min)
            .unwrap_or(0.0);
        unsafe {
            if let Some((texture, _)) = self.ray_mesh.take() {
                gl.delete_texture(texture);
            }
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
            gl.buffer_data_u8_slice(
                glow::ARRAY_BUFFER,
                bytemuck::cast_slice(vertices),
                glow::STATIC_DRAW,
            );
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
        }
        Ok(())
    }

    pub fn prepare_ray_mesh(
        &mut self,
        gl: &glow::Context,
        mesh: &PickMesh,
        vertices: &[Vertex],
        running: &impl Fn() -> bool,
    ) -> Option<&str> {
        let count = mesh.ray_texel_count();
        if count > 0 && self.ray_mesh.is_none() && self.ray_mesh_error.is_none() {
            match self.upload_ray_mesh(gl, mesh, vertices, count, running) {
                Ok(data) => self.ray_mesh = data,
                Err(error) => self.ray_mesh_error = Some(error),
            }
        }
        self.ray_mesh_error.as_deref()
    }

    fn upload_ray_mesh(
        &self,
        gl: &glow::Context,
        mesh: &PickMesh,
        vertices: &[Vertex],
        count: usize,
        running: &impl Fn() -> bool,
    ) -> Result<Option<(glow::Texture, i32)>, String> {
        let max_size = unsafe { gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE) }.max(1) as usize;
        let max_layers =
            unsafe { gl.get_parameter_i32(glow::MAX_ARRAY_TEXTURE_LAYERS) }.max(0) as usize;
        let triangle_count = vertices.len() / 3;
        let [width, height, layers] = ray_texture_size(count, max_size, max_layers).ok_or_else(|| {
            format!("Ray tracing unavailable for this {triangle_count}-triangle scene: GPU texture array limit is {max_size} × {max_size} × {max_layers}.")
        })?;
        let texel_capacity = width * height * layers;
        let mut texels = Vec::new();
        texels
            .try_reserve_exact(texel_capacity)
            .map_err(|_| "Could not allocate ray tracing data")?;
        texels.resize(texel_capacity, [0_u32; 4]);
        let Some(nodes) = mesh.write_ray_texels(vertices, &mut texels[..count], running) else {
            return Ok(None);
        };
        if !running() {
            return Ok(None);
        }
        unsafe {
            // Ignore errors left by earlier draws when checking this allocation.
            for _ in 0..16 {
                if gl.get_error() == glow::NO_ERROR {
                    break;
                }
            }
            let texture = gl.create_texture()?;
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(texture));
            for parameter in [glow::TEXTURE_MIN_FILTER, glow::TEXTURE_MAG_FILTER] {
                gl.tex_parameter_i32(glow::TEXTURE_2D_ARRAY, parameter, glow::NEAREST as i32);
            }
            gl.tex_image_3d(
                glow::TEXTURE_2D_ARRAY,
                0,
                glow::RGBA32UI as i32,
                width as i32,
                height as i32,
                layers as i32,
                0,
                glow::RGBA_INTEGER,
                glow::UNSIGNED_INT,
                glow::PixelUnpackData::Slice(Some(bytemuck::cast_slice(&texels))),
            );
            let error = gl.get_error();
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, None);
            if error != glow::NO_ERROR {
                gl.delete_texture(texture);
                return Err(format!(
                    "GPU could not upload ray tracing data ({} MiB for {triangle_count} triangles, OpenGL 0x{error:04X}).",
                    (texels.len() * size_of::<[u32; 4]>()).div_ceil(1024 * 1024)
                ));
            }
            Ok(Some((texture, nodes)))
        }
    }
    pub fn upload_environment(
        &mut self,
        gl: &glow::Context,
        image: &HdrImage,
        running: &impl Fn() -> bool,
    ) -> bool {
        let diffuse_sh = image.diffuse_sh();
        if !running() {
            return false;
        }
        let shadow_samples = image.shadow_samples();
        if !running() {
            return false;
        }
        self.diffuse_sh = diffuse_sh;
        self.shadow_samples = shadow_samples;
        self.max_lod = (image.width.max(image.height) as f32).log2().floor();
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(self.environment));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGB32F as i32,
                image.width as i32,
                image.height as i32,
                0,
                glow::RGB,
                glow::FLOAT,
                glow::PixelUnpackData::Slice(Some(bytemuck::cast_slice(&image.pixels))),
            );
            gl.generate_mipmap(glow::TEXTURE_2D);
            gl.bind_texture(glow::TEXTURE_2D, None);
        }
        true
    }

    fn bind_ray_uniforms(
        &self,
        gl: &glow::Context,
        program: glow::Program,
        nodes: i32,
        shadow_count: u32,
    ) {
        unsafe {
            gl.uniform_1_i32(gl.get_uniform_location(program, "ray_mesh").as_ref(), 1);
            gl.uniform_1_i32(
                gl.get_uniform_location(program, "ray_nodes").as_ref(),
                nodes,
            );
            let samples = self
                .shadow_samples
                .as_ref()
                .filter(|_| shadow_count > 0 && nodes > 0);
            gl.uniform_1_i32(
                gl.get_uniform_location(program, "use_soft_shadows")
                    .as_ref(),
                samples.is_some() as i32,
            );
            if let Some(samples) = samples {
                gl.uniform_1_i32(
                    gl.get_uniform_location(program, "shadow_count").as_ref(),
                    shadow_count as i32,
                );
                gl.uniform_3_f32_slice(
                    gl.get_uniform_location(program, "shadow_directions[0]")
                        .as_ref(),
                    bytemuck::cast_slice(&samples.directions),
                );
                gl.uniform_3_f32_slice(
                    gl.get_uniform_location(program, "shadow_weights[0]")
                        .as_ref(),
                    bytemuck::cast_slice(&samples.weights),
                );
            }
        }
    }

    pub fn paint(&self, gl: &glow::Context, camera: &Camera, lighting: Lighting) {
        let display_gain = 2.0_f32.powf(lighting.render_preview_exposure_ev.clamp(-10.0, 10.0));
        unsafe {
            // The offscreen viewport supplies depth; scissor bounds each render band.
            gl.disable(glow::CULL_FACE);
            gl.disable(glow::BLEND);
            gl.bind_vertex_array(Some(self.vao));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.environment));
            let (texture, nodes) = self
                .ray_mesh
                .filter(|_| lighting.environment)
                .map_or((None, 0), |(texture, nodes)| (Some(texture), nodes));
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, texture);
            let show_background = lighting.background && lighting.environment;
            let show_floor = lighting.floor_shadows && nodes > 0 && self.shadow_samples.is_some();
            if show_background || show_floor {
                gl.disable(glow::DEPTH_TEST);
                gl.depth_mask(false);
                gl.use_program(Some(self.background_program));
                gl.uniform_1_f32(
                    gl.get_uniform_location(self.background_program, "display_gain")
                        .as_ref(),
                    display_gain,
                );
                let inverse = (camera.projection * camera.view)
                    .inverse()
                    .as_mat4()
                    .to_cols_array();
                gl.uniform_matrix_4_f32_slice(
                    gl.get_uniform_location(self.background_program, "inverse_vp")
                        .as_ref(),
                    false,
                    &inverse,
                );
                gl.uniform_1_i32(
                    gl.get_uniform_location(self.background_program, "environment_map")
                        .as_ref(),
                    0,
                );
                gl.uniform_1_i32(
                    gl.get_uniform_location(self.background_program, "show_background")
                        .as_ref(),
                    show_background as i32,
                );
                gl.uniform_1_f32(
                    gl.get_uniform_location(self.background_program, "floor_height")
                        .as_ref(),
                    self.floor_height,
                );
                self.bind_ray_uniforms(
                    gl,
                    self.background_program,
                    nodes,
                    if show_floor {
                        lighting.shadow_samples
                    } else {
                        0
                    },
                );
                gl.draw_arrays(glow::TRIANGLES, 0, 3);
            }
            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LESS);
            gl.depth_mask(true);
            gl.clear_depth_f64(1.0);
            gl.clear(glow::DEPTH_BUFFER_BIT);
            gl.disable(glow::CULL_FACE);
            gl.disable(glow::BLEND);
            gl.use_program(Some(self.program));
            gl.uniform_1_f32(
                gl.get_uniform_location(self.program, "display_gain")
                    .as_ref(),
                display_gain,
            );
            gl.bind_vertex_array(Some(self.vao));
            let projection = camera.projection.as_mat4().to_cols_array();
            let view = camera.view.as_mat4().to_cols_array();
            gl.uniform_matrix_4_f32_slice(
                gl.get_uniform_location(self.program, "projection").as_ref(),
                false,
                &projection,
            );
            gl.uniform_matrix_4_f32_slice(
                gl.get_uniform_location(self.program, "view").as_ref(),
                false,
                &view,
            );
            gl.uniform_1_i32(
                gl.get_uniform_location(self.program, "default_light")
                    .as_ref(),
                lighting.default_light as i32,
            );
            gl.uniform_1_i32(
                gl.get_uniform_location(self.program, "use_environment")
                    .as_ref(),
                lighting.environment as i32,
            );
            gl.uniform_1_i32(
                gl.get_uniform_location(self.program, "environment_map")
                    .as_ref(),
                0,
            );
            self.bind_ray_uniforms(
                gl,
                self.program,
                nodes,
                if lighting.soft_shadows {
                    lighting.shadow_samples
                } else {
                    0
                },
            );
            gl.uniform_1_i32(
                gl.get_uniform_location(self.program, "use_ray_reflections")
                    .as_ref(),
                lighting.ray_reflections as i32,
            );
            gl.uniform_1_f32(
                gl.get_uniform_location(self.program, "roughness").as_ref(),
                lighting.roughness,
            );
            gl.uniform_1_f32(
                gl.get_uniform_location(self.program, "metallic").as_ref(),
                lighting.metallic,
            );
            gl.uniform_1_f32(
                gl.get_uniform_location(self.program, "max_lod").as_ref(),
                self.max_lod,
            );
            gl.uniform_3_f32_slice(
                gl.get_uniform_location(self.program, "sh[0]").as_ref(),
                bytemuck::cast_slice(&self.diffuse_sh),
            );
            let inverse = camera.view.inverse();
            let position = inverse.w_axis.truncate().as_vec3();
            let forward = inverse.z_axis.truncate().normalize().as_vec3();
            gl.uniform_3_f32(
                gl.get_uniform_location(self.program, "eye").as_ref(),
                position.x,
                position.y,
                position.z,
            );
            gl.uniform_3_f32(
                gl.get_uniform_location(self.program, "eye_forward")
                    .as_ref(),
                forward.x,
                forward.y,
                forward.z,
            );
            gl.uniform_1_i32(
                gl.get_uniform_location(self.program, "orthographic")
                    .as_ref(),
                camera.is_orthographic() as i32,
            );
            gl.draw_arrays(glow::TRIANGLES, 0, self.count);
            gl.bind_vertex_array(None);
            gl.use_program(None);
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, None);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.disable(glow::DEPTH_TEST);
            gl.depth_mask(false);
            gl.enable(glow::BLEND);
        }
    }
    pub fn destroy(&self, gl: &glow::Context) {
        unsafe {
            gl.delete_buffer(self.vbo);
            gl.delete_vertex_array(self.vao);
            gl.delete_program(self.program);
            gl.delete_program(self.background_program);
            gl.delete_texture(self.environment);
            if let Some((texture, _)) = self.ray_mesh {
                gl.delete_texture(texture);
            }
        }
    }
}

const VERTEX: &str = r#"#version 330 core
layout(location=0) in vec3 position;
layout(location=1) in vec3 normal;
uniform mat4 projection;
uniform mat4 view;
out vec3 N;
out vec3 P;
out vec3 world_normal;
out vec3 world_position;
void main() {
    vec4 p = view * vec4(position,1.0);
    P = p.xyz;
    N = mat3(transpose(inverse(view))) * normal;
    world_normal = normal;
    world_position = position;
    gl_Position = projection * p;
}
"#;
const COMMON: &str = r#"#version 330 core
uniform sampler2D environment_map;
uniform float display_gain;
const float PI = 3.141592653589793;
vec2 env_uv(vec3 d) {
    return vec2(atan(d.z,d.x)/(2.0*PI)+0.5,acos(clamp(d.y,-1.0,1.0))/PI);
}
vec3 display_color(vec3 c) {
    c=max(c*display_gain,vec3(0.0));c=c/(1.0+c);
    return mix(12.92*c,1.055*pow(c,vec3(1.0/2.4))-0.055,step(vec3(0.0031308),c));
}
"#;
const RAY_TRACE: &str = r#"
uniform usampler2DArray ray_mesh;
uniform int ray_nodes;
uniform bool use_soft_shadows;
uniform int shadow_count;
uniform vec3 shadow_directions[SHADOW_SAMPLES];
uniform vec3 shadow_weights[SHADOW_SAMPLES];

uvec4 mesh_texel(uint index, uvec2 page_size) {
    uint row = index / page_size.x;
    ivec3 address = ivec3(index % page_size.x, row % page_size.y, row / page_size.y);
    return texelFetch(ray_mesh, address, 0);
}

vec3 unpack_normal(uint bits) {
    ivec3 components = ivec3(bits & 1023u, (bits >> 10u) & 1023u, (bits >> 20u) & 1023u);
    return vec3((components ^ ivec3(512)) - ivec3(512)) / 511.0;
}

bool intersects_box(vec3 origin, vec3 direction, vec3 low, vec3 high, float nearest) {
    float start = 0.0;
    float end = nearest;
    for (int axis = 0; axis < 3; ++axis) {
        if (abs(direction[axis]) < 1e-12) {
            if (origin[axis] < low[axis] || origin[axis] > high[axis]) return false;
        } else {
            float a = (low[axis] - origin[axis]) / direction[axis];
            float b = (high[axis] - origin[axis]) / direction[axis];
            start = max(start, min(a, b));
            end = min(end, max(a, b));
            if (start > end) return false;
        }
    }
    return true;
}

bool trace_mesh(vec3 origin, vec3 direction, bool any_hit, out vec3 normal) {
    uvec2 page_size = uvec2(textureSize(ray_mesh, 0).xy);
    float scale = max(abs(origin.x), max(abs(origin.y), abs(origin.z)));
    float near_limit = max(1e-7, scale * 4e-7);
    float nearest = 1e30;
    uint node = 0u;
    uint hit = 0xffffffffu;
    vec3 hit_weights;
    vec3 hit_face;
    while (node < uint(ray_nodes)) {
        uvec4 low = mesh_texel(node * 2u, page_size);
        uvec4 high = mesh_texel(node * 2u + 1u, page_size);
        bool branch = low.w == 0xffffffffu;
        if (!intersects_box(origin, direction, uintBitsToFloat(low.xyz), uintBitsToFloat(high.xyz), nearest)) {
            node = branch ? high.w : node + 1u;
            continue;
        }
        ++node;
        if (branch) continue;
        for (uint triangle = low.w; triangle < high.w; ++triangle) {
            uint first = uint(ray_nodes) * 2u + triangle * 3u;
            vec3 a = uintBitsToFloat(mesh_texel(first, page_size).xyz);
            vec3 ab = uintBitsToFloat(mesh_texel(first + 1u, page_size).xyz) - a;
            vec3 ac = uintBitsToFloat(mesh_texel(first + 2u, page_size).xyz) - a;
            vec3 cross_ac = cross(direction, ac);
            float determinant = dot(ab, cross_ac);
            if (abs(determinant) <= 1e-7 * length(ab) * length(ac)) continue;
            vec3 offset = origin - a;
            float u = dot(offset, cross_ac) / determinant;
            if (u < 0.0 || u > 1.0) continue;
            vec3 cross_ab = cross(offset, ab);
            float v = dot(direction, cross_ab) / determinant;
            if (v < 0.0 || u + v > 1.0) continue;
            float distance = dot(ac, cross_ab) / determinant;
            if (distance <= near_limit || distance >= nearest) continue;
            if (any_hit) return true;
            nearest = distance;
            hit = first;
            hit_weights = vec3(1.0 - u - v, u, v);
            hit_face = cross(ab, ac);
        }
    }
    if (hit == 0xffffffffu) return false;
    vec3 interpolated = unpack_normal(mesh_texel(hit, page_size).w) * hit_weights.x
                      + unpack_normal(mesh_texel(hit + 1u, page_size).w) * hit_weights.y
                      + unpack_normal(mesh_texel(hit + 2u, page_size).w) * hit_weights.z;
    normal = normalize(dot(interpolated, interpolated) > 1e-12 ? interpolated : hit_face);
    if (dot(normal, direction) > 0.0) normal = -normal;
    return true;
}

vec3 shadow_visibility(vec3 position, vec3 normal, vec3 geometric_normal) {
    float scale = max(abs(position.x), max(abs(position.y), abs(position.z)));
    // Start above the actual triangle plane so grazing rays cannot re-hit it
    // because of interpolation error. Keep the incidence term on the shading normal.
    vec3 origin = position + geometric_normal * max(1e-6, scale * 2e-6);
    vec3 total = vec3(0.0);
    vec3 blocked = vec3(0.0);
    // Choose one ray per stratum with a stable, per-pixel hash.
    uvec2 pixel = uvec2(gl_FragCoord.xy);
    uint seed = pixel.x * 747796405u ^ pixel.y * 2891336453u;
    int stride = SHADOW_SAMPLES / max(shadow_count, 1);
    for (int i = 0; i < SHADOW_SAMPLES; i += stride) {
        seed = seed * 747796405u + 2891336453u;
        uint bits = ((seed >> ((seed >> 28u) + 4u)) ^ seed) * 277803737u;
        int sample_index = i + int(((bits >> 22u) ^ bits) % uint(stride));
        vec3 direction = shadow_directions[sample_index];
        float cosine = dot(normal, direction);
        if (cosine <= 0.0) continue;
        vec3 weight = shadow_weights[sample_index] * cosine;
        total += weight;
        vec3 unused_normal;
        if (dot(geometric_normal, direction) <= 0.0
            || trace_mesh(origin, direction, true, unused_normal)) blocked += weight;
    }
    return clamp(1.0 - blocked / max(total, vec3(1e-6)), 0.0, 1.0);
}
"#;
const FRAGMENT: &str = r#"
in vec3 N;
in vec3 P;
in vec3 world_normal;
in vec3 world_position;
uniform bool default_light;
uniform bool use_environment;
uniform bool orthographic;
uniform bool use_ray_reflections;
uniform vec3 eye;
uniform vec3 eye_forward;
uniform vec3 sh[9];
uniform float roughness;
uniform float metallic;
uniform float max_lod;
out vec4 color;

vec3 environment_reflection(vec3 direction) {
    return textureLod(environment_map, env_uv(direction), roughness * max_lod).rgb;
}

vec3 shade_environment(vec3 w, vec3 v, vec3 albedo, vec3 reflection, vec3 visibility) {
    vec3 irradiance=sh[0]*0.282095+sh[1]*(0.488603*w.y)+sh[2]*(0.488603*w.z)+sh[3]*(0.488603*w.x)
        +sh[4]*(1.092548*w.x*w.y)+sh[5]*(1.092548*w.y*w.z)+sh[6]*(0.315392*(3.0*w.z*w.z-1.0))
        +sh[7]*(1.092548*w.x*w.z)+sh[8]*(0.546274*(w.x*w.x-w.y*w.y));
    vec3 f0=mix(vec3(0.04),albedo,metallic);
    float nv=max(dot(w,v),0.0);
    vec3 fresnel=f0+(1.0-f0)*pow(1.0-nv,5.0);
    return max(irradiance,vec3(0.0))*albedo*visibility*(1.0-metallic)*(1.0-fresnel)+reflection*fresnel;
}

void main() {
    vec3 n = normalize(N);
    if (dot(n,-P)<0.0) n=-n;
    vec3 l = normalize(vec3(-0.5,0.8,0.7));
    float diffuse = max(dot(n,l),0.0);
    vec3 h = normalize(l+normalize(-P));
    float specular = pow(max(dot(n,h),0.0),48.0);
    vec3 albedo=vec3(0.47,0.56,0.65);
    vec3 c=vec3(0.0);
    if(default_light) c+=albedo*(0.20+0.75*diffuse)+vec3(0.25)*specular;
    if(use_environment) {
        vec3 v=orthographic?normalize(eye_forward):normalize(eye-world_position);
        vec3 w=normalize(world_normal);if(dot(w,v)<0.0)w=-w;
        vec3 direction = reflect(-v, w);
        vec3 reflection = environment_reflection(direction);
        // A single specular ray represents smooth surfaces. Fade into the
        // filtered environment as the reflection lobe broadens.
        float ray_weight = 1.0 - smoothstep(0.15, 0.65, roughness);
        vec3 hit_normal;
        if (use_ray_reflections && ray_nodes > 0 && ray_weight > 0.0
            && trace_mesh(world_position, direction, false, hit_normal)) {
            vec3 incoming = environment_reflection(reflect(direction, hit_normal));
            vec3 bounced = shade_environment(hit_normal, -direction, albedo, incoming, vec3(1.0));
            reflection = mix(reflection, bounced, ray_weight);
        }
        vec3 visibility = vec3(1.0);
        if (use_soft_shadows) {
            vec3 face = cross(dFdx(world_position), dFdy(world_position));
            vec3 geometric_normal = dot(face, face) > 0.0 ? normalize(face) : w;
            if (dot(geometric_normal, w) < 0.0) geometric_normal = -geometric_normal;
            visibility = shadow_visibility(world_position, w, geometric_normal);
        }
        c += shade_environment(w, v, albedo, reflection, visibility);
    }
    color = vec4(display_color(c),1.0);
}
"#;
const BACKGROUND_VERTEX: &str = r#"#version 330 core
out vec2 ndc;
void main(){vec2 p=vec2((gl_VertexID==1)?3.0:-1.0,(gl_VertexID==2)?3.0:-1.0);ndc=p;gl_Position=vec4(p,0.0,1.0);}
"#;
const BACKGROUND_FRAGMENT: &str = r#"
in vec2 ndc;
uniform mat4 inverse_vp;
uniform bool show_background;
uniform float floor_height;
out vec4 color;
void main() {
    vec4 a = inverse_vp * vec4(ndc, -1.0, 1.0);
    vec4 b = inverse_vp * vec4(ndc, 1.0, 1.0);
    vec3 start = a.xyz / a.w;
    vec3 direction = normalize(b.xyz / b.w - start);
    float visibility = 1.0;
    if (use_soft_shadows && direction.y < -1e-6 && start.y >= floor_height) {
        float distance = (floor_height - start.y) / direction.y;
        vec3 point = start + direction * distance;
        point.y = floor_height;
        vec3 up = vec3(0.0, 1.0, 0.0);
        vec3 visible = shadow_visibility(point, up, up);
        visibility = dot(visible, vec3(0.2126, 0.7152, 0.0722));
    }
    vec3 background = show_background
        ? display_color(textureLod(environment_map, env_uv(direction), 0.0).rgb * visibility)
        : (vec3(35.0, 40.0, 49.0) / 255.0) * sqrt(visibility);
    color = vec4(background, 1.0);
}
"#;

#[cfg(test)]
mod tests {
    use super::ray_texture_size;

    #[test]
    fn ray_texture_uses_multiple_pages_beyond_the_size_of_one_texture() {
        let count = 20_000_000;
        let [width, height, layers] = ray_texture_size(count, 4096, 256).unwrap();
        assert!(width <= 4096 && height <= 4096);
        assert_eq!(layers, 5);
        assert!(width * height * layers >= count);

        let [width, height, layers] = ray_texture_size(count, 4096, 2).unwrap();
        assert!(width <= 4096 && height <= 4096);
        assert_eq!(layers, 2);
        assert!(width * height * layers >= count);
        assert!(ray_texture_size(count, 4096, 1).is_none());
    }
}
