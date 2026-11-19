use crate::{
    environment::{
        BlendMode, BoxInterpolation, Document, FalloffCurve, FalloffPreset, HdrImage, Light,
        LightMapping, LightShape, PREVIEW_HEIGHT, PREVIEW_WIDTH,
    },
    renderer::{Lighting, PREVIEW_SCALE_OPTIONS, SHADOW_SAMPLE_OPTIONS},
};
use eframe::egui;
use std::{
    path::PathBuf,
    sync::{Arc, mpsc},
};

fn falloff_controls(
    ui: &mut egui::Ui,
    curve: &mut FalloffCurve,
    directional: bool,
    box_ramp: bool,
) -> bool {
    const MIN_SPACING: f32 = 0.004;
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label("Preset");
        egui::ComboBox::from_id_salt("falloff-preset")
            .selected_text(curve.preset.map_or("Custom", FalloffPreset::label))
            .show_ui(ui, |ui| {
                for preset in FalloffPreset::ALL {
                    if ui
                        .selectable_label(curve.preset == Some(preset), preset.label())
                        .clicked()
                    {
                        *curve = FalloffCurve::from_preset(preset);
                        changed = true;
                    }
                }
            });
    });
    if box_ramp {
        ui.horizontal(|ui| {
            ui.label("Interpolation");
            for (mode, label) in [
                (BoxInterpolation::Linear, "Linear"),
                (BoxInterpolation::Smooth, "Smooth"),
            ] {
                if ui
                    .selectable_value(&mut curve.box_interpolation, mode, label)
                    .changed()
                {
                    curve.preset = None;
                    changed = true;
                }
            }
        });
    }

    let (area, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 108.0),
        egui::Sense::click_and_drag(),
    );
    let graph = area.shrink(8.0);
    let screen = |[x, y]: [f32; 2]| {
        egui::pos2(
            graph.left() + graph.width() * x,
            graph.bottom() - graph.height() * y,
        )
    };
    let coordinates = |p: egui::Pos2| {
        [
            ((p.x - graph.left()) / graph.width()).clamp(0.0, 1.0),
            ((graph.bottom() - p.y) / graph.height()).clamp(0.0, 1.0),
        ]
    };
    let hit = |points: &[[f32; 2]], position: egui::Pos2| {
        points
            .iter()
            .enumerate()
            .map(|(index, &point)| (index, screen(point).distance_sq(position)))
            .filter(|(_, distance)| *distance < 64.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(index, _)| index)
    };
    let drag_id = response.id.with("point-drag");
    if response.drag_started() {
        let origin = ui.input(|input| input.pointer.press_origin());
        ui.data_mut(|data| data.insert_temp(drag_id, origin.and_then(|p| hit(&curve.points, p))));
    }
    let dragged = ui.data(|data| data.get_temp::<Option<usize>>(drag_id).flatten());
    if let Some(pointer) = response.interact_pointer_pos() {
        if response.dragged()
            && let Some(index) = dragged.filter(|&index| index < curve.points.len())
        {
            let [pointer_x, y] = coordinates(pointer);
            let mut x = curve.points[index][0];
            if index > 0 && index + 1 < curve.points.len() {
                let lower = curve.points[index - 1][0] + MIN_SPACING;
                let upper = curve.points[index + 1][0] - MIN_SPACING;
                if lower <= upper {
                    x = pointer_x.clamp(lower, upper);
                }
            }
            curve.points[index] = [x, y];
            curve.preset = None;
            changed = true;
        } else if response.secondary_clicked()
            && let Some(index) = hit(&curve.points, pointer)
            && index > 0
            && index + 1 < curve.points.len()
        {
            curve.points.remove(index);
            curve.preset = None;
            changed = true;
        } else if response.double_clicked()
            && curve.points.len() < 16
            && hit(&curve.points, pointer).is_none()
            && graph.contains(pointer)
        {
            let point = coordinates(pointer);
            let index = curve.points.partition_point(|p| p[0] < point[0]);
            if index > 0
                && index < curve.points.len()
                && point[0] - curve.points[index - 1][0] >= MIN_SPACING
                && curve.points[index][0] - point[0] >= MIN_SPACING
            {
                curve.points.insert(index, point);
                curve.preset = None;
                changed = true;
            }
        }
    }

    let painter = ui.painter();
    let guide = egui::Stroke::new(1.0_f32, ui.visuals().widgets.inactive.bg_stroke.color);
    painter.rect_filled(area, 5.0, ui.visuals().extreme_bg_color);
    for step in 0..=4 {
        let t = step as f32 / 4.0;
        painter.hline(graph.x_range(), screen([0.0, t]).y, guide);
        painter.vline(screen([t, 0.0]).x, graph.y_range(), guide);
    }
    let line: Vec<_> = (0..=96)
        .map(|step| {
            let x = step as f32 / 96.0;
            screen([x, curve.sample(x, box_ramp)])
        })
        .collect();
    let accent = egui::Color32::from_rgb(255, 194, 77);
    painter.add(egui::Shape::line(line, egui::Stroke::new(2.0_f32, accent)));
    for &point in &curve.points {
        painter.circle_filled(screen(point), 4.0, accent);
    }
    ui.horizontal(|ui| {
        ui.small(if directional { "Start" } else { "Core" });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.small(if directional { "End" } else { "Edge" });
        });
    });
    ui.small("Drag points · double-click to add · right-click to remove");
    changed
}

fn rotation_dial(ui: &mut egui::Ui, angle: &mut f32, hint: &str) -> bool {
    let (dial, response) =
        ui.allocate_exact_size(egui::Vec2::splat(62.0), egui::Sense::click_and_drag());
    let center = dial.center();
    let radius = dial.width() * 0.5 - 3.0;
    let mut changed = false;
    if (response.clicked() || response.dragged() || response.drag_started())
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let offset = pointer - center;
        if offset.length_sq() > 9.0 {
            *angle = offset.y.atan2(offset.x).to_degrees().rem_euclid(360.0);
            changed = true;
        }
    }

    let painter = ui.painter();
    let visuals = ui.visuals();
    let outline = visuals.widgets.inactive.bg_stroke;
    painter.circle(center, radius, visuals.extreme_bg_color, outline);
    for direction in [
        egui::vec2(-1.0, 0.0),
        egui::vec2(1.0, 0.0),
        egui::vec2(0.0, -1.0),
        egui::vec2(0.0, 1.0),
    ] {
        painter.line_segment(
            [
                center + direction * (radius - 6.0),
                center + direction * radius,
            ],
            outline,
        );
    }
    let (sin, cos) = angle.to_radians().sin_cos();
    painter.arrow(
        center,
        egui::vec2(cos, sin) * (radius - 7.0),
        egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(255, 194, 77)),
    );
    response.on_hover_text(hint);
    changed
}

fn rotation_control(ui: &mut egui::Ui, label: &str, angle: &mut f32, hint: &str) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        changed |= rotation_dial(ui, angle, hint);
        ui.vertical(|ui| {
            ui.label(label);
            changed |= ui
                .add(
                    egui::DragValue::new(angle)
                        .range(0.0..=360.0)
                        .speed(1.0)
                        .max_decimals(1)
                        .suffix("°"),
                )
                .changed();
        });
    });
    changed
}

fn mapping_hotspot_cut_controls(ui: &mut egui::Ui, light: &mut Light) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label("Mapping");
        changed |= ui
            .selectable_value(&mut light.mapping, LightMapping::Spherical, "3D")
            .changed();
        changed |= ui
            .selectable_value(&mut light.mapping, LightMapping::Flat, "Flat")
            .changed();
    });
    if light.shape == LightShape::Gradient {
        changed |= rotation_control(
            ui,
            "Gradient direction",
            &mut light.gradient_rotation,
            "The arrow points from the start of the curve toward its end.",
        );
    } else {
        changed |= hotspot_controls(ui, light);
    }

    changed |= ui.checkbox(&mut light.cut_enabled, "Hard cut").changed();
    if light.cut_enabled {
        changed |= rotation_control(
            ui,
            "Cutoff rotation",
            &mut light.cut_rotation,
            "The arrow points toward the side to trim.",
        );
        changed |= ui
            .add(
                egui::Slider::new(&mut light.cut_amount, 0.0..=1.0)
                    .text("Trim")
                    .custom_formatter(|value, _| format!("{:.0}%", 100.0 * value))
                    .custom_parser(|value| {
                        value
                            .trim_end_matches('%')
                            .trim()
                            .parse::<f64>()
                            .ok()
                            .map(|value| value / 100.0)
                    }),
            )
            .changed();
    }
    changed
}

fn hotspot_controls(ui: &mut egui::Ui, light: &mut Light) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label("Hotspot position");
        if ui.small_button("Center").clicked() {
            light.core = [0.0; 2];
            changed = true;
        }
    });
    let (pad, response) =
        ui.allocate_exact_size(egui::vec2(156.0, 86.0), egui::Sense::click_and_drag());
    let center = pad.center();
    let range = pad.shrink(9.0).size() * 0.5;
    const CORE_LIMIT: f32 = 0.68;
    if (response.clicked() || response.dragged() || response.drag_started())
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let relative = pointer - center;
        light.core = [
            (relative.x / range.x).clamp(-1.0, 1.0) * CORE_LIMIT,
            (relative.y / range.y).clamp(-1.0, 1.0) * CORE_LIMIT,
        ];
        changed = true;
    }
    let painter = ui.painter();
    painter.rect_filled(pad, 6.0, ui.visuals().extreme_bg_color);
    let guide = egui::Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color);
    painter.rect_stroke(pad, 6.0, guide, egui::StrokeKind::Inside);
    painter.hline(pad.x_range(), center.y, guide);
    painter.vline(center.x, pad.y_range(), guide);
    let dot = center + egui::vec2(light.core[0] * range.x, light.core[1] * range.y) / CORE_LIMIT;
    painter.circle_filled(dot, 5.0, egui::Color32::from_rgb(255, 194, 77));
    response.on_hover_text(
        "Drag to shift the bright core inside the light. The falloff curve controls its gradient.",
    );

    changed
}

fn draw_lat_long_guide(painter: &egui::Painter, rect: egui::Rect) {
    let shadow = egui::Stroke::new(3.0_f32, egui::Color32::from_black_alpha(160));
    let line = egui::Stroke::new(1.0_f32, egui::Color32::from_white_alpha(160));
    for stroke in [shadow, line] {
        painter.hline(rect.x_range(), rect.center().y, stroke);
    }

    let red = egui::Color32::from_rgb(255, 105, 112);
    let green = egui::Color32::from_rgb(112, 224, 132);
    let blue = egui::Color32::from_rgb(105, 169, 255);

    for (direction, name, color) in [
        (glam::Vec3::NEG_X, "X-", red),
        (glam::Vec3::NEG_Z, "Z-", blue),
        (glam::Vec3::X, "X+", red),
        (glam::Vec3::Z, "Z+", blue),
    ] {
        let uv = crate::environment::panorama_uv(direction);
        // The X- meridian is the panorama seam and appears on both edges.
        for u in [uv.x, uv.x + 1.0].into_iter().take_while(|&u| u <= 1.0) {
            let x = rect.left() + rect.width() * u;
            let width: f32 = if u == 0.0 || u == 1.0 { 2.0 } else { 1.0 };
            for stroke in [shadow, egui::Stroke::new(width, color)] {
                painter.vline(x, rect.y_range(), stroke);
            }
            let label =
                painter.layout_no_wrap(name.to_owned(), egui::FontId::monospace(12.0), color);
            let left = (x - label.size().x * 0.5)
                .min(rect.right() - label.size().x - 5.0)
                .max(rect.left() + 5.0);
            let label_rect =
                egui::Rect::from_min_size(egui::pos2(left, rect.top() + 5.0), label.size());
            painter.rect_filled(label_rect.expand2(egui::vec2(3.0, 1.0)), 2.0, shadow.color);
            painter.galley(label_rect.min, label, color);
        }
    }
    for y in [rect.top(), rect.bottom()] {
        for stroke in [shadow, egui::Stroke::new(2.0_f32, green)] {
            painter.hline(rect.x_range(), y, stroke);
        }
    }
}

fn draw_light_guide(
    painter: &egui::Painter,
    rect: egui::Rect,
    light: &Light,
    rotation: f32,
    selected: bool,
    stroke: egui::Stroke,
) {
    let projection = light.projection();
    let to_screen =
        |uv: glam::Vec2| rect.min + egui::vec2(uv.x * rect.width(), uv.y * rect.height());
    let to_map = |point| {
        let mut uv = projection.panorama(point);
        uv.x = (uv.x + rotation).rem_euclid(1.0);
        uv
    };
    let draw_curve = |points: &mut dyn Iterator<Item = glam::Vec2>| {
        let mut mapped = points.map(to_map);
        let Some(mut last) = mapped.next() else {
            return;
        };
        for current in mapped {
            let mut end = current;
            end.x = last.x + (end.x - last.x + 0.5).rem_euclid(1.0) - 0.5;
            painter.line_segment([to_screen(last), to_screen(end)], stroke);
            if !(0.0..=1.0).contains(&end.x) {
                let offset = glam::Vec2::new(if end.x < 0.0 { 1.0 } else { -1.0 }, 0.0);
                painter.line_segment([to_screen(last + offset), to_screen(end + offset)], stroke);
            }
            last = current;
        }
    };
    match light.shape {
        LightShape::Rectangle | LightShape::Gradient => {
            let corners = [
                glam::Vec2::new(-1.0, -1.0),
                glam::Vec2::new(1.0, -1.0),
                glam::Vec2::new(1.0, 1.0),
                glam::Vec2::new(-1.0, 1.0),
            ];
            for i in 0..4 {
                draw_curve(
                    &mut (0..=32)
                        .map(|step| corners[i].lerp(corners[(i + 1) % 4], step as f32 / 32.0)),
                );
            }
        }
        LightShape::Round => {
            draw_curve(
                &mut (0..=128).map(|step| {
                    glam::Vec2::from_angle(step as f32 / 128.0 * std::f32::consts::TAU)
                }),
            );
        }
        LightShape::Image => return,
    }
    if selected {
        let core = to_map(if light.shape == LightShape::Gradient {
            glam::Vec2::ZERO
        } else {
            glam::Vec2::from_array(light.core)
        });
        for seam_offset in [-1.0, 0.0, 1.0] {
            painter.circle_stroke(
                to_screen(core + glam::Vec2::new(seam_offset, 0.0)),
                4.0,
                stroke,
            );
        }
        if let Some([start, end]) = light.cut_segment() {
            draw_curve(&mut (0..=64).map(|step| start.lerp(end, step as f32 / 64.0)));
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CanvasView {
    Hdr,
    Falloff,
}

pub struct Editor {
    pub lighting: Lighting,
    document: Document,
    selected: Option<usize>,
    dirty: bool,
    preview_job: Option<mpsc::Receiver<HdrImage>>,
    texture: Option<egui::TextureHandle>,
    canvas_view: CanvasView,
    show_lat_long_guide: bool,
    falloff_texture: Option<(usize, egui::TextureHandle)>,
    falloff_stale: bool,
    import: mpsc::Receiver<Result<(HdrImage, String), String>>,
    import_tx: mpsc::Sender<Result<(HdrImage, String), String>>,
    importing: Option<u32>,
    export: Option<mpsc::Receiver<Result<PathBuf, String>>>,
    export_width: u32,
    drag_offset: [f32; 2],
    dragged_light: Option<usize>,
    status: String,
    error: Option<String>,
}
impl Default for Editor {
    fn default() -> Self {
        let (import_tx, import) = mpsc::channel();
        Self {
            lighting: Lighting::default(),
            document: Document::default(),
            selected: None,
            dirty: true,
            preview_job: None,
            texture: None,
            canvas_view: CanvasView::Hdr,
            show_lat_long_guide: true,
            falloff_texture: None,
            falloff_stale: true,
            import,
            import_tx,
            importing: None,
            export: None,
            export_width: 2048,
            drag_offset: [0.; 2],
            dragged_light: None,
            status: String::new(),
            error: None,
        }
    }
}
impl Editor {
    fn add_light(&mut self) -> usize {
        let index = self.document.add_light();
        self.selected = Some(index);
        self.lighting.default_light = false;
        self.dirty = true;
        index
    }

    pub fn place_light(&mut self, direction: glam::Vec3, new_light: bool) -> Option<&str> {
        let existing = self.selected.filter(|&index| {
            !new_light
                && self
                    .document
                    .lights
                    .get(index)
                    .is_some_and(|light| light.enabled && light.shape != LightShape::Image)
        });
        let index = match existing {
            Some(index) => index,
            None if self.document.lights.len() < 32 => self.add_light(),
            None => return None,
        };
        let uv = crate::environment::panorama_uv(direction);
        let light = &mut self.document.lights[index];
        let u = (uv.x - self.document.rotation / 360.0).rem_euclid(1.0);
        let v = uv.y.clamp(0.0, 1.0);
        self.dirty |= u != light.u || v != light.v;
        light.u = u;
        light.v = v;
        self.selected = Some(index);
        Some(&light.name)
    }

    pub fn import(&mut self, path: PathBuf, ctx: &egui::Context) {
        if self.importing.is_some() || self.document.lights.len() >= 32 {
            return;
        }
        let index = self.add_light();
        let layer = &mut self.document.lights[index];
        layer.shape = LightShape::Image;
        layer.name = format!("Image {}", layer.id);
        let id = layer.id;
        self.import_image(path, id, ctx);
    }

    fn import_image(&mut self, path: PathBuf, id: u32, ctx: &egui::Context) {
        self.error = None;
        self.importing = Some(id);
        let tx = self.import_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = HdrImage::load(&path).map(|image| {
                (
                    image,
                    path.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                )
            });
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }
    pub fn poll(&mut self) {
        if let Ok(result) = self.import.try_recv() {
            let target = self.importing.take();
            if let Some(layer) = self
                .document
                .lights
                .iter_mut()
                .find(|layer| Some(layer.id) == target)
            {
                match result {
                    Ok((image, name)) => {
                        self.status = format!("Loaded {} × {}", image.width, image.height);
                        if layer.name == format!("Image {}", layer.id) {
                            layer.name = name.clone();
                        }
                        layer.image_layer.image = Some(Arc::new(image));
                        layer.image_layer.source = name;
                        self.dirty = true;
                    }
                    Err(e) => self.error = Some(e),
                }
            }
        }
        if let Some(rx) = &self.export {
            match rx.try_recv() {
                Ok(result) => {
                    self.export = None;
                    match result {
                        Ok(path) => self.status = format!("Saved {}", path.display()),
                        Err(e) => self.error = Some(e),
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.export = None;
                    self.error = Some("HDR export worker stopped".into());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
    }
    pub fn lighting_controls(&mut self, ui: &mut egui::Ui) {
        ui.checkbox(&mut self.lighting.default_light,"Default lighting").on_hover_text("Turn off the built-in key light and ambient fill to judge the HDR environment alone.");
        ui.checkbox(&mut self.lighting.environment, "HDR lighting");
        ui.checkbox(&mut self.lighting.background, "HDR background");
        ui.checkbox(&mut self.lighting.soft_shadows, "HDR soft shadows")
            .on_hover_text("Cast shadows from the HDR environment onto model surfaces.");
        ui.checkbox(&mut self.lighting.floor_shadows, "Show floor shadows")
            .on_hover_text("Catch HDR shadows on a floor at the bottom of the model.");
        preview_quality_options(
            ui,
            "Min shadow samples",
            &mut self.lighting.shadow_samples,
            &SHADOW_SAMPLE_OPTIONS,
            0,
            "",
        );
        self.lighting.idle_shadow_samples = self
            .lighting
            .idle_shadow_samples
            .max(self.lighting.shadow_samples);
        preview_quality_options(
            ui,
            "Idle shadow samples",
            &mut self.lighting.idle_shadow_samples,
            &SHADOW_SAMPLE_OPTIONS,
            self.lighting.shadow_samples,
            "",
        );
        preview_quality_options(
            ui,
            "Min preview resolution",
            &mut self.lighting.preview_scale,
            &PREVIEW_SCALE_OPTIONS,
            0,
            "%",
        );
        self.lighting.idle_preview_scale = self
            .lighting
            .idle_preview_scale
            .max(self.lighting.preview_scale);
        preview_quality_options(
            ui,
            "Idle preview resolution",
            &mut self.lighting.idle_preview_scale,
            &PREVIEW_SCALE_OPTIONS,
            self.lighting.preview_scale,
            "%",
        );
        ui.collapsing("Preview material", |ui| {
            ui.checkbox(&mut self.lighting.ray_reflections, "Ray-traced reflections")
                .on_hover_text("Trace one reflection to nearby surfaces when HDR lighting is on.");
            ui.add(egui::Slider::new(&mut self.lighting.roughness, 0.0..=1.0).text("Roughness"));
            ui.add(egui::Slider::new(&mut self.lighting.metallic, 0.0..=1.0).text("Metallic"));
            ui.small("Preview material only; not part of the exported HDR.");
        });
        if !self.lighting.environment
            && (self.lighting.soft_shadows
                || self.lighting.floor_shadows
                || self.lighting.ray_reflections)
        {
            ui.small("Enable HDR lighting to preview HDR shadows and reflections.");
        }
    }

    pub fn environment_controls(&mut self, ui: &mut egui::Ui) {
        self.dirty |= ui
            .add(egui::Slider::new(&mut self.document.exposure, -10.0..=10.0).text("Exposure (EV)"))
            .changed();
        self.dirty |= ui
            .add(
                egui::Slider::new(&mut self.document.rotation, -180.0..=180.0).text("Rotation (°)"),
            )
            .changed();
        ui.separator();
        ui.horizontal(|ui| {
            ui.strong("Light layers");
            if ui
                .add_enabled(
                    self.document.lights.len() < 32,
                    egui::Button::new("+ Add light"),
                )
                .clicked()
            {
                self.add_light();
            }
        });
        if !self.document.lights.is_empty() {
            egui::ScrollArea::vertical()
                .id_salt("light-list")
                .max_height(135.0)
                .show(ui, |ui| {
                    for (index, light) in self.document.lights.iter_mut().enumerate().rev() {
                        let selected = self.selected == Some(index);
                        egui::Frame::new()
                            .fill(if selected {
                                ui.visuals().selection.bg_fill
                            } else {
                                ui.visuals().faint_bg_color
                            })
                            .corner_radius(4.0)
                            .inner_margin(5)
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    self.dirty |= ui
                                        .checkbox(&mut light.enabled, "")
                                        .on_hover_text("Enable layer")
                                        .changed();
                                    let (swatch, _) = ui.allocate_exact_size(
                                        egui::vec2(13.0, 13.0),
                                        egui::Sense::hover(),
                                    );
                                    let color: egui::Color32 = egui::Rgba::from_rgb(
                                        light.color[0],
                                        light.color[1],
                                        light.color[2],
                                    )
                                    .into();
                                    match light.shape {
                                        LightShape::Rectangle | LightShape::Gradient => {
                                            ui.painter().rect_filled(swatch, 2.0, color);
                                        }
                                        LightShape::Round => {
                                            ui.painter().circle_filled(swatch.center(), 6.5, color);
                                        }
                                        LightShape::Image => {
                                            ui.painter().rect_filled(
                                                swatch.shrink2(egui::vec2(0.0, 3.0)),
                                                2.0,
                                                ui.visuals().weak_text_color(),
                                            );
                                        }
                                    }
                                    let name_width = (ui.available_width() - 59.0).max(30.0);
                                    if ui
                                        .add_sized(
                                            [name_width, 18.0],
                                            egui::Label::new(&light.name)
                                                .truncate()
                                                .sense(egui::Sense::click()),
                                        )
                                        .on_hover_text(&light.name)
                                        .clicked()
                                    {
                                        self.selected = Some(index);
                                    }
                                    ui.small(if light.shape == LightShape::Image {
                                        "Image"
                                    } else {
                                        light.blend.label()
                                    });
                                });
                            });
                    }
                });
        } else {
            ui.small("Add a layer or double-click the HDR canvas.");
        }
        if let Some(index) = self.selected {
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        index + 1 < self.document.lights.len(),
                        egui::Button::new("↑"),
                    )
                    .on_hover_text("Move layer up")
                    .clicked()
                {
                    self.document.lights.swap(index, index + 1);
                    self.selected = Some(index + 1);
                    self.dirty = true;
                }
                if ui
                    .add_enabled(index > 0, egui::Button::new("↓"))
                    .on_hover_text("Move layer down")
                    .clicked()
                {
                    self.document.lights.swap(index, index - 1);
                    self.selected = Some(index - 1);
                    self.dirty = true;
                }
                if ui.button("Remove layer").clicked() {
                    self.document.lights.remove(index);
                    self.selected = (!self.document.lights.is_empty())
                        .then(|| index.min(self.document.lights.len() - 1));
                    self.dirty = true;
                }
            });
        }
        let mut image_import = None;
        if let Some(index) = self.selected {
            ui.separator();
            ui.strong("Layer properties");
            let light = &mut self.document.lights[index];
            ui.text_edit_singleline(&mut light.name);
            ui.horizontal_wrapped(|ui| {
                ui.label("Shape");
                self.dirty |= ui
                    .selectable_value(&mut light.shape, LightShape::Round, "Round")
                    .changed();
                self.dirty |= ui
                    .selectable_value(&mut light.shape, LightShape::Rectangle, "Rectangle")
                    .changed();
                self.dirty |= ui
                    .selectable_value(&mut light.shape, LightShape::Gradient, "Gradient")
                    .on_hover_text("A rectangular light with a gradient from side to side")
                    .changed();
                if ui
                    .selectable_value(&mut light.shape, LightShape::Image, "Image")
                    .changed()
                {
                    if light.name == format!("Round {}", light.id) {
                        light.name = format!("Image {}", light.id);
                    }
                    self.dirty = true;
                }
            });
            if light.shape == LightShape::Image {
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            self.importing.is_none(),
                            egui::Button::new("Load HDR / EXR…"),
                        )
                        .clicked()
                        && let Some(path) = rfd::FileDialog::new()
                            .add_filter("HDR environment", &["hdr", "exr"])
                            .pick_file()
                    {
                        image_import = Some((path, light.id));
                    }
                    if self.importing == Some(light.id) {
                        ui.spinner();
                    }
                });
                let layer = &mut light.image_layer;
                if !layer.source.is_empty() {
                    ui.small(&layer.source);
                }
                self.dirty |= ui
                    .add(
                        egui::Slider::new(&mut layer.rotation, -180.0..=180.0)
                            .text("Image rotation (°)"),
                    )
                    .changed();
                self.dirty |= ui
                    .add(
                        egui::Slider::new(&mut layer.exposure, -10.0..=10.0)
                            .text("Image exposure (EV)"),
                    )
                    .changed();
                self.dirty |= ui
                    .add(egui::Slider::new(&mut layer.alpha, 0.0..=1.0).text("Alpha"))
                    .changed();
                self.dirty |= ui
                    .checkbox(&mut layer.linear, "Linear")
                    .on_hover_text("On: use linear RGB. Off: decode the image as sRGB.")
                    .changed();
            } else {
                ui.horizontal(|ui| {
                    ui.label("Blend");
                    egui::ComboBox::from_id_salt("blend-mode")
                        .selected_text(light.blend.label())
                        .show_ui(ui, |ui| {
                            for mode in BlendMode::ALL {
                                self.dirty |= ui
                                    .selectable_value(&mut light.blend, mode, mode.label())
                                    .changed();
                            }
                        });
                    if light.blend.supports_invert() {
                        self.dirty |= ui
                            .checkbox(&mut light.invert, "Invert")
                            .on_hover_text("With Add, subtract brightness to make a Dark Light")
                            .changed();
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Color");
                    self.dirty |= ui.color_edit_button_rgb(&mut light.color).changed();
                });
                self.dirty |= ui
                    .add(
                        egui::Slider::new(&mut light.intensity, 0.0..=10000.0)
                            .logarithmic(true)
                            .text("Intensity"),
                    )
                    .changed();
                self.dirty |= ui
                    .add(egui::Slider::new(&mut light.width_degrees, 1.0..=180.0).text("Width (°)"))
                    .changed();
                self.dirty |= ui
                    .add(
                        egui::Slider::new(&mut light.height_degrees, 1.0..=90.0).text("Height (°)"),
                    )
                    .changed();
                let directional = light.shape == LightShape::Gradient;
                let box_gradient = light.shape == LightShape::Rectangle;
                egui::CollapsingHeader::new(if directional {
                    "Gradient curve"
                } else if box_gradient {
                    "Box Gradient"
                } else {
                    "Falloff curve"
                })
                .id_salt("light-curve")
                .default_open(true)
                .show(ui, |ui| {
                    if box_gradient {
                        self.dirty |= ui
                            .checkbox(&mut light.box_link_axes, "Link horizontal and vertical")
                            .changed();
                        if !light.box_link_axes {
                            light
                                .box_vertical
                                .get_or_insert_with(|| light.falloff.clone());
                            ui.label("Horizontal curve");
                        }
                    }
                    let curve = if directional {
                        &mut light.gradient_curve
                    } else {
                        &mut light.falloff
                    };
                    ui.push_id("main-ramp", |ui| {
                        self.dirty |= falloff_controls(ui, curve, directional, box_gradient);
                    });
                    if box_gradient
                        && !light.box_link_axes
                        && let Some(vertical) = &mut light.box_vertical
                    {
                        egui::CollapsingHeader::new("Vertical curve")
                            .default_open(true)
                            .show(ui, |ui| {
                                self.dirty |= falloff_controls(ui, vertical, false, true);
                            });
                    }
                });
                egui::CollapsingHeader::new("Mapping, hotspot & cut")
                    .default_open(true)
                    .show(ui, |ui| {
                        self.dirty |= mapping_hotspot_cut_controls(ui, light);
                    });
                let mut longitude = (light.u - 0.5) * 360.0;
                let mut latitude = 90.0 - light.v * 180.0;
                if ui
                    .add(egui::Slider::new(&mut longitude, -180.0..=180.0).text("Longitude (°)"))
                    .changed()
                {
                    light.u = (longitude / 360.0 + 0.5).rem_euclid(1.0);
                    self.dirty = true;
                }
                if ui
                    .add(egui::Slider::new(&mut latitude, -90.0..=90.0).text("Latitude (°)"))
                    .changed()
                {
                    light.v = (90.0 - latitude) / 180.0;
                    self.dirty = true;
                }
            }
        }
        if let Some((path, id)) = image_import {
            self.import_image(path, id, ui.ctx());
        }
        ui.separator();
        egui::ComboBox::from_id_salt("hdr-output-size")
            .selected_text(format!("{} × {}", self.export_width, self.export_width / 2))
            .show_ui(ui, |ui| {
                for width in [512, 1024, 2048, 4096, 8192] {
                    ui.selectable_value(
                        &mut self.export_width,
                        width,
                        format!("{width} × {}", width / 2),
                    );
                }
            });
        if ui
            .add_enabled(
                self.export.is_none() && self.importing.is_none(),
                egui::Button::new("Export HDR…"),
            )
            .clicked()
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("Radiance HDR", &["hdr"])
                .set_file_name("lighting.hdr")
                .save_file()
        {
            let path = path.with_extension("hdr");
            let snapshot = self.document.clone();
            let width = self.export_width;
            let (tx, rx) = mpsc::channel();
            self.export = Some(rx);
            self.error = None;
            let ctx = ui.ctx().clone();
            std::thread::spawn(move || {
                let result = snapshot
                    .compose(width, width / 2)
                    .save(&path)
                    .map(|()| path);
                let _ = tx.send(result);
                ctx.request_repaint();
            });
        }
        if self.export.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Exporting HDR…");
            });
        }
        if !self.status.is_empty() {
            ui.small(&self.status);
        }
        if let Some(error) = &self.error {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        }
    }

    pub fn canvas(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("hdr-canvas")
            .default_height(250.0)
            .min_height(140.0)
            .max_height(500.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.canvas_view, CanvasView::Hdr, "HDR Canvas");
                    ui.selectable_value(
                        &mut self.canvas_view,
                        CanvasView::Falloff,
                        "Falloff Preview",
                    );
                    match self.canvas_view {
                        CanvasView::Hdr => {
                            ui.checkbox(&mut self.show_lat_long_guide, "Lat-Long Guide");
                            ui.small("Double-click to add · drag a light to move");
                        }
                        CanvasView::Falloff => {
                            ui.small("Linear grayscale · drag the selected light to move");
                        }
                    }
                });
                let available = ui.available_rect_before_wrap();
                let width = available.width().min(available.height() * 2.0).max(2.0);
                let rect = egui::Rect::from_center_size(
                    available.center(),
                    egui::vec2(width, width * 0.5),
                );
                let editing_hdr = self.canvas_view == CanvasView::Hdr;
                let texture = match self.canvas_view {
                    CanvasView::Hdr => self.texture.as_ref(),
                    CanvasView::Falloff => self
                        .falloff_texture
                        .as_ref()
                        .filter(|(index, _)| {
                            Some(*index) == self.selected
                                && self.document.lights.get(*index).is_some_and(|light| {
                                    light.enabled && light.shape != LightShape::Image
                                })
                        })
                        .map(|(_, texture)| texture),
                };
                if let Some(texture) = texture {
                    ui.painter().image(
                        texture.id(),
                        rect,
                        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
                        egui::Color32::WHITE,
                    );
                } else {
                    ui.painter().rect_filled(rect, 0.0, egui::Color32::BLACK);
                }
                let painter = ui.painter().with_clip_rect(rect);
                painter.rect_stroke(
                    rect,
                    0.0,
                    egui::Stroke::new(1.0_f32, egui::Color32::GRAY),
                    egui::StrokeKind::Inside,
                );
                if editing_hdr && self.show_lat_long_guide {
                    draw_lat_long_guide(&painter, rect);
                }
                if !editing_hdr
                    && self
                        .selected
                        .and_then(|index| self.document.lights.get(index))
                        .is_none_or(|light| light.shape == LightShape::Image)
                {
                    painter.text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "Select a light layer",
                        egui::FontId::proportional(14.0),
                        egui::Color32::LIGHT_GRAY,
                    );
                }
                let response = ui.allocate_rect(rect, egui::Sense::click_and_drag());
                let pointer = response.interact_pointer_pos().map(|p| {
                    egui::vec2(
                        (p.x - rect.left()) / rect.width(),
                        (p.y - rect.top()) / rect.height(),
                    )
                });
                let rotation = self.document.rotation / 360.0;
                // Drag-start may arrive after the pointer has moved substantially.
                // Hit-test at the press origin, then retain the original grab offset.
                let drag_started = response.drag_started_by(egui::PointerButton::Primary);
                let hit_pointer = if drag_started {
                    ui.input(|i| i.pointer.press_origin()).map(|p| {
                        egui::vec2(
                            (p.x - rect.left()) / rect.width(),
                            (p.y - rect.top()) / rect.height(),
                        )
                    })
                } else {
                    pointer
                };
                if editing_hdr
                    && response.double_clicked()
                    && self.document.lights.len() < 32
                    && let Some(uv) = pointer
                {
                    let i = self.add_light();
                    self.document.lights[i].u = (uv.x - rotation).rem_euclid(1.0);
                    self.document.lights[i].v = uv.y.clamp(0.005, 0.995);
                } else if (response.clicked() || drag_started)
                    && let Some(uv) = hit_pointer
                {
                    let hit = self
                        .document
                        .lights
                        .iter()
                        .enumerate()
                        .rev()
                        .find(|(index, light)| {
                            (editing_hdr || self.selected == Some(*index))
                                && light.hit(uv.x - rotation, uv.y)
                        })
                        .map(|(i, _)| i);
                    if editing_hdr {
                        self.selected = hit;
                    }
                    if drag_started {
                        self.dragged_light = hit;
                    }
                    if drag_started && let Some(i) = hit {
                        let light = &self.document.lights[i];
                        self.drag_offset = [
                            ((light.u + rotation - uv.x + 0.5).rem_euclid(1.0) - 0.5),
                            light.v - uv.y,
                        ];
                    }
                }
                if response.dragged_by(egui::PointerButton::Primary)
                    && let (Some(i), Some(uv)) = (self.dragged_light, pointer)
                    && let Some(light) = self
                        .document
                        .lights
                        .get_mut(i)
                        .filter(|light| light.enabled)
                {
                    light.u = (uv.x + self.drag_offset[0] - rotation).rem_euclid(1.0);
                    light.v = (uv.y + self.drag_offset[1]).clamp(0.005, 0.995);
                    self.dirty = true;
                }
                if response.drag_stopped() {
                    self.dragged_light = None;
                }
                if !editing_hdr {
                    return;
                }
                for (i, light) in self.document.lights.iter().enumerate() {
                    if !light.enabled || light.shape == LightShape::Image {
                        continue;
                    }
                    let selected = self.selected == Some(i);
                    let color = if selected {
                        egui::Color32::from_rgb(255, 194, 77)
                    } else {
                        egui::Color32::WHITE
                    };
                    let stroke = egui::Stroke::new(if selected { 2.0_f32 } else { 1.0_f32 }, color);
                    draw_light_guide(&painter, rect, light, rotation, selected, stroke);
                }
            });
    }
    pub fn refresh(&mut self, ctx: &egui::Context) -> Option<Arc<HdrImage>> {
        let size = [PREVIEW_WIDTH as usize, PREVIEW_HEIGHT as usize];
        let mut completed = None;
        if let Some(image) = self
            .preview_job
            .as_ref()
            .and_then(|job| match job.try_recv() {
                Ok(image) => Some(Some(image)),
                Err(mpsc::TryRecvError::Disconnected) => Some(None),
                Err(mpsc::TryRecvError::Empty) => None,
            })
        {
            self.preview_job = None;
            if let Some(image) = image {
                let color = egui::ColorImage::new(size, image.display_pixels());
                if let Some(texture) = &mut self.texture {
                    texture.set(color, egui::TextureOptions::LINEAR);
                } else {
                    self.texture =
                        Some(ctx.load_texture("HDR canvas", color, egui::TextureOptions::LINEAR));
                }
                completed = Some(Arc::new(image));
                ctx.request_repaint();
            }
        }
        if self.dirty && self.preview_job.is_none() {
            // Coalesce edits while the worker renders. The next job uses the latest document.
            let (tx, job) = mpsc::channel();
            let document = self.document.clone();
            let ctx = ctx.clone();
            self.preview_job = Some(job);
            std::thread::spawn(move || {
                let image = document.compose(PREVIEW_WIDTH, PREVIEW_HEIGHT);
                let _ = tx.send(image);
                ctx.request_repaint();
            });
            self.falloff_stale = true;
            self.dirty = false;
        }
        if self.canvas_view == CanvasView::Falloff
            && let Some(index) = self.selected
            && (self.falloff_stale
                || self
                    .falloff_texture
                    .as_ref()
                    .is_none_or(|(cached, _)| *cached != index))
            && let Some(pixels) =
                self.document
                    .falloff_preview(index, PREVIEW_WIDTH, PREVIEW_HEIGHT)
        {
            let color = egui::ColorImage::new(size, pixels);
            if let Some((cached, texture)) = &mut self.falloff_texture {
                *cached = index;
                texture.set(color, egui::TextureOptions::LINEAR);
            } else {
                self.falloff_texture = Some((
                    index,
                    ctx.load_texture("Falloff preview", color, egui::TextureOptions::LINEAR),
                ));
            }
            self.falloff_stale = false;
            ctx.request_repaint();
        }
        completed
    }
}

fn preview_quality_options(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut u32,
    options: &[u32],
    minimum: u32,
    suffix: &str,
) {
    egui::ComboBox::from_label(label)
        .selected_text(format!("{value}{suffix}"))
        .show_ui(ui, |ui| {
            for &option in options.iter().filter(|&&option| option >= minimum) {
                ui.selectable_value(value, option, format!("{option}{suffix}"));
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_light_layers_are_soft_round_lights_and_disable_default_lighting() {
        let mut editor = Editor::default();
        assert!(!editor.lighting.default_light);
        editor.lighting.default_light = true;

        assert_eq!(editor.place_light(glam::Vec3::X, false), Some("Round 1"));
        let light = &editor.document.lights[0];
        assert!(matches!(light.shape, LightShape::Round));
        assert!(matches!(light.falloff.preset, Some(FalloffPreset::Smooth)));
        assert!(!editor.lighting.default_light);

        editor.lighting.default_light = true;
        assert_eq!(editor.place_light(glam::Vec3::Z, false), Some("Round 1"));
        assert!(editor.lighting.default_light);
        assert_eq!(editor.place_light(glam::Vec3::Z, true), Some("Round 2"));
        assert!(!editor.lighting.default_light);
    }
}
