use crate::{
    camera::Camera,
    environment::HdrImage,
    hdr_editor::Editor,
    light_paint::LightPaint,
    preview::{Preview, Request},
    scene::{self, Scene},
    settings::Settings,
};
use eframe::egui;
use std::{
    path::PathBuf,
    sync::{Arc, mpsc},
    time::Instant,
};

type LoadResult = Result<(Scene, f64), String>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InspectorTab {
    Lighting,
    FixedCamera,
    HdrEnvironment,
}

struct LightStroke {
    new_light: bool,
}

pub struct App {
    preview: Preview,
    show_preview: bool,
    environment: Option<Arc<HdrImage>>,
    scene: Option<Arc<Scene>>,
    loading: Option<mpsc::Receiver<LoadResult>>,
    camera: usize,
    seconds: f64,
    error: Option<String>,
    elapsed: f64,
    initial: Option<PathBuf>,
    editor: Editor,
    settings: Option<Settings>,
    inspector_tab: InspectorTab,
    light_paint: LightPaint,
    light_stroke: Option<LightStroke>,
    placement_status: String,
}
impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, initial: Option<PathBuf>) -> Result<Self, String> {
        cc.egui_ctx.set_theme(egui::ThemePreference::Dark);
        let settings = Settings::open();
        let mut editor = Editor::default();
        if let Some(settings) = &settings {
            editor.lighting = settings.lighting();
            editor.lighting.default_light = true;
        }
        Ok(Self {
            preview: Preview::new(&cc.egui_ctx)?,
            show_preview: true,
            environment: None,
            scene: None,
            loading: None,
            camera: 0,
            seconds: 0.0,
            error: None,
            elapsed: 0.0,
            initial,
            editor,
            settings,
            inspector_tab: InspectorTab::Lighting,
            light_paint: LightPaint::default(),
            light_stroke: None,
            placement_status: String::new(),
        })
    }
    fn open(&mut self, path: PathBuf, ctx: &egui::Context) {
        if self.scene.as_ref().is_none_or(|s| s.path != path) {
            match scene::default_sample(&path) {
                Ok(seconds) => self.seconds = seconds,
                Err(e) => {
                    self.error = Some(e);
                    return;
                }
            }
        }
        let (tx, rx) = mpsc::channel();
        self.loading = Some(rx);
        self.error = None;
        let seconds = self.seconds;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let start = Instant::now();
            let result = scene::load(&path, seconds).map(|s| (s, start.elapsed().as_secs_f64()));
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    fn paint_light(
        &mut self,
        ui: &egui::Ui,
        rect: egui::Rect,
        response: egui::Response,
        camera: &Camera,
    ) {
        let (down, press_shift, focused) = ui.input(|i| {
            let press_shift = i.events.iter().find_map(|event| match event {
                egui::Event::PointerButton {
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers,
                    ..
                } => Some(modifiers.shift),
                _ => None,
            });
            (i.pointer.primary_down(), press_shift, i.focused)
        });
        if !focused {
            self.light_stroke = None;
            return;
        }
        if self.light_stroke.is_none()
            && let Some(new_light) = press_shift
            && (response.is_pointer_button_down_on()
                || response.clicked_by(egui::PointerButton::Primary)
                || response.drag_stopped_by(egui::PointerButton::Primary))
        {
            self.light_stroke = Some(LightStroke { new_light });
        }
        if let Some(stroke) = &mut self.light_stroke
            && let Some(scene) = &self.scene
            && let Some(pick_mesh) = scene.pick_mesh.get()
        {
            let positions = ui.input(|i| {
                let mut positions = Vec::new();
                for event in &i.events {
                    match *event {
                        egui::Event::PointerMoved(pos) => positions.push(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            ..
                        } => {
                            if pressed {
                                positions.clear();
                            }
                            positions.push(pos);
                            if !pressed {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                positions
            });
            if !positions.is_empty() {
                // A frame can contain several moves, including one that left the mesh.
                let direction = positions
                    .into_iter()
                    .rev()
                    .filter(|p| rect.contains(*p))
                    .find_map(|pointer| {
                        let ndc = glam::DVec2::new(
                            (2.0 * (pointer.x - rect.left()) / rect.width() - 1.0) as f64,
                            (1.0 - 2.0 * (pointer.y - rect.top()) / rect.height()) as f64,
                        );
                        self.light_paint
                            .direction(camera, ndc, pick_mesh, &scene.vertices)
                    });
                self.placement_status = match direction {
                    Some(direction) => {
                        match self
                            .editor
                            .place_light(direction.as_vec3(), stroke.new_light)
                        {
                            Some(name) => {
                                stroke.new_light = false;
                                ui.ctx().request_repaint();
                                format!("Placed {name}")
                            }
                            None => "Light layer limit reached".to_owned(),
                        }
                    }
                    None => "No surface under the pointer".to_owned(),
                };
            }
        }
        if !down {
            self.light_stroke = None;
        }
    }

    fn camera_controls(&mut self, ui: &mut egui::Ui) {
        let Some(scene) = &self.scene else {
            ui.label("Open an .abc file containing polygon meshes and a camera.");
            return;
        };
        let previous = self.camera;
        egui::ComboBox::from_id_salt("camera")
            .selected_text(
                scene
                    .cameras
                    .get(self.camera)
                    .map_or("No camera", |c| c.path.as_str()),
            )
            .show_ui(ui, |ui| {
                for (index, camera) in scene.cameras.iter().enumerate() {
                    ui.selectable_value(&mut self.camera, index, &camera.path);
                }
            });
        if self.camera != previous {
            self.light_stroke = None;
        }
        if let Some(camera) = scene.cameras.get(self.camera) {
            ui.label(format!(
                "{:.2} mm · aspect {:.6}",
                camera.focal, camera.aspect
            ));
            ui.small(&camera.source);
        }
        ui.separator();
        ui.label(scene.path.file_name().unwrap_or_default().to_string_lossy());
        ui.label(format!(
            "{} meshes · {} triangles",
            scene.meshes,
            scene.vertices.len() / 3
        ));
        ui.label(format!("Loaded in {:.3} s", self.elapsed));
        if !scene.warnings.is_empty() {
            ui.separator();
            ui.heading("Import notes");
            for warning in &scene.warnings {
                ui.colored_label(egui::Color32::YELLOW, warning);
            }
        }
    }
}
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let (before, restored) = self.editor.begin_edit_frame(ctx);
        if restored {
            self.light_stroke = None;
        }
        self.editor.poll();
        if let Some(path) = self.initial.take() {
            if is_hdr(&path) {
                self.editor.import(path, ctx);
            } else {
                self.open(path, ctx);
            }
        }
        if let Some(rx) = &self.loading {
            match rx.try_recv() {
                Ok(result) => {
                    self.loading = None;
                    match result {
                        Ok((scene, elapsed)) => {
                            self.light_stroke = None;
                            self.camera = 0;
                            self.elapsed = elapsed;
                            self.scene = Some(Arc::new(scene));
                        }
                        Err(e) => self.error = Some(e),
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.loading = None;
                    self.error = Some("ABC loading thread stopped unexpectedly".into());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let mut open_path = None;
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("HDR STUDIO");
                ui.separator();
                if ui
                    .add_enabled(self.loading.is_none(), egui::Button::new("Open Alembic…"))
                    .clicked()
                {
                    open_path = rfd::FileDialog::new()
                        .add_filter("Alembic Ogawa", &["abc"])
                        .pick_file();
                }
                if ui
                    .add_enabled(
                        self.scene.is_some() && self.loading.is_none(),
                        egui::Button::new("Reload"),
                    )
                    .clicked()
                {
                    open_path = self.scene.as_ref().map(|s| s.path.clone());
                }
                ui.separator();
                ui.label("Sample (seconds)");
                ui.add_enabled(
                    self.loading.is_none(),
                    egui::DragValue::new(&mut self.seconds).speed(0.01),
                );
                ui.label("applied on Open / Reload");
                ui.separator();
                if ui.checkbox(&mut self.show_preview, "3D Preview").changed() {
                    self.editor.set_preview_visible(self.show_preview);
                    if !self.show_preview {
                        self.preview.suspend();
                        self.environment = None;
                        self.light_stroke = None;
                        self.placement_status.clear();
                    }
                }
                if self.loading.is_some() {
                    ui.spinner();
                    ui.label("Loading…");
                }
            });
        });
        if self.show_preview {
            self.preview.poll(ctx);
        }
        egui::SidePanel::right("inspector")
            .default_width(320.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for (tab, label) in [
                        (InspectorTab::Lighting, "Lighting"),
                        (InspectorTab::FixedCamera, "Fixed camera"),
                        (InspectorTab::HdrEnvironment, "HDR environment"),
                    ] {
                        ui.selectable_value(&mut self.inspector_tab, tab, label);
                    }
                });
                ui.separator();
                let scroll_source = if self.inspector_tab == InspectorTab::HdrEnvironment {
                    egui::scroll_area::ScrollSource::SCROLL_BAR | egui::scroll_area::ScrollSource::MOUSE_WHEEL
                } else {
                    egui::scroll_area::ScrollSource::ALL
                };
                egui::ScrollArea::vertical()
                    .id_salt(egui::Id::new("inspector-scroll").with(self.inspector_tab as u8))
                    .scroll_source(scroll_source)
                    .show(ui, |ui| match self.inspector_tab {
                        InspectorTab::Lighting => {
                            self.editor.lighting_controls(ui);
                            if let Some(([width, height], scale, samples)) = self.preview.quality() {
                                ui.small(format!("Preview: {width} × {height} px ({scale}%) · {samples} shadow samples"));
                            }
                        }
                        InspectorTab::FixedCamera => self.camera_controls(ui),
                        InspectorTab::HdrEnvironment => self.editor.environment_controls(ui),
                    });
            });
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            let preview_warning = self.preview.warning.as_ref().filter(|_| self.show_preview);
            if let Some(error) = self.error.as_ref().or(preview_warning)
                .or(self.settings.as_ref().and_then(|settings| settings.error.as_ref())) {
                ui.colored_label(egui::Color32::LIGHT_RED, error);
            } else if self.show_preview {
                ui.label(
                    "Camera locked · HDR preview uses approximate environment shading · export is linear HDR",
                );
            } else {
                ui.label("Export is linear HDR");
            }
        });
        self.editor.canvas(ctx, !self.show_preview);
        if self.show_preview {
            egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(egui::Color32::from_rgb(14, 17, 22)))
            .show(ctx, |ui| {
                let camera: Option<Camera> = self
                    .scene
                    .as_ref()
                    .and_then(|s| s.cameras.get(self.camera))
                    .cloned();
                ui.horizontal_wrapped(|ui| {
                    ui.label("Light placement");
                    let mut changed = ui
                        .selectable_value(&mut self.light_paint, LightPaint::Off, "Off")
                        .changed();
                    changed |= ui
                        .selectable_value(
                            &mut self.light_paint,
                            LightPaint::Reflection,
                            "Reflection",
                        )
                        .on_hover_text(
                            "Click or drag to aim a reflection. Hold Shift at the start to add a light.",
                        )
                        .changed();
                    changed |= ui
                        .selectable_value(&mut self.light_paint, LightPaint::Diffuse, "Diffuse")
                        .on_hover_text(
                            "Click or drag to follow the surface normal. Hold Shift at the start to add a light.",
                        )
                        .changed();
                    if changed {
                        self.light_stroke = None;
                        self.placement_status.clear();
                    }
                    ui.small(&self.placement_status);
                });
                let space = ui.available_rect_before_wrap();
                if let Some(camera) = camera && let Some(scene) = self.scene.clone() {
                    let size = if space.width() / space.height() > camera.aspect as f32 {
                        egui::vec2(space.height() * camera.aspect as f32, space.height())
                    } else {
                        egui::vec2(space.width(), space.width() / camera.aspect as f32)
                    };
                    let rect = egui::Rect::from_center_size(space.center(), size);
                    ui.allocate_rect(space, egui::Sense::hover());
                    if self.light_paint != LightPaint::Off {
                        let response = ui
                            .interact(rect, ui.id().with("light-placement"), egui::Sense::click_and_drag())
                            .on_hover_cursor(egui::CursorIcon::Crosshair);
                        self.paint_light(ui, rect, response, &camera);
                    }
                    ui.painter()
                        .rect_filled(rect, 0.0, egui::Color32::from_rgb(35, 40, 49));
                    if let Some(environment) = self.environment.clone() {
                        self.preview.show(ui, rect, Request {
                            scene,
                            camera: self.camera,
                            environment: Some(environment),
                            lighting: self.editor.lighting,
                            needs_pick_mesh: self.light_paint != LightPaint::Off,
                            size: [0; 2],
                            refinement: 0,
                        });
                    }
                    ui.painter().rect_stroke(
                        rect,
                        0.0,
                        egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(100, 116, 137)),
                        egui::StrokeKind::Inside,
                    );
                } else {
                    self.light_stroke = None;
                    ui.centered_and_justified(|ui| {
                        ui.label("Import a camera to preview its framing.");
                    });
                }
            });
        }
        if self.loading.is_none()
            && let Some(path) =
                ctx.input(|i| i.raw.dropped_files.first().and_then(|f| f.path.clone()))
        {
            open_path = Some(path);
        }
        if let Some(path) = open_path {
            if is_hdr(&path) {
                self.editor.import(path, ctx);
            } else {
                self.open(path, ctx);
            }
        }
        self.editor.finish_edit_frame(ctx, before);
        if let Some(image) = self.editor.refresh(ctx) {
            self.environment = Some(image);
        }
        if !ctx.input(|input| input.pointer.any_down())
            && let Some(settings) = &mut self.settings
        {
            settings.save(self.editor.lighting);
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Some(settings) = &mut self.settings {
            settings.save(self.editor.lighting);
        }
    }
}

fn is_hdr(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|v| v.to_str())
        .is_some_and(|v| v.eq_ignore_ascii_case("hdr") || v.eq_ignore_ascii_case("exr"))
}
