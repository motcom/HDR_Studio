use crate::{camera::Camera, environment::HdrImage, scene::Vertex};
use eframe::glow::{self, HasContext};

pub struct Renderer {
    program: glow::Program,
    background_program: glow::Program,
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    count: i32,
    environment: glow::Texture,
    diffuse_sh: [[f32; 3]; 9],
    max_lod: f32,
}

#[derive(Clone, Copy)]
pub struct Lighting {
    pub default_light: bool,
    pub environment: bool,
    pub background: bool,
    pub roughness: f32,
    pub metallic: f32,
}
impl Default for Lighting {
    fn default() -> Self {
        Self {
            default_light: true,
            environment: true,
            background: false,
            roughness: 0.25,
            metallic: 0.0,
        }
    }
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
            let program = program(gl, VERTEX, &format!("{COMMON}\n{FRAGMENT}"))?;
            let background_program = self::program(
                gl,
                BACKGROUND_VERTEX,
                &format!("{COMMON}\n{BACKGROUND_FRAGMENT}"),
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
                diffuse_sh: [[0.; 3]; 9],
                max_lod: 0.0,
            })
        }
    }
    pub fn upload(&mut self, gl: &glow::Context, vertices: &[Vertex]) -> Result<(), String> {
        self.count = i32::try_from(vertices.len()).map_err(|_| "Mesh exceeds GPU draw size")?;
        unsafe {
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
    pub fn upload_environment(&mut self, gl: &glow::Context, image: &HdrImage) {
        self.diffuse_sh = image.diffuse_sh();
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
    }
    pub fn paint(&self, gl: &glow::Context, camera: &Camera, lighting: Lighting) {
        unsafe {
            // Eframe supplies a 24-bit depth buffer. Scissor confines clearing to the render gate.
            gl.disable(glow::CULL_FACE);
            gl.disable(glow::BLEND);
            gl.bind_vertex_array(Some(self.vao));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.environment));
            if lighting.background && lighting.environment {
                gl.disable(glow::DEPTH_TEST);
                gl.depth_mask(false);
                gl.use_program(Some(self.background_program));
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
                gl.draw_arrays(glow::TRIANGLES, 0, 3);
            }
            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LESS);
            gl.depth_mask(true);
            gl.clear_depth_f32(1.0);
            gl.clear(glow::DEPTH_BUFFER_BIT);
            gl.disable(glow::CULL_FACE);
            gl.disable(glow::BLEND);
            gl.use_program(Some(self.program));
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
const float PI = 3.141592653589793;
vec2 env_uv(vec3 d) {
    return vec2(atan(d.z,d.x)/(2.0*PI)+0.5,acos(clamp(d.y,-1.0,1.0))/PI);
}
vec3 display_color(vec3 c) {
    c=max(c,vec3(0.0));c=c/(1.0+c);
    return mix(12.92*c,1.055*pow(c,vec3(1.0/2.4))-0.055,step(vec3(0.0031308),c));
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
uniform vec3 eye;
uniform vec3 eye_forward;
uniform vec3 sh[9];
uniform float roughness;
uniform float metallic;
uniform float max_lod;
out vec4 color;
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
        vec3 irradiance=sh[0]*0.282095+sh[1]*(0.488603*w.y)+sh[2]*(0.488603*w.z)+sh[3]*(0.488603*w.x)
            +sh[4]*(1.092548*w.x*w.y)+sh[5]*(1.092548*w.y*w.z)+sh[6]*(0.315392*(3.0*w.z*w.z-1.0))
            +sh[7]*(1.092548*w.x*w.z)+sh[8]*(0.546274*(w.x*w.x-w.y*w.y));
        vec3 reflection=textureLod(environment_map,env_uv(reflect(-v,w)),roughness*max_lod).rgb;
        vec3 f0=mix(vec3(0.04),albedo,metallic);
        float nv=max(dot(w,v),0.0);
        vec3 fresnel=f0+(1.0-f0)*pow(1.0-nv,5.0);
        c+=max(irradiance,vec3(0.0))*albedo*(1.0-metallic)*(1.0-fresnel)+reflection*fresnel;
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
out vec4 color;
void main(){vec4 a=inverse_vp*vec4(ndc,-1.0,1.0);vec4 b=inverse_vp*vec4(ndc,1.0,1.0);
    vec3 direction=normalize(b.xyz/b.w-a.xyz/a.w);color=vec4(display_color(textureLod(environment_map,env_uv(direction),0.0).rgb),1.0);}
"#;
