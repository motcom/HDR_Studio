use crate::{
    environment::HdrImage,
    renderer::{Lighting, Renderer},
    scene::Scene,
};
use eframe::{
    egui,
    glow::{self, HasContext},
};
use glutin::{
    config::ConfigTemplateBuilder,
    context::{ContextApi, ContextAttributesBuilder, GlProfile, Version},
    display::{Display, DisplayApiPreference},
    prelude::*,
    surface::{SurfaceAttributesBuilder, WindowSurface},
};
use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, Win32WindowHandle, WindowsDisplayHandle,
};
use std::{
    num::{NonZeroIsize, NonZeroU32},
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::HWND,
    System::LibraryLoader::GetModuleHandleW,
    UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WS_POPUP},
};

#[derive(Clone)]
pub struct Request {
    pub scene: Arc<Scene>,
    pub camera: usize,
    pub environment: Option<Arc<HdrImage>>,
    pub lighting: Lighting,
    pub size: [u32; 2],
    pub refinement: usize,
}

impl Request {
    fn same_frame(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.scene, &other.scene)
            && self.camera == other.camera
            && self.lighting == other.lighting
            && self.size == other.size
            && match (&self.environment, &other.environment) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
    }
}

const IDLE_AFTER: [f64; 3] = [0.35, 0.8, 1.5];

fn refined_lighting(mut lighting: Lighting, stage: usize) -> Lighting {
    lighting.preview_scale =
        (lighting.preview_scale + 25 * stage as u32).min(lighting.idle_preview_scale);
    if lighting.environment && (lighting.soft_shadows || lighting.floor_shadows) {
        lighting.shadow_samples =
            (lighting.shadow_samples << stage).min(lighting.idle_shadow_samples);
    }
    lighting
}

struct Frame {
    request: Request,
    image: egui::ColorImage,
    warning: Option<String>,
}

#[derive(Default)]
struct Mailbox {
    pending: Option<Request>,
    result: Option<Result<Frame, String>>,
    closed: bool,
}

#[derive(Default)]
struct Shared {
    mailbox: Mutex<Mailbox>,
    work: Condvar,
}

pub struct Preview {
    shared: Arc<Shared>,
    baseline: Option<Request>,
    idle_since: f64,
    refinement: usize,
    requested: Option<Request>,
    displayed: Option<Request>,
    texture: Option<egui::TextureHandle>,
    pub warning: Option<String>,
}

impl Preview {
    pub fn new(ctx: &egui::Context) -> Result<Self, String> {
        let shared = Arc::new(Shared::default());
        let worker = shared.clone();
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("3D preview".into())
            .spawn(move || {
                if let Err(error) = run_worker(&worker, &ctx) {
                    worker.mailbox.lock().unwrap().result = Some(Err(error));
                    ctx.request_repaint();
                }
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            shared,
            baseline: None,
            idle_since: 0.0,
            refinement: 0,
            requested: None,
            displayed: None,
            texture: None,
            warning: None,
        })
    }

    pub fn poll(&mut self, ctx: &egui::Context) {
        let result = self.shared.mailbox.lock().unwrap().result.take();
        match result {
            Some(Ok(frame)) => {
                if self.requested.as_ref().is_some_and(|request| {
                    Arc::ptr_eq(&request.scene, &frame.request.scene)
                        && request.camera == frame.request.camera
                        && (frame.request.refinement == 0 || request.same_frame(&frame.request))
                }) {
                    self.warning = frame.warning;
                    self.displayed = Some(frame.request);
                    if let Some(texture) = &mut self.texture {
                        texture.set(frame.image, egui::TextureOptions::LINEAR);
                    } else {
                        self.texture = Some(ctx.load_texture(
                            "3D preview",
                            frame.image,
                            egui::TextureOptions::LINEAR,
                        ));
                    }
                }
            }
            Some(Err(error)) => self.warning = Some(error),
            None => {}
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, rect: egui::Rect, mut request: Request) {
        let width = rect.width() * ui.ctx().pixels_per_point();
        let height = rect.height() * ui.ctx().pixels_per_point();
        if width <= 0.0 || height <= 0.0 {
            return;
        }
        let (time, active, max_size) = ui.ctx().input(|input| {
            let active = input.pointer.any_down()
                || input.events.iter().any(|event| {
                    matches!(
                        event,
                        egui::Event::PointerButton { .. }
                            | egui::Event::Key { .. }
                            | egui::Event::MouseWheel { .. }
                            | egui::Event::Zoom(_)
                            | egui::Event::Text(_)
                    )
                });
            (input.time, active, input.max_texture_side as f32)
        });
        request.size = [width.round() as u32, height.round() as u32];
        let changed = self
            .baseline
            .as_ref()
            .is_none_or(|baseline| !baseline.same_frame(&request));
        if changed {
            self.baseline = Some(request.clone());
        }
        if changed || active {
            self.idle_since = time;
            self.refinement = 0;
        }

        let target_frame = |stage| {
            let mut frame = request.clone();
            frame.refinement = stage;
            frame.lighting = refined_lighting(frame.lighting, stage);
            let scale =
                (frame.lighting.preview_scale as f32 / 100.0).min(max_size / width.max(height));
            frame.size = [
                (width * scale).round().max(1.0) as u32,
                (height * scale).round().max(1.0) as u32,
            ];
            frame
        };
        let mut target = target_frame(self.refinement);
        if !active
            && !changed
            && let Some(&delay) = IDLE_AFTER.get(self.refinement)
            && self
                .displayed
                .as_ref()
                .is_some_and(|displayed| displayed.same_frame(&target))
        {
            let next = target_frame(self.refinement + 1);
            if !next.same_frame(&target) {
                let remaining = self.idle_since + delay - time;
                if remaining <= 0.0 {
                    self.refinement += 1;
                    target = next;
                } else {
                    ui.ctx()
                        .request_repaint_after(Duration::from_secs_f64(remaining));
                }
            }
        }

        if self.displayed.as_ref().is_some_and(|frame| {
            Arc::ptr_eq(&frame.scene, &request.scene) && frame.camera == request.camera
        }) && let Some(texture) = &self.texture
        {
            ui.painter().image(
                texture.id(),
                rect,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        if self
            .requested
            .as_ref()
            .is_none_or(|previous| !previous.same_frame(&target))
        {
            self.requested = Some(target.clone());
            self.shared.mailbox.lock().unwrap().pending = Some(target);
            self.shared.work.notify_one();
        }
    }

    pub fn quality(&self) -> Option<([usize; 2], u32, u32)> {
        let texture = self.texture.as_ref()?;
        let lighting = self.displayed.as_ref()?.lighting;
        Some((
            texture.size(),
            lighting.preview_scale,
            lighting.shadow_samples,
        ))
    }
}

impl Drop for Preview {
    fn drop(&mut self) {
        self.shared.mailbox.lock().unwrap().closed = true;
        self.shared.work.notify_one();
    }
}

struct HiddenWindow(HWND);

impl Drop for HiddenWindow {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.0);
        }
    }
}

fn run_worker(shared: &Shared, ui: &egui::Context) -> Result<(), String> {
    let error = |detail: &dyn std::fmt::Display| format!("3D preview renderer: {detail}");
    // This window and its context belong to this thread. Only CPU pixels cross to the UI.
    let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
    let class = [83_u16, 84, 65, 84, 73, 67, 0]; // Win32 STATIC class.
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            class.as_ptr(),
            WS_POPUP,
            0,
            0,
            1,
            1,
            0,
            0,
            instance,
            std::ptr::null(),
        )
    };
    let hwnd = NonZeroIsize::new(hwnd).ok_or_else(|| error(&std::io::Error::last_os_error()))?;
    let _window = HiddenWindow(hwnd.get());
    let mut handle = Win32WindowHandle::new(hwnd);
    handle.hinstance = NonZeroIsize::new(instance);
    let handle = RawWindowHandle::Win32(handle);
    let display_handle = RawDisplayHandle::Windows(WindowsDisplayHandle::new());
    let display = unsafe { Display::new(display_handle, DisplayApiPreference::Wgl(Some(handle))) }
        .map_err(|e| error(&e))?;
    let template = ConfigTemplateBuilder::new()
        .compatible_with_native_window(handle)
        .build();
    let config = unsafe { display.find_configs(template) }
        .map_err(|e| error(&e))?
        .next()
        .ok_or_else(|| error(&"no OpenGL pixel format"))?;
    let attributes = ContextAttributesBuilder::new()
        .with_profile(GlProfile::Core)
        .with_context_api(ContextApi::OpenGl(Some(Version::new(3, 3))))
        .build(Some(handle));
    let context = unsafe { display.create_context(&config, &attributes) }.map_err(|e| error(&e))?;
    let one = NonZeroU32::MIN;
    let attributes = SurfaceAttributesBuilder::<WindowSurface>::new().build(handle, one, one);
    let surface =
        unsafe { display.create_window_surface(&config, &attributes) }.map_err(|e| error(&e))?;
    let _current = context.make_current(&surface).map_err(|e| error(&e))?;
    let gl =
        unsafe { glow::Context::from_loader_function_cstr(|name| display.get_proc_address(name)) };
    render_frames(&gl, shared, ui).map_err(|e| error(&e))
}

fn render_frames(gl: &glow::Context, shared: &Shared, ui: &egui::Context) -> Result<(), String> {
    let mut renderer = Renderer::new(gl)?;
    let mut scene = None::<Arc<Scene>>;
    let mut environment = None::<Arc<HdrImage>>;
    let color;
    let depth;
    let max_size;
    unsafe {
        let fbo = gl.create_framebuffer()?;
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
        color = gl.create_renderbuffer()?;
        depth = gl.create_renderbuffer()?;
        max_size = gl.get_parameter_i32(glow::MAX_RENDERBUFFER_SIZE).max(1);
        gl.enable(glow::SCISSOR_TEST);
        gl.clear_color(35.0 / 255.0, 40.0 / 255.0, 49.0 / 255.0, 1.0);
    }
    let mut buffer_size = [0; 2];
    'frames: loop {
        let request = {
            let mailbox = shared.mailbox.lock().unwrap();
            let mut mailbox = shared
                .work
                .wait_while(mailbox, |state| !state.closed && state.pending.is_none())
                .unwrap();
            if mailbox.closed {
                break;
            }
            mailbox.pending.take().unwrap()
        };
        if scene
            .as_ref()
            .is_none_or(|current| !Arc::ptr_eq(current, &request.scene))
        {
            renderer.upload(gl, &request.scene.vertices)?;
            scene = Some(request.scene.clone());
        }
        if let Some(image) = &request.environment
            && environment
                .as_ref()
                .is_none_or(|current| !Arc::ptr_eq(current, image))
        {
            renderer.upload_environment(gl, image);
            environment = Some(image.clone());
        }
        let lighting = request.lighting;
        let warning = if lighting.environment
            && (lighting.ray_reflections || lighting.soft_shadows || lighting.floor_shadows)
        {
            renderer
                .prepare_ray_mesh(gl, &request.scene.pick_mesh, &request.scene.vertices)
                .map(str::to_owned)
        } else {
            None
        };
        let scale = (max_size as f32 / request.size[0].max(request.size[1]) as f32).min(1.0);
        let size = request
            .size
            .map(|side| (side as f32 * scale).round().max(1.0) as i32);
        let [width, height] = size;
        if size != buffer_size {
            unsafe {
                for _ in 0..16 {
                    if gl.get_error() == glow::NO_ERROR {
                        break;
                    }
                }
                for (buffer, format, attachment) in [
                    (color, glow::RGBA8, glow::COLOR_ATTACHMENT0),
                    (depth, glow::DEPTH_COMPONENT24, glow::DEPTH_ATTACHMENT),
                ] {
                    gl.bind_renderbuffer(glow::RENDERBUFFER, Some(buffer));
                    gl.renderbuffer_storage(glow::RENDERBUFFER, format, width, height);
                    gl.framebuffer_renderbuffer(
                        glow::FRAMEBUFFER,
                        attachment,
                        glow::RENDERBUFFER,
                        Some(buffer),
                    );
                }
                let error = gl.get_error();
                if error != glow::NO_ERROR
                    || gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE
                {
                    buffer_size = [0; 2];
                    shared.mailbox.lock().unwrap().result = Some(Err(format!(
                        "Could not allocate the {width} × {height} preview. Lower the idle or minimum preview resolution."
                    )));
                    ui.request_repaint();
                    continue;
                }
            }
            buffer_size = size;
        }
        let mut pixels = vec![0; width as usize * height as usize * 4];
        unsafe {
            gl.viewport(0, 0, width, height);
            let band = if lighting.environment
                && (lighting.soft_shadows || lighting.floor_shadows || lighting.ray_reflections)
            {
                96
            } else {
                height
            };
            for y in (0..height).step_by(band as usize) {
                {
                    let mailbox = shared.mailbox.lock().unwrap();
                    if mailbox.closed {
                        break 'frames;
                    }
                    if request.refinement != 0 && mailbox.pending.is_some() {
                        continue 'frames;
                    }
                }
                gl.scissor(0, y, width, band.min(height - y));
                gl.clear(glow::COLOR_BUFFER_BIT);
                renderer.paint(gl, &request.scene.cameras[request.camera], lighting);
                // Bound individual GPU submissions so the UI context can make progress.
                gl.finish();
            }
            if shared.mailbox.lock().unwrap().closed {
                break;
            }
            gl.read_pixels(
                0,
                0,
                width,
                height,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut pixels)),
            );
        }
        let pixels = pixels
            .chunks_exact(width as usize * 4)
            .rev()
            .flat_map(|row| {
                row.as_chunks::<4>()
                    .0
                    .iter()
                    .map(|p| egui::Color32::from_rgb(p[0], p[1], p[2]))
            })
            .collect();
        let image = egui::ColorImage::new(size.map(|v| v as usize), pixels);
        shared.mailbox.lock().unwrap().result = Some(Ok(Frame {
            request,
            image,
            warning,
        }));
        ui.request_repaint();
    }
    renderer.destroy(gl);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_quality_steps_from_the_minimum_and_respects_user_limits() {
        let settings = Lighting::default();
        for (stage, (samples, scale)) in [(4, 50), (8, 75), (16, 100), (32, 100)]
            .into_iter()
            .enumerate()
        {
            let quality = refined_lighting(settings, stage);
            assert_eq!(
                (quality.shadow_samples, quality.preview_scale),
                (samples, scale)
            );
        }
        let limited = Lighting {
            shadow_samples: 8,
            idle_shadow_samples: 16,
            preview_scale: 25,
            idle_preview_scale: 75,
            ..settings
        };
        let quality = refined_lighting(limited, IDLE_AFTER.len());
        assert_eq!((quality.shadow_samples, quality.preview_scale), (16, 75));
        assert!(refined_lighting(limited, 0) == limited);
    }
}
