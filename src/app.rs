use crate::{
    camera::Camera,
    environment::HdrImage,
    hdr_editor::Editor,
    light_paint::LightPaint,
    preview::{Preview, Request},
    project::{self, Archive, LoadedModel, LoadedProject, ModelState, Snapshot, State},
    renderer::Lighting,
    scene::Scene,
    settings::Settings,
};
use eframe::egui;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    time::Instant,
};

enum FileResult {
    Imported(LoadedModel, f64),
    Loaded(PathBuf, LoadedProject, f64),
    Saved(PathBuf, Snapshot),
}

enum FileCommand {
    New,
    Save,
    Load,
    ReloadAlembic,
    RemoveModels,
    RemoveCamera,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InspectorTab {
    Lighting,
    FixedCamera,
    HdrEnvironment,
    File,
}

struct LightStroke {
    new_light: bool,
}

pub struct App {
    preview: Preview,
    show_preview: bool,
    environment: Option<Arc<HdrImage>>,
    scene: Option<Arc<Scene>>,
    file_job: Option<mpsc::Receiver<Result<FileResult, String>>>,
    loading_project: bool,
    archive: Option<Arc<Archive>>,
    project_path: Option<PathBuf>,
    saved: Snapshot,
    camera: usize,
    retained_cameras: Vec<Camera>,
    removed_camera_paths: Vec<String>,
    seconds: f64,
    applied_seconds: f64,
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
        let saved = Snapshot {
            state: State {
                lighting: editor.lighting,
                ..State::default()
            },
            model: None,
        };
        Ok(Self {
            preview: Preview::new(&cc.egui_ctx)?,
            show_preview: true,
            environment: None,
            scene: None,
            file_job: None,
            loading_project: false,
            archive: None,
            project_path: None,
            saved,
            camera: 0,
            retained_cameras: Vec::new(),
            removed_camera_paths: Vec::new(),
            seconds: 0.0,
            applied_seconds: 0.0,
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
    fn run_file_job(
        &mut self,
        ctx: &egui::Context,
        task: impl FnOnce() -> Result<FileResult, String> + Send + 'static,
    ) {
        let (tx, rx) = mpsc::channel();
        self.file_job = Some(rx);
        self.error = None;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(task());
            ctx.request_repaint();
        });
    }

    fn open(&mut self, path: PathBuf, ctx: &egui::Context) {
        if is_hdr(&path) {
            self.editor.import(path, ctx);
        } else if has_extension(&path, "lmo") {
            if self.confirm_replace_project() {
                self.load_project(path, ctx);
            }
        } else {
            let sample = self.archive.as_ref().and_then(|archive| {
                (archive.original.as_ref() == Some(&path)).then_some(self.seconds)
            });
            self.run_file_job(ctx, move || {
                let start = Instant::now();
                let model = Archive::import(&path, sample)?;
                Ok(FileResult::Imported(model, start.elapsed().as_secs_f64()))
            });
        }
    }

    fn snapshot(&self) -> Snapshot {
        let model = self
            .archive
            .as_ref()
            .zip(self.scene.as_ref())
            .map(|(archive, scene)| {
                let state = ModelState {
                    name: scene.path.file_name().unwrap_or_default().into(),
                    seconds: self.applied_seconds,
                    selected_camera: self.camera,
                    cameras: scene.cameras.clone(),
                    removed_camera_paths: self.removed_camera_paths.clone(),
                };
                (Arc::clone(archive), state)
            });
        Snapshot {
            state: State {
                document: self.editor.project_document(),
                lighting: self.editor.lighting,
                export_width: self.editor.project_export_width(),
                show_preview: self.show_preview,
                light_paint: self.light_paint,
                retained_cameras: self.retained_cameras.clone(),
                retained_camera: if model.is_none() { self.camera } else { 0 },
            },
            model,
        }
    }

    fn confirm_replace_project(&self) -> bool {
        (self.snapshot() == self.saved && !self.editor.is_importing())
            || rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Warning)
                .set_title("Discard unsaved project changes?")
                .set_description("Save the current project before replacing it if you want to keep these changes.")
                .set_buttons(rfd::MessageButtons::YesNo)
                .show() == rfd::MessageDialogResult::Yes
    }

    fn load_project(&mut self, path: PathBuf, ctx: &egui::Context) {
        self.loading_project = true;
        self.run_file_job(ctx, move || {
            let start = Instant::now();
            let project =
                project::load(&path).map_err(|err| format!("Could not load project: {err}"))?;
            Ok(FileResult::Loaded(
                path,
                project,
                start.elapsed().as_secs_f64(),
            ))
        });
    }

    fn save_project(&mut self, ctx: &egui::Context) {
        let file_name = self
            .project_path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|name| name.to_str());
        let mut dialog = rfd::FileDialog::new()
            .add_filter("HDR STUDIO project", &["lmo"])
            .set_file_name(file_name.unwrap_or("NewProject.lmo"));
        if let Some(directory) = self.project_path.as_ref().and_then(|p| p.parent()) {
            dialog = dialog.set_directory(directory);
        }
        let Some(mut path) = dialog.save_file() else {
            return;
        };
        if !has_extension(&path, "lmo") {
            let Some(name) = path.file_name() else {
                return;
            };
            let mut name = name.to_os_string();
            name.push(".lmo");
            path.set_file_name(name);
            if path.exists()
                && rfd::MessageDialog::new()
                    .set_title("Replace existing project?")
                    .set_description(path.display().to_string())
                    .set_buttons(rfd::MessageButtons::YesNo)
                    .show()
                    != rfd::MessageDialogResult::Yes
            {
                return;
            }
        }
        let snapshot = self.snapshot();
        self.run_file_job(ctx, move || {
            project::save(&path, &snapshot)
                .map_err(|err| format!("Could not save project: {err}"))?;
            Ok(FileResult::Saved(path, snapshot))
        });
    }

    fn set_project_path(&mut self, path: Option<PathBuf>, ctx: &egui::Context) {
        self.project_path = path;
        let stem = self.project_path.as_ref().and_then(|path| path.file_stem());
        let title = stem.map_or_else(|| "NewProject".into(), |stem| stem.to_string_lossy());
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
            "HDR STUDIO — {title}"
        )));
    }

    fn apply_model(&mut self, model: Option<LoadedModel>, elapsed: f64) {
        self.invalidate_scene_preview();
        self.elapsed = elapsed;
        self.retained_cameras.clear();
        if let Some(model) = model {
            self.scene = Some(Arc::new(model.scene));
            self.archive = Some(model.archive);
            self.seconds = model.seconds;
            self.applied_seconds = model.seconds;
            self.camera = model.selected_camera;
            self.removed_camera_paths = model.removed_camera_paths;
        } else {
            self.scene = None;
            self.archive = None;
            self.seconds = 0.0;
            self.applied_seconds = 0.0;
            self.camera = 0;
            self.removed_camera_paths.clear();
        }
    }

    fn invalidate_scene_preview(&mut self) {
        self.preview.suspend();
        self.placement_status.clear();
        self.light_stroke = None;
    }

    fn cameras(&self) -> &[Camera] {
        self.scene
            .as_ref()
            .map_or(&self.retained_cameras, |scene| &scene.cameras)
    }

    fn selected_camera(&self) -> Option<&Camera> {
        self.cameras()
            .get(self.camera)
            .filter(|camera| !self.removed_camera_paths.contains(&camera.path))
    }

    fn remove_models(&mut self) {
        let selected = self.selected_camera().map(|camera| camera.path.clone());
        let cameras: Vec<_> = self
            .cameras()
            .iter()
            .filter(|camera| !self.removed_camera_paths.contains(&camera.path))
            .cloned()
            .collect();
        self.apply_model(None, 0.0);
        self.camera = cameras
            .iter()
            .position(|camera| Some(&camera.path) == selected.as_ref())
            .unwrap_or(0);
        self.retained_cameras = cameras;
    }

    fn remove_camera(&mut self) {
        let Some(camera) = self.selected_camera() else {
            return;
        };
        if self.scene.is_some() {
            self.removed_camera_paths.push(camera.path.clone());
            self.camera = self
                .cameras()
                .iter()
                .enumerate()
                .cycle()
                .skip(self.camera + 1)
                .take(self.cameras().len())
                .find(|(_, camera)| !self.removed_camera_paths.contains(&camera.path))
                .map_or(0, |(index, _)| index);
        } else {
            self.retained_cameras.remove(self.camera);
            self.camera = self
                .camera
                .min(self.retained_cameras.len().saturating_sub(1));
        }
        self.invalidate_scene_preview();
    }

    fn apply_project(&mut self, project: LoadedProject, elapsed: f64) {
        let State {
            document,
            lighting,
            export_width,
            show_preview,
            light_paint,
            retained_cameras,
            retained_camera,
        } = project.state;
        self.editor
            .replace_project(document, lighting, export_width);
        self.show_preview = show_preview;
        self.editor.set_preview_visible(show_preview);
        self.light_paint = light_paint;
        self.environment = None;
        self.apply_model(project.model, elapsed);
        if self.scene.is_none() {
            self.retained_cameras = retained_cameras;
            self.camera = retained_camera;
        }
        self.error = None;
        self.saved = self.snapshot();
    }

    fn poll_file_job(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.file_job else {
            return;
        };
        let result = match job.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("File operation stopped unexpectedly".into())
            }
        };
        self.file_job = None;
        self.loading_project = false;
        match result {
            Ok(FileResult::Imported(model, elapsed)) => self.apply_model(Some(model), elapsed),
            Ok(FileResult::Loaded(path, project, elapsed)) => {
                self.apply_project(project, elapsed);
                self.set_project_path(Some(path), ctx);
            }
            Ok(FileResult::Saved(path, snapshot)) => {
                self.saved = snapshot;
                self.set_project_path(Some(path), ctx);
            }
            Err(error) => self.error = Some(error),
        }
    }

    fn file_command(&mut self, command: FileCommand, ctx: &egui::Context) {
        match command {
            FileCommand::New if self.confirm_replace_project() => {
                self.apply_project(
                    LoadedProject {
                        state: State::default(),
                        model: None,
                    },
                    0.0,
                );
                self.set_project_path(None, ctx);
            }
            FileCommand::Save => self.save_project(ctx),
            FileCommand::Load => {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("HDR STUDIO project", &["lmo"])
                    .pick_file()
                    && self.confirm_replace_project()
                {
                    self.load_project(path, ctx);
                }
            }
            FileCommand::ReloadAlembic => {
                if let Some(archive) = self.archive.clone() {
                    let seconds = self.seconds;
                    let removed_camera_paths = self.removed_camera_paths.clone();
                    let selected_path = self.selected_camera().map(|camera| camera.path.clone());
                    self.run_file_job(ctx, move || {
                        let start = Instant::now();
                        let mut model = archive.reload(seconds)?;
                        let cameras = &model.scene.cameras;
                        model.selected_camera = cameras
                            .iter()
                            .position(|camera| Some(&camera.path) == selected_path.as_ref())
                            .or_else(|| {
                                cameras
                                    .iter()
                                    .position(|camera| !removed_camera_paths.contains(&camera.path))
                            })
                            .unwrap_or(0);
                        model.removed_camera_paths = removed_camera_paths;
                        Ok(FileResult::Imported(model, start.elapsed().as_secs_f64()))
                    });
                }
            }
            FileCommand::RemoveModels => self.remove_models(),
            FileCommand::RemoveCamera => self.remove_camera(),
            FileCommand::New => {}
        }
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
        if self.scene.is_none() && self.retained_cameras.is_empty() {
            ui.label("Import an .abc file containing polygon meshes and a camera.");
            return;
        }
        let previous = self.camera;
        let mut selected = self.camera;
        egui::ComboBox::from_id_salt("camera")
            .selected_text(
                self.selected_camera()
                    .map_or("No camera", |c| c.path.as_str()),
            )
            .show_ui(ui, |ui| {
                for (index, camera) in self.cameras().iter().enumerate() {
                    if !self.removed_camera_paths.contains(&camera.path) {
                        ui.selectable_value(&mut selected, index, &camera.path);
                    }
                }
            });
        self.camera = selected;
        if self.camera != previous {
            self.light_stroke = None;
        }
        if let Some(camera) = self.selected_camera() {
            ui.label(format!(
                "{:.2} mm · aspect {:.6}",
                camera.focal, camera.aspect
            ));
            ui.small(&camera.source);
        }
        let Some(scene) = &self.scene else {
            ui.small("No models loaded.");
            return;
        };
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
        self.poll_file_job(ctx);
        if let Some(path) = self.initial.take() {
            self.open(path, ctx);
        }
        if self.loading_project {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Loading project…");
                });
            });
            return;
        }
        let (before, restored) = self.editor.begin_edit_frame(ctx);
        if restored {
            self.light_stroke = None;
        }
        self.editor.poll();
        let mut open_path = None;
        let mut command = None;
        let available = self.file_job.is_none();
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("HDR STUDIO");
                ui.separator();
                ui.add_enabled_ui(available, |ui| {
                    ui.menu_button("File", |ui| {
                        if ui.button("New Project").clicked() {
                            command = Some(FileCommand::New);
                            ui.close();
                        }
                        if ui
                            .add_enabled(
                                !self.editor.is_importing(),
                                egui::Button::new("Save Project…"),
                            )
                            .clicked()
                        {
                            command = Some(FileCommand::Save);
                            ui.close();
                        }
                        if ui.button("Load Project…").clicked() {
                            command = Some(FileCommand::Load);
                            ui.close();
                        }
                    });
                });
                ui.separator();
                if ui
                    .add_enabled(available, egui::Button::new("Import Alembic…"))
                    .clicked()
                {
                    open_path = rfd::FileDialog::new()
                        .add_filter("Alembic Ogawa", &["abc"])
                        .pick_file();
                }
                if ui
                    .add_enabled(
                        self.scene.is_some() && available,
                        egui::Button::new("Reload"),
                    )
                    .clicked()
                {
                    command = Some(FileCommand::ReloadAlembic);
                }
                ui.separator();
                ui.label("Sample (seconds)");
                ui.add_enabled(
                    available,
                    egui::DragValue::new(&mut self.seconds).speed(0.01),
                );
                ui.label("applied on Import / Reload");
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
                if !available {
                    ui.spinner();
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
                        (InspectorTab::File, "File"),
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
                        InspectorTab::File => {
                            let size = egui::vec2(ui.available_width(), 28.0);
                            self.editor.file_controls(ui, size);
                            ui.separator();
                            ui.strong("Scene");
                            if ui.add_enabled(available && self.scene.is_some(),
                                egui::Button::new("Remove All Models").min_size(size)).clicked()
                            {
                                command = Some(FileCommand::RemoveModels);
                            }
                            if ui.add_enabled(available && self.selected_camera().is_some(),
                                egui::Button::new("Remove Selected Camera").min_size(size)).clicked()
                            {
                                command = Some(FileCommand::RemoveCamera);
                            }
                        }
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
                let camera = self.selected_camera().cloned();
                ui.horizontal_wrapped(|ui| {
                    ui.label("Light placement");
                    let mut changed = ui
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
                    let response = ui
                        .interact(rect, ui.id().with("light-placement"), egui::Sense::click_and_drag())
                        .on_hover_cursor(egui::CursorIcon::Crosshair);
                    self.paint_light(ui, rect, response, &camera);
                    ui.painter()
                        .rect_filled(rect, 0.0, egui::Color32::from_rgb(35, 40, 49));
                    if let Some(environment) = self.environment.clone() {
                        self.preview.show(ui, rect, Request {
                            scene,
                            camera: self.camera,
                            environment: Some(environment),
                            lighting: Lighting { canvas_exposure_ev: 0.0, ..self.editor.lighting },
                            needs_pick_mesh: true,
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
                        ui.label(if self.scene.is_none() {
                            "Import an Alembic model to preview the scene."
                        } else {
                            "Import a camera to preview its framing."
                        });
                    });
                }
            });
        }
        if available
            && command.is_none()
            && let Some(path) =
                ctx.input(|i| i.raw.dropped_files.first().and_then(|f| f.path.clone()))
        {
            open_path = Some(path);
        }
        self.editor.finish_edit_frame(ctx, before);
        if let Some(command) = command {
            self.file_command(command, ctx);
        } else if let Some(path) = open_path {
            self.open(path, ctx);
        }
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

fn is_hdr(path: &Path) -> bool {
    has_extension(path, "hdr") || has_extension(path, "exr")
}

fn has_extension(path: &Path, extension: &str) -> bool {
    path.extension()
        .and_then(|v| v.to_str())
        .is_some_and(|v| v.eq_ignore_ascii_case(extension))
}
