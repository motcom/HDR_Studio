use crate::{
    environment::{
        BlendMode, BoxInterpolation, ColorStop, Document, FalloffCurve, FalloffPreset,
        GradientBackground, HdrImage, ImageSource, Light, LightMapping, LightShape,
        MAX_COLOR_STOPS, PREVIEW_HEIGHT, PREVIEW_WIDTH, panorama_rotation,
    },
    renderer::{Lighting, PREVIEW_SCALE_OPTIONS, SHADOW_SAMPLE_OPTIONS},
    vray_link::VrayLink,
};
use eframe::egui;
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
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

fn background_color_controls(ui: &mut egui::Ui, ramp: &mut GradientBackground) -> bool {
    const MIN_SPACING: f32 = 0.001;
    let mut changed = false;
    let mut remove = None;
    let (bar, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 14.0), egui::Sense::hover());
    for step in 0..64 {
        let left = step as f32 / 64.0;
        let right = (step + 1) as f32 / 64.0;
        let [r, g, b] = ramp.color_at((left + right) * 0.5).to_array();
        let rect = egui::Rect::from_x_y_ranges(
            egui::lerp(bar.x_range(), left)..=egui::lerp(bar.x_range(), right),
            bar.y_range(),
        );
        ui.painter().rect_filled(
            rect,
            0.0,
            egui::Color32::from(egui::Rgba::from_rgb(r, g, b)),
        );
    }
    for index in 0..ramp.colors.len() {
        let last = ramp.colors.len() - 1;
        let range = (index > 0 && index < last).then(|| {
            ramp.colors[index - 1].position + MIN_SPACING
                ..=ramp.colors[index + 1].position - MIN_SPACING
        });
        let stop = &mut ramp.colors[index];
        ui.push_id(index, |ui| {
            ui.horizontal(|ui| {
                changed |= ui.color_edit_button_rgb(&mut stop.color).changed();
                if let Some(range) = range {
                    let mut percent = stop.position * 100.0;
                    if ui
                        .add(
                            egui::DragValue::new(&mut percent)
                                .range(range.start() * 100.0..=range.end() * 100.0)
                                .speed(0.25)
                                .max_decimals(2)
                                .suffix("%"),
                        )
                        .changed()
                    {
                        stop.position = percent / 100.0;
                        changed = true;
                    }
                    if ui.small_button("Remove").clicked() {
                        remove = Some(index);
                    }
                } else {
                    ui.label(if index == 0 {
                        "Top · 0%"
                    } else {
                        "Bottom · 100%"
                    });
                }
            });
        });
    }
    if let Some(index) = remove {
        ramp.colors.remove(index);
        changed = true;
    }
    if ui
        .add_enabled(
            ramp.colors.len() < MAX_COLOR_STOPS,
            egui::Button::new("+ Add color stop"),
        )
        .clicked()
        && let Some((index, pair)) = ramp.colors.windows(2).enumerate().max_by(|(_, a), (_, b)| {
            (a[1].position - a[0].position).total_cmp(&(b[1].position - b[0].position))
        })
    {
        let position = (pair[0].position + pair[1].position) * 0.5;
        let color = ramp.color_at(position).to_array();
        ramp.colors.insert(index + 1, ColorStop { position, color });
        changed = true;
    }
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

fn blend_picker(ui: &mut egui::Ui, light: &mut Light) -> bool {
    let blend = light.blend_mode();
    let mut changed = false;
    egui::ComboBox::from_id_salt("blend-mode")
        .width(69.0)
        .selected_text(blend.label())
        .show_ui(ui, |ui| {
            for mode in BlendMode::ALL {
                changed |= ui.selectable_value(blend, mode, mode.label()).changed();
            }
        })
        .response
        .on_hover_text("Blend mode");
    changed
}

fn shape_icon(ui: &mut egui::Ui, shape: LightShape) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
    let painter = ui.painter();
    let color = ui.visuals().text_color();
    let stroke = egui::Stroke::new(1.25_f32, color);
    let icon = rect.shrink(2.0);
    let outline = |rect| painter.rect_stroke(rect, 1.5, stroke, egui::StrokeKind::Inside);
    match shape {
        LightShape::Round => {
            painter.circle_stroke(icon.center(), 5.5, stroke);
        }
        LightShape::Rectangle => {
            outline(icon.shrink2(egui::vec2(0.0, 2.0)));
        }
        LightShape::Gradient => {
            for (offset, height) in [(-4.0, 5.0), (0.0, 8.0), (4.0, 11.0)] {
                let bar = egui::Rect::from_min_size(
                    egui::pos2(icon.center().x + offset - 1.0, icon.bottom() - height),
                    egui::vec2(2.0, height),
                );
                painter.rect_filled(bar, 0.5, color);
            }
        }
        LightShape::GradientBackground => {
            outline(icon);
            for (offset, opacity) in [(3.0, 225), (6.0, 155), (9.0, 85)] {
                painter.hline(
                    icon.shrink(2.0).x_range(),
                    icon.top() + offset,
                    egui::Stroke::new(1.5_f32, color.gamma_multiply(opacity as f32 / 255.0)),
                );
            }
        }
        LightShape::Image => {
            outline(icon);
            painter.line(
                vec![
                    icon.left_bottom() + egui::vec2(2.0, -3.0),
                    icon.center() + egui::vec2(-1.0, -1.0),
                    icon.right_bottom() + egui::vec2(-2.0, -3.0),
                ],
                stroke,
            );
            painter.circle_filled(icon.right_top() + egui::vec2(-3.0, 3.5), 1.0, color);
        }
        LightShape::CompositeLight => {
            let front = icon.shrink2(egui::vec2(1.5, 1.5));
            outline(front.translate(egui::vec2(1.5, -1.5)));
            let front = front.translate(egui::vec2(-1.5, 1.5));
            painter.rect_filled(front, 1.5, ui.visuals().panel_fill);
            outline(front);
        }
    }
}

fn shape_picker(ui: &mut egui::Ui, light: &mut Light) -> bool {
    let previous = light.shape;
    ui.horizontal(|ui| {
        ui.label("Shape");
        shape_icon(ui, light.shape);
        egui::ComboBox::from_id_salt("light-shape")
            .width(148.0)
            .selected_text(light.shape.label())
            .show_ui(ui, |ui| {
                for shape in LightShape::ALL {
                    ui.horizontal(|ui| {
                        shape_icon(ui, shape);
                        ui.selectable_value(&mut light.shape, shape, shape.label());
                    });
                }
            });
    });
    if previous == light.shape {
        return false;
    }
    if light.shape == LightShape::CompositeLight {
        light.composite.get_or_insert_with(Box::default);
    }
    if light.name == format!("{} {}", previous.label(), light.id) {
        light.name = format!("{} {}", light.shape.label(), light.id);
    }
    true
}

fn layer_lock(ui: &mut egui::Ui, locked: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(19.0, 19.0), egui::Sense::click());
    let visuals = ui.style().interact(&response);
    let painter = ui.painter();
    let stroke = visuals.fg_stroke;
    let center = rect.center();
    let shackle = egui::Rect::from_center_size(
        center + egui::vec2(if locked { 0.0 } else { 2.0 }, -3.0),
        egui::vec2(6.0, 8.0),
    );
    painter.rect_stroke(shackle, 2.5, stroke, egui::StrokeKind::Inside);
    let body = egui::Rect::from_center_size(center + egui::vec2(0.0, 3.0), egui::vec2(10.0, 7.0));
    if locked {
        painter.rect_filled(body, 1.5, stroke.color);
    } else {
        painter.rect_filled(body, 1.5, visuals.bg_fill);
        painter.rect_stroke(body, 1.5, stroke, egui::StrokeKind::Inside);
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, locked, "Lock layer")
    });
    response.on_hover_text(if locked { "Unlock layer" } else { "Lock layer" })
}

fn position_controls(ui: &mut egui::Ui, light: &mut Light) -> bool {
    let uv = light.position();
    let mut longitude = (uv.x - 0.5) * 360.0;
    let mut latitude = 90.0 - uv.y * 180.0;
    let longitude_changed = ui
        .add(egui::Slider::new(&mut longitude, -180.0..=180.0).text("Longitude (°)"))
        .changed();
    let latitude_changed = ui
        .add(egui::Slider::new(&mut latitude, -90.0..=90.0).text("Latitude (°)"))
        .changed();
    if longitude_changed || latitude_changed {
        light.set_position(glam::Vec2::new(
            if longitude_changed {
                (longitude / 360.0 + 0.5).rem_euclid(1.0)
            } else {
                uv.x
            },
            if latitude_changed {
                (90.0 - latitude) / 180.0
            } else {
                uv.y
            },
        ));
    }
    longitude_changed || latitude_changed
}

// Z+ is at U = 0.75 in the environment panorama.
const CANVAS_SEAM_U: f32 = 0.75;

fn canvas_u(panorama_u: f32) -> f32 {
    (panorama_u - CANVAS_SEAM_U).rem_euclid(1.0)
}

fn fit_canvas(available: egui::Rect) -> Option<egui::Rect> {
    let width = available.width().min(available.height() * 2.0);
    (width > 0.0)
        .then(|| egui::Rect::from_center_size(available.center(), egui::vec2(width, width * 0.5)))
}

fn canvas_texture(size: [usize; 2], mut pixels: Vec<egui::Color32>) -> egui::ColorImage {
    let width = size[0];
    let seam_column = (width as f32 * CANVAS_SEAM_U) as usize;
    for row in pixels.chunks_exact_mut(width) {
        row.rotate_left(seam_column);
    }
    egui::ColorImage::new(size, pixels)
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
        let u = canvas_u(crate::environment::panorama_uv(direction).x);
        // The Z+ meridian is the canvas seam and appears on both edges.
        for u in [u, u + 1.0].into_iter().take_while(|&u| u <= 1.0) {
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
    selected: bool,
    stroke: egui::Stroke,
) {
    let projection = light.projection();
    let to_screen =
        |uv: glam::Vec2| rect.min + egui::vec2(uv.x * rect.width(), uv.y * rect.height());
    let to_map = |point| {
        let mut uv = projection.panorama(point);
        uv.x = canvas_u(uv.x);
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
        _ => return,
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

fn draw_composite_handle(painter: &egui::Painter, rect: egui::Rect, position: glam::Vec2) {
    let u = canvas_u(position.x);
    let stroke = egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(255, 194, 77));
    for u in [u - 1.0, u, u + 1.0] {
        let center = rect.min + egui::vec2(u * rect.width(), position.y * rect.height());
        painter.circle_stroke(center, 6.0, stroke);
        painter.line_segment(
            [center - egui::vec2(3.0, 0.0), center + egui::vec2(3.0, 0.0)],
            stroke,
        );
        painter.line_segment(
            [center - egui::vec2(0.0, 3.0), center + egui::vec2(0.0, 3.0)],
            stroke,
        );
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CanvasView {
    Hdr,
    Falloff,
}

struct CompositeParent {
    document: Document,
    layer: usize,
    solo: Option<u32>,
}

pub(crate) struct EditSnapshot {
    document: Document,
    path: Vec<(u32, Option<u32>)>,
    selected: Option<usize>,
    solo: Option<u32>,
    default_light: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditGroup {
    Pointer,
    Nudge(NudgeMode),
    Text(egui::Id),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NudgeMode {
    Position,
    Size,
    Intensity,
}

impl NudgeMode {
    fn from_modifiers(modifiers: egui::Modifiers) -> Option<Self> {
        if modifiers.is_none() {
            Some(Self::Position)
        } else if modifiers.matches_exact(egui::Modifiers::SHIFT) {
            Some(Self::Size)
        } else if modifiers.matches_exact(egui::Modifiers::CTRL) {
            Some(Self::Intensity)
        } else {
            None
        }
    }
}

#[derive(Default)]
struct EditHistory {
    undo: VecDeque<EditSnapshot>,
    redo: Vec<EditSnapshot>,
    group: Option<EditGroup>,
}

fn text_focus(ctx: &egui::Context) -> Option<egui::Id> {
    ctx.memory(|memory| memory.focused())
        .filter(|&id| egui::TextEdit::load_state(ctx, id).is_some())
}

const ARROW_KEYS: [egui::Key; 4] = [
    egui::Key::ArrowLeft,
    egui::Key::ArrowRight,
    egui::Key::ArrowUp,
    egui::Key::ArrowDown,
];
const WIDTH_RANGE: std::ops::RangeInclusive<f32> = 1.0..=180.0;
const HEIGHT_RANGE: std::ops::RangeInclusive<f32> = 1.0..=90.0;
const INTENSITY_RANGE: std::ops::RangeInclusive<f32> = 0.0..=10000.0;

fn nudge_value(value: &mut f32, delta: f32, range: std::ops::RangeInclusive<f32>) -> bool {
    delta != 0.0 && {
        let next = (*value + delta).clamp(*range.start(), *range.end());
        std::mem::replace(value, next) != next
    }
}

fn solo_document(mut document: Document, solo: Option<u32>) -> Document {
    if let Some(id) = solo {
        document.lights.retain(|light| light.id == id);
        if let Some(light) = document.lights.first_mut() {
            light.enabled = true;
            light.invert = false;
            *light.blend_mode() = if light.shape == LightShape::Image {
                BlendMode::Over
            } else {
                BlendMode::Add
            };
        }
    }
    document
}

enum LayerAction {
    Rename,
    ToggleSolo,
    Delete,
}

#[derive(Clone, Copy)]
struct LayerDrag {
    canvas_revision: u64,
    id: u32,
}

struct PreviewImages {
    canvas: Arc<HdrImage>,
    environment: Option<Arc<HdrImage>>,
    canvas_revision: u64,
}

struct CanvasDrag {
    index: usize,
    anchor: DragAnchor,
}

enum DragAnchor {
    PositionOffset(glam::Vec2),
    Composite {
        grabbed: glam::Vec2,
        rotation: glam::Quat,
    },
}

impl CanvasDrag {
    fn new(index: usize, light: &Light, grabbed: glam::Vec2) -> Self {
        let anchor = if light.shape == LightShape::CompositeLight {
            DragAnchor::Composite {
                grabbed,
                rotation: light.composite_local_rotation(),
            }
        } else {
            let mut offset = light.position() - grabbed;
            offset.x = (offset.x + 0.5).rem_euclid(1.0) - 0.5;
            DragAnchor::PositionOffset(offset)
        };
        Self { index, anchor }
    }

    fn apply(&self, light: &mut Light, mut pointer: glam::Vec2) {
        match self.anchor {
            DragAnchor::Composite { grabbed, rotation } => {
                pointer.y = pointer.y.clamp(0.005, 0.995);
                light.set_composite_local_rotation(
                    rotation * panorama_rotation(grabbed, pointer).conjugate(),
                );
            }
            DragAnchor::PositionOffset(offset) => {
                let position = pointer + offset;
                light.set_position(glam::Vec2::new(
                    position.x.rem_euclid(1.0),
                    position.y.clamp(0.005, 0.995),
                ));
            }
        }
    }
}

pub(crate) const EXPORT_WIDTHS: [u32; 5] = [512, 1024, 2048, 4096, 8192];
pub(crate) const DEFAULT_EXPORT_WIDTH: u32 = 2048;

pub struct Editor {
    pub lighting: Lighting,
    document: Document,
    composite_parents: Vec<CompositeParent>,
    selected: Option<usize>,
    solo: Option<u32>,
    history: EditHistory,
    dirty: bool,
    preview_job: Option<mpsc::Receiver<PreviewImages>>,
    preview_visible: Arc<AtomicBool>,
    canvas_revision: u64,
    canvas_image: Option<Arc<HdrImage>>,
    displayed_canvas_ev: Option<f32>,
    texture: Option<egui::TextureHandle>,
    canvas_view: CanvasView,
    show_lat_long_guide: bool,
    falloff_texture: Option<(usize, egui::TextureHandle)>,
    falloff_stale: bool,
    import: mpsc::Receiver<Result<(Arc<ImageSource>, String), String>>,
    import_tx: mpsc::Sender<Result<(Arc<ImageSource>, String), String>>,
    importing: Option<u32>,
    discard_import: bool,
    export: Option<mpsc::Receiver<Result<PathBuf, String>>>,
    export_width: u32,
    vray_link: VrayLink,
    canvas_drag: Option<CanvasDrag>,
    status: String,
    error: Option<String>,
}
impl Default for Editor {
    fn default() -> Self {
        let (import_tx, import) = mpsc::channel();
        Self {
            lighting: Lighting::default(),
            document: Document::default(),
            composite_parents: Vec::new(),
            selected: None,
            solo: None,
            history: EditHistory::default(),
            dirty: true,
            preview_job: None,
            preview_visible: Arc::new(AtomicBool::new(true)),
            canvas_revision: 0,
            canvas_image: None,
            displayed_canvas_ev: None,
            texture: None,
            canvas_view: CanvasView::Hdr,
            show_lat_long_guide: true,
            falloff_texture: None,
            falloff_stale: true,
            import,
            import_tx,
            importing: None,
            discard_import: false,
            export: None,
            export_width: DEFAULT_EXPORT_WIDTH,
            vray_link: VrayLink::default(),
            canvas_drag: None,
            status: String::new(),
            error: None,
        }
    }
}
impl Editor {
    pub(crate) fn project_document(&self) -> Document {
        self.environment_snapshot()
    }

    pub(crate) fn project_export_width(&self) -> u32 {
        self.export_width
    }

    pub(crate) fn is_importing(&self) -> bool {
        self.importing.is_some()
    }

    pub(crate) fn replace_project(
        &mut self,
        document: Document,
        lighting: Lighting,
        export_width: u32,
    ) {
        let vray_link = std::mem::take(&mut self.vray_link);
        *self = Self {
            document,
            lighting,
            export_width,
            vray_link,
            ..Self::default()
        };
    }

    fn has_light_layers(&self) -> bool {
        !self.document.lights.is_empty() || !self.composite_parents.is_empty()
    }

    fn edit_snapshot(&self) -> EditSnapshot {
        EditSnapshot {
            document: self.environment_snapshot(),
            path: self
                .composite_parents
                .iter()
                .map(|parent| (parent.document.lights[parent.layer].id, parent.solo))
                .collect(),
            selected: self.selected,
            solo: self.solo,
            default_light: self.lighting.default_light,
        }
    }

    fn restore_edit(&mut self, snapshot: EditSnapshot) {
        self.document = snapshot.document;
        self.composite_parents.clear();
        let view = self.canvas_view;
        for (id, solo) in snapshot.path {
            self.solo = solo;
            if let Some(index) = self.document.lights.iter().position(|light| light.id == id) {
                self.edit_composite(index);
            }
        }
        self.selected = snapshot.selected;
        self.solo = snapshot.solo;
        self.lighting.default_light = snapshot.default_light || !self.has_light_layers();
        self.discard_import |= self.importing.is_some();
        self.reset_canvas();
        self.canvas_view = view;
    }

    fn push_undo(&mut self, snapshot: EditSnapshot) {
        const HISTORY_LIMIT: usize = 100;
        if self.history.undo.len() >= HISTORY_LIMIT {
            self.history.undo.pop_front();
        }
        self.history.undo.push_back(snapshot);
    }

    fn travel_history(&mut self, redo: bool) -> bool {
        self.history.group = None;
        let next = if redo {
            self.history.redo.pop()
        } else {
            self.history.undo.pop_back()
        };
        let Some(next) = next else {
            return false;
        };
        let current = self.edit_snapshot();
        if redo {
            self.push_undo(current);
        } else {
            self.history.redo.push(current);
        }
        self.restore_edit(next);
        true
    }

    pub(crate) fn begin_edit_frame(&mut self, ctx: &egui::Context) -> (EditSnapshot, bool) {
        let shortcuts = text_focus(ctx).is_none() && !egui::Popup::is_any_open(ctx);
        let mut restored = false;
        if shortcuts {
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Z)) {
                restored = self.travel_history(false);
            } else if ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Y)) {
                restored = self.travel_history(true);
            }
        }
        let before = self.edit_snapshot();
        if shortcuts && let Some(index) = self.selected {
            self.layer_keys(ctx, index);
        }
        (before, restored)
    }

    pub(crate) fn finish_edit_frame(&mut self, ctx: &egui::Context, before: EditSnapshot) {
        let (pointer, released, arrows, arrow_released, nudge_mode) = ctx.input(|i| {
            (
                i.pointer.primary_down(),
                i.pointer.button_released(egui::PointerButton::Primary),
                ARROW_KEYS.iter().any(|&key| i.key_down(key)),
                ARROW_KEYS.iter().any(|&key| i.key_released(key)),
                NudgeMode::from_modifiers(i.modifiers),
            )
        });
        let group = if pointer || released {
            Some(EditGroup::Pointer)
        } else if let Some(id) = text_focus(ctx) {
            Some(EditGroup::Text(id))
        } else if arrows || arrow_released {
            nudge_mode.map(EditGroup::Nudge)
        } else {
            None
        };
        let document = self.environment_snapshot();
        self.vray_link
            .update(ctx, &document, self.importing.is_some());
        let changed =
            self.lighting.default_light != before.default_light || document != before.document;
        let same_group = group.is_some() && self.history.group == group;
        if changed {
            if !same_group {
                self.push_undo(before);
            }
            self.history.redo.clear();
        }
        self.history.group = match group {
            Some(EditGroup::Pointer) if !pointer => None,
            Some(EditGroup::Nudge(_)) if !arrows => None,
            group => group.filter(|_| changed || same_group),
        };
    }

    fn layer_keys(&mut self, ctx: &egui::Context, index: usize) {
        let light = &self.document.lights[index];
        if light.locked {
            return;
        }
        let delete = ctx.input_mut(|input| {
            let mut delete = false;
            input.events.retain(|event| {
                if let egui::Event::Key {
                    key: egui::Key::Delete,
                    pressed: true,
                    repeat,
                    modifiers,
                    ..
                } = event
                    && modifiers.is_none()
                {
                    delete |= !repeat;
                    return false;
                }
                true
            });
            delete
        });
        if delete {
            self.delete_layer(index);
            return;
        }

        let shape = light.shape;
        let light = &mut self.document.lights[index];
        self.dirty |= ctx.input_mut(|input| {
            let mut changed = false;
            input.events.retain(|event| {
                let egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } = event
                else {
                    return true;
                };
                let Some(mode) = NudgeMode::from_modifiers(*modifiers) else {
                    return true;
                };
                let delta = match key {
                    egui::Key::ArrowLeft => glam::Vec2::NEG_X,
                    egui::Key::ArrowRight => glam::Vec2::X,
                    egui::Key::ArrowUp => glam::Vec2::Y,
                    egui::Key::ArrowDown => glam::Vec2::NEG_Y,
                    _ => return true,
                };
                match mode {
                    NudgeMode::Position
                        if shape.is_positioned() || shape == LightShape::CompositeLight =>
                    {
                        let position = light.position() + delta / glam::Vec2::new(360.0, -180.0);
                        light.set_position(glam::Vec2::new(
                            position.x.rem_euclid(1.0),
                            position.y.clamp(0.0, 1.0),
                        ));
                        changed = true;
                    }
                    NudgeMode::Size if shape.is_positioned() => {
                        changed |= nudge_value(&mut light.width_degrees, delta.x, WIDTH_RANGE);
                        changed |= nudge_value(&mut light.height_degrees, delta.y, HEIGHT_RANGE);
                    }
                    NudgeMode::Intensity
                        if delta.y != 0.0
                            && (shape.is_positioned()
                                || shape == LightShape::GradientBackground) =>
                    {
                        changed |= nudge_value(&mut light.intensity, delta.y, INTENSITY_RANGE);
                    }
                    _ => return true,
                }
                false
            });
            changed
        });
    }

    fn delete_layer(&mut self, index: usize) {
        let light = &self.document.lights[index];
        if light.locked {
            return;
        }
        if self.solo == Some(light.id) {
            self.solo = None;
        }
        self.discard_import |= self.importing == Some(light.id);
        self.document.lights.remove(index);
        if !self.has_light_layers() {
            self.lighting.default_light = true;
        }
        self.selected = self
            .document
            .lights
            .iter()
            .enumerate()
            .filter(|(_, light)| !light.locked)
            .min_by_key(|(i, _)| index.abs_diff(*i))
            .map(|(i, _)| i);
        self.canvas_drag = None;
        self.dirty = true;
    }

    pub fn set_preview_visible(&mut self, visible: bool) {
        self.preview_visible.store(visible, Ordering::Relaxed);
        self.dirty |= visible;
    }

    fn reorder_layer(&mut self, id: u32, destination: usize) {
        let lights = &mut self.document.lights;
        let Some(source) = lights.iter().position(|light| light.id == id) else {
            return;
        };
        let target = destination - usize::from(source < destination);
        if source == target {
            return;
        }
        let selection = self.selected.map(|index| lights[index].id);
        let light = lights.remove(source);
        lights.insert(target, light);
        self.selected = lights.iter().position(|light| selection == Some(light.id));
        self.canvas_drag = None;
        self.dirty = true;
    }

    fn edit_composite(&mut self, layer: usize) {
        let content = self.document.lights[layer]
            .composite
            .take()
            .unwrap_or_default();
        let document = std::mem::replace(&mut self.document, *content);
        self.composite_parents.push(CompositeParent {
            document,
            layer,
            solo: self.solo.take(),
        });
        self.selected = None;
        self.reset_canvas();
    }

    fn leave_composite(&mut self) {
        if let Some(parent) = self.composite_parents.pop() {
            let content = std::mem::replace(&mut self.document, parent.document);
            self.document.lights[parent.layer].composite = Some(Box::new(content));
            self.solo = parent.solo;
            self.selected = Some(parent.layer);
            self.reset_canvas();
        }
    }

    fn reset_canvas(&mut self) {
        self.canvas_revision += 1;
        self.canvas_view = CanvasView::Hdr;
        self.canvas_image = None;
        self.displayed_canvas_ev = None;
        self.texture = None;
        self.falloff_stale = true;
        self.canvas_drag = None;
        self.dirty = true;
    }

    fn environment_snapshot(&self) -> Document {
        self.snapshot_with_canvas(self.document.clone(), false)
    }

    fn snapshot_with_canvas(&self, mut content: Document, preview: bool) -> Document {
        for parent in self.composite_parents.iter().rev() {
            let mut document = parent.document.clone();
            document.lights[parent.layer].composite = Some(Box::new(content));
            content = if preview {
                solo_document(document, parent.solo)
            } else {
                document
            };
        }
        content
    }

    fn add_light(&mut self) -> usize {
        let index = self.document.add_light();
        self.selected = Some(index);
        self.lighting.default_light = false;
        self.dirty = true;
        index
    }

    pub fn place_light(&mut self, direction: glam::Vec3, new_light: bool) -> Option<&str> {
        let existing = self.selected.filter(|&index| {
            !new_light && self.document.lights.get(index).is_some_and(Light::can_move)
        });
        let index = match existing {
            Some(index) => index,
            None if self.document.lights.len() < 32 => self.add_light(),
            None => return None,
        };
        let mut direction = direction;
        for parent in &self.composite_parents {
            direction = parent.document.lights[parent.layer].composite_local_rotation() * direction;
        }
        let uv = crate::environment::panorama_uv(direction);
        let light = &mut self.document.lights[index];
        self.dirty |= uv != light.position();
        light.set_position(uv);
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
        self.discard_import = false;
        let tx = self.import_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = HdrImage::load(&path).map(|image| {
                (
                    ImageSource::shared(image),
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
            if !std::mem::take(&mut self.discard_import)
                && let Some(layer) = self
                    .document
                    .lights
                    .iter_mut()
                    .find(|layer| Some(layer.id) == target && !layer.locked)
            {
                match result {
                    Ok((image, name)) => {
                        self.status = format!(
                            "Loaded {} × {}",
                            image.original.width, image.original.height
                        );
                        if layer.name == format!("Image {}", layer.id) {
                            layer.name = name.clone();
                        }
                        layer.image_layer.image = Some(image);
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
        ui.add(
            egui::Slider::new(&mut self.lighting.canvas_exposure_ev, -10.0..=10.0)
                .text("Canvas Exposure (EV)"),
        )
        .on_hover_text("Adjust the HDR Canvas display.");
        ui.add(
            egui::Slider::new(&mut self.lighting.render_preview_exposure_ev, -10.0..=10.0)
                .text("Render Preview Exposure (EV)"),
        )
        .on_hover_text("Adjust the 3D render preview display.");

        ui.add_space(4.0);
        ui.separator();
        ui.strong("Preview Material");
        ui.add(egui::Slider::new(&mut self.lighting.roughness, 0.0..=1.0).text("Roughness"));
        ui.add(egui::Slider::new(&mut self.lighting.metallic, 0.0..=1.0).text("Metallic"));
        ui.checkbox(&mut self.lighting.ray_reflections, "Ray-traced reflections")
            .on_hover_text("Trace one reflection to nearby surfaces when HDR lighting is on.");

        ui.add_space(4.0);
        ui.separator();
        ui.add_enabled(
            self.has_light_layers(),
            egui::Checkbox::new(&mut self.lighting.default_light, "Default lighting"),
        )
        .on_hover_text(
            "Turn off the built-in key light and ambient fill to judge the HDR environment alone.",
        )
        .on_disabled_hover_text("Default lighting is on when there are no light layers.");
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
        if !self.lighting.environment
            && (self.lighting.soft_shadows
                || self.lighting.floor_shadows
                || self.lighting.ray_reflections)
        {
            ui.small("Enable HDR lighting to preview HDR shadows and reflections.");
        }
    }

    pub fn environment_controls(&mut self, ui: &mut egui::Ui) {
        if !self.composite_parents.is_empty() {
            if ui
                .add_enabled(self.importing.is_none(), egui::Button::new("Back"))
                .clicked()
            {
                self.leave_composite();
            }
            if let Some(parent) = self.composite_parents.last() {
                ui.strong(&parent.document.lights[parent.layer].name);
            }
        }
        ui.heading("Light Layers");
        ui.add_space(5.0);
        let add_light = ui
            .scope(|ui| {
                let widgets = &mut ui.visuals_mut().widgets;
                for (state, color) in [
                    (&mut widgets.inactive, egui::Color32::from_rgb(22, 79, 115)),
                    (&mut widgets.hovered, egui::Color32::from_rgb(28, 97, 139)),
                    (&mut widgets.active, egui::Color32::from_rgb(16, 65, 97)),
                ] {
                    state.weak_bg_fill = color;
                    state.bg_fill = color;
                    state.fg_stroke.color = egui::Color32::WHITE;
                }
                ui.add_enabled(
                    self.document.lights.len() < 32,
                    egui::Button::new(egui::RichText::new("+ Add Light").strong())
                        .min_size(egui::vec2(ui.available_width(), 34.0)),
                )
                .on_disabled_hover_text("This list has reached its 32-layer limit.")
                .clicked()
            })
            .inner;
        if add_light {
            self.add_light();
        }
        ui.add_space(5.0);
        let mut drop = None;
        let mut action = None;
        let mut rename = None;
        if !self.document.lights.is_empty() {
            egui::ScrollArea::vertical()
                .id_salt("light-list")
                .max_height((ui.clip_rect().height() * 0.34).clamp(166.0, 280.0))
                .scroll_source(
                    egui::scroll_area::ScrollSource::SCROLL_BAR
                        | egui::scroll_area::ScrollSource::MOUSE_WHEEL,
                )
                .show_viewport(ui, |ui, viewport| {
                    for (index, light) in self.document.lights.iter_mut().enumerate().rev() {
                        let row = ui
                            .push_id(light.id, |ui| {
                                egui::Frame::new()
                                    .fill(if self.solo == Some(light.id) {
                                        egui::Color32::from_rgb(106, 72, 24)
                                    } else if self.selected == Some(index) {
                                        ui.visuals().selection.bg_fill
                                    } else {
                                        ui.visuals().faint_bg_color
                                    })
                                    .corner_radius(4.0)
                                    .inner_margin(5)
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            let payload = LayerDrag {
                                                canvas_revision: self.canvas_revision,
                                                id: light.id,
                                            };
                                            let (grip, drag) = ui.allocate_exact_size(
                                                egui::vec2(13.0, 19.0),
                                                egui::Sense::drag(),
                                            );
                                            drag.dnd_set_drag_payload(payload);
                                            let color = ui.style().interact(&drag).fg_stroke.color;
                                            for x in [-2.0, 2.0] {
                                                for y in [-4.0, 0.0, 4.0] {
                                                    ui.painter().circle_filled(
                                                        grip.center() + egui::vec2(x, y),
                                                        1.0,
                                                        color,
                                                    );
                                                }
                                            }
                                            drag.on_hover_text("Drag to reorder layer");
                                            if layer_lock(ui, light.locked).clicked() {
                                                light.locked = !light.locked;
                                                if light.locked && self.selected == Some(index) {
                                                    self.selected = None;
                                                }
                                                if light.locked
                                                    && self
                                                        .canvas_drag
                                                        .as_ref()
                                                        .is_some_and(|drag| drag.index == index)
                                                {
                                                    self.canvas_drag = None;
                                                }
                                            }
                                            self.dirty |= ui
                                                .add_enabled(
                                                    !light.locked,
                                                    egui::Checkbox::new(&mut light.enabled, ""),
                                                )
                                                .on_hover_text("Enable layer")
                                                .changed();
                                            shape_icon(ui, light.shape);
                                            let name_width =
                                                (ui.available_width() - 81.0).max(24.0);
                                            let name = egui::Label::new(&light.name)
                                                .truncate()
                                                .sense(egui::Sense::click_and_drag());
                                            let response = ui
                                                .add_enabled_ui(!light.locked, |ui| {
                                                    ui.add_sized([name_width, 19.0], name)
                                                })
                                                .inner;
                                            if !light.locked {
                                                response.dnd_set_drag_payload(payload);
                                                if response.clicked() {
                                                    self.selected = Some(index);
                                                }
                                            }
                                            response.on_hover_text(&light.name);
                                            self.dirty |= ui
                                                .add_enabled_ui(!light.locked, |ui| {
                                                    blend_picker(ui, light)
                                                })
                                                .inner;
                                        });
                                    })
                                    .response
                            })
                            .inner;
                        let open = row.contains_pointer()
                            && ui.input(|input| input.pointer.secondary_clicked());
                        egui::Popup::context_menu(&row)
                            .open_memory(open.then_some(egui::SetOpenCommand::Bool(true)))
                            .show(|ui| {
                                let solo_label = if self.solo == Some(light.id) {
                                    "Exit Solo"
                                } else {
                                    "Solo"
                                };
                                for (label, enabled, item) in [
                                    ("Rename…", !light.locked, LayerAction::Rename),
                                    (solo_label, true, LayerAction::ToggleSolo),
                                    ("Delete", !light.locked, LayerAction::Delete),
                                ] {
                                    if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                                        action = Some((light.id, item));
                                        ui.close();
                                    }
                                }
                            });
                        if let Some(payload) = row.dnd_hover_payload::<LayerDrag>()
                            && payload.canvas_revision == self.canvas_revision
                            && payload.id != light.id
                            && let Some(pointer) = ui.ctx().pointer_interact_pos()
                        {
                            let above = pointer.y < row.rect.center().y;
                            let y = if above {
                                row.rect.top()
                            } else {
                                row.rect.bottom()
                            };
                            ui.painter().hline(
                                row.rect.x_range(),
                                y,
                                egui::Stroke::new(2.0_f32, ui.visuals().selection.stroke.color),
                            );
                            if row.dnd_release_payload::<LayerDrag>().is_some() {
                                drop = Some((payload.id, index + usize::from(above)));
                            }
                        }
                    }
                    let clip = ui.clip_rect();
                    if let Some(payload) = egui::DragAndDrop::payload::<LayerDrag>(ui.ctx())
                        && payload.canvas_revision == self.canvas_revision
                        && ui.rect_contains_pointer(clip)
                        && let Some(pointer) = ui.ctx().pointer_interact_pos()
                    {
                        let delta = if pointer.y < clip.top() + 18.0 && viewport.min.y > 0.0 {
                            8.0
                        } else if pointer.y > clip.bottom() - 18.0
                            && viewport.max.y < ui.min_rect().height()
                        {
                            -8.0
                        } else {
                            0.0
                        };
                        if delta != 0.0 {
                            ui.scroll_with_delta_animation(
                                egui::vec2(0.0, delta),
                                egui::style::ScrollAnimation::none(),
                            );
                            ui.ctx().request_repaint();
                        }
                    }
                });
        } else {
            ui.small("Add a layer or double-click the HDR canvas.");
        }
        if let Some((id, destination)) = drop {
            self.reorder_layer(id, destination);
        }
        if let Some((id, action)) = action
            && let Some(index) = self.document.lights.iter().position(|light| light.id == id)
        {
            match action {
                LayerAction::Rename => {
                    self.selected = Some(index);
                    rename = Some(id);
                }
                LayerAction::ToggleSolo => {
                    self.solo = (self.solo != Some(id)).then_some(id);
                    if self.solo.is_some() {
                        self.selected = (!self.document.lights[index].locked).then_some(index);
                    }
                    self.canvas_drag = None;
                    self.dirty = true;
                }
                LayerAction::Delete => self.delete_layer(index),
            }
        }

        let mut image_import = None;
        let mut edit_composite = None;
        if let Some(index) = self.selected.filter(|&i| !self.document.lights[i].locked) {
            ui.separator();
            ui.strong("Layer properties");
            let light = &mut self.document.lights[index];
            let name = egui::TextEdit::singleline(&mut light.name)
                .id(ui.make_persistent_id(("layer-name", self.canvas_revision, light.id)))
                .show(ui);
            if rename == Some(light.id) {
                name.response.request_focus();
                let mut state = name.state;
                state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(0),
                        egui::text::CCursor::new(light.name.chars().count()),
                    )));
                state.store(ui.ctx(), name.response.id);
            }
            self.dirty |= shape_picker(ui, light);
            let blend = *light.blend_mode();
            if blend == BlendMode::Multiply {
                self.dirty |= ui
                    .checkbox(&mut light.multiply_mask, "Mask")
                    .on_hover_text("Treat areas outside this layer as black")
                    .changed();
            } else if blend.supports_invert() {
                self.dirty |= ui.checkbox(&mut light.invert, "Invert").changed();
            }

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
            } else if light.shape == LightShape::GradientBackground {
                self.dirty |= ui
                    .add(
                        egui::Slider::new(&mut light.intensity, INTENSITY_RANGE)
                            .logarithmic(true)
                            .text("Intensity"),
                    )
                    .changed();
                let ramp = &mut light.gradient_background;
                egui::CollapsingHeader::new("Color ramp")
                    .default_open(true)
                    .show(ui, |ui| {
                        self.dirty |= background_color_controls(ui, ramp);
                    });
                for (label, curve) in [
                    ("Brightness ramp", &mut ramp.value),
                    ("Alpha ramp", &mut ramp.alpha),
                ] {
                    egui::CollapsingHeader::new(label).show(ui, |ui| {
                        self.dirty |= falloff_controls(ui, curve, true, true);
                    });
                }
            } else if light.shape == LightShape::CompositeLight {
                if ui
                    .add_enabled(
                        self.importing.is_none(),
                        egui::Button::new("Edit composite"),
                    )
                    .clicked()
                {
                    edit_composite = Some(index);
                }
            } else {
                ui.horizontal(|ui| {
                    ui.label("Color");
                    self.dirty |= ui.color_edit_button_rgb(&mut light.color).changed();
                });
                self.dirty |= ui
                    .add(
                        egui::Slider::new(&mut light.intensity, INTENSITY_RANGE)
                            .logarithmic(true)
                            .text("Intensity"),
                    )
                    .changed();
                self.dirty |= ui
                    .add(egui::Slider::new(&mut light.width_degrees, WIDTH_RANGE).text("Width (°)"))
                    .changed();
                self.dirty |= ui
                    .add(
                        egui::Slider::new(&mut light.height_degrees, HEIGHT_RANGE)
                            .text("Height (°)"),
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
            }
            if light.shape.is_positioned() || light.shape == LightShape::CompositeLight {
                self.dirty |= position_controls(ui, light);
            }
        }
        if let Some((path, id)) = image_import {
            self.import_image(path, id, ui.ctx());
        }
        if let Some(index) = edit_composite {
            self.edit_composite(index);
        }
        self.feedback(ui);
    }

    pub fn file_controls(&mut self, ui: &mut egui::Ui, button_size: egui::Vec2) {
        ui.strong("HDR output");
        ui.label("Render size");
        ui.scope(|ui| {
            ui.spacing_mut().interact_size.y = button_size.y;
            egui::ComboBox::from_id_salt("hdr-output-size")
                .width(button_size.x)
                .selected_text(format!("{} × {}", self.export_width, self.export_width / 2))
                .show_ui(ui, |ui| {
                    for width in EXPORT_WIDTHS {
                        ui.selectable_value(
                            &mut self.export_width,
                            width,
                            format!("{width} × {}", width / 2),
                        );
                    }
                });
        });
        if ui
            .add_enabled(
                self.export.is_none() && self.importing.is_none() && !self.vray_link.is_rendering(),
                egui::Button::new("Export HDR…").min_size(button_size),
            )
            .clicked()
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("Radiance HDR", &["hdr"])
                .set_file_name("lighting.hdr")
                .save_file()
        {
            let path = path.with_extension("hdr");
            let snapshot = self.environment_snapshot();
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
        ui.separator();
        ui.strong("Maya V-Ray");
        self.vray_link.controls(
            ui,
            button_size,
            self.export_width,
            self.importing.is_some() || self.export.is_some(),
        );
        self.feedback(ui);
    }

    fn feedback(&self, ui: &mut egui::Ui) {
        if !self.status.is_empty() {
            ui.small(&self.status);
        }
        if let Some(error) = &self.error {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        }
    }

    pub fn canvas(&mut self, ctx: &egui::Context, maximized: bool) {
        if maximized {
            egui::CentralPanel::default().show(ctx, |ui| self.canvas_contents(ui));
        } else {
            egui::TopBottomPanel::bottom("hdr-canvas")
                .default_height(250.0)
                .min_height(140.0)
                .max_height(500.0)
                .resizable(true)
                .show(ctx, |ui| self.canvas_contents(ui));
        }
    }

    fn canvas_contents(&mut self, ui: &mut egui::Ui) {
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
        let Some(rect) = fit_canvas(ui.available_rect_before_wrap()) else {
            return;
        };
        let editing_hdr = self.canvas_view == CanvasView::Hdr;
        let solo = self.solo;
        let can_move = |light: &Light| {
            solo.map_or_else(
                || light.can_move(),
                |id| light.id == id && !light.locked && light.shape != LightShape::Image,
            )
        };
        let texture = match self.canvas_view {
            CanvasView::Hdr => self.texture.as_ref(),
            CanvasView::Falloff => self
                .falloff_texture
                .as_ref()
                .filter(|(index, _)| {
                    Some(*index) == self.selected
                        && self
                            .document
                            .lights
                            .get(*index)
                            .is_some_and(|light| can_move(light) && light.shape.is_positioned())
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
                .is_none_or(|light| !can_move(light) || !light.shape.is_positioned())
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
        let to_panorama = |p: egui::Pos2| {
            glam::Vec2::new(
                (p.x - rect.left()) / rect.width() + CANVAS_SEAM_U,
                (p.y - rect.top()) / rect.height(),
            )
        };
        let pointer = response.interact_pointer_pos().map(to_panorama);
        // Drag-start may arrive after the pointer has moved substantially.
        // Hit-test at the press origin, then retain the original grab offset.
        let drag_started = response.drag_started_by(egui::PointerButton::Primary);
        let hit_pointer = if drag_started {
            ui.input(|i| i.pointer.press_origin()).map(to_panorama)
        } else {
            pointer
        };
        if editing_hdr
            && response.double_clicked()
            && self.document.lights.len() < 32
            && let Some(uv) = pointer
        {
            let i = self.add_light();
            self.document.lights[i].u = uv.x.rem_euclid(1.0);
            self.document.lights[i].v = uv.y.clamp(0.005, 0.995);
        } else if (response.clicked() || drag_started)
            && let Some(uv) = hit_pointer
        {
            let on_composite_handle = |index: usize| {
                let light = &self.document.lights[index];
                let position = light.position();
                let dx = (position.x - uv.x + 0.5).rem_euclid(1.0) - 0.5;
                let offset = egui::vec2(dx * rect.width(), (position.y - uv.y) * rect.height());
                can_move(light)
                    && light.shape == LightShape::CompositeLight
                    && offset.length_sq() <= 100.0
            };
            let hit = self
                .selected
                .filter(|&index| editing_hdr && on_composite_handle(index))
                .or_else(|| {
                    self.document
                        .lights
                        .iter()
                        .enumerate()
                        .rev()
                        .find(|(index, light)| {
                            (editing_hdr || self.selected == Some(*index))
                                && solo.map_or_else(
                                    || light.hit(uv.x, uv.y),
                                    |id| id == light.id && light.hit_visible(uv.x, uv.y),
                                )
                        })
                        .map(|(i, _)| i)
                });
            if editing_hdr {
                self.selected = hit;
            }
            if drag_started {
                self.canvas_drag = hit.map(|i| CanvasDrag::new(i, &self.document.lights[i], uv));
            }
        }
        if (response.dragged_by(egui::PointerButton::Primary)
            || response.drag_stopped_by(egui::PointerButton::Primary))
            && let (Some(drag), Some(uv)) = (&self.canvas_drag, pointer)
            && let Some(light) = self
                .document
                .lights
                .get_mut(drag.index)
                .filter(|light| can_move(light))
        {
            drag.apply(light, uv);
            self.dirty = true;
        }
        if response.drag_stopped() {
            self.canvas_drag = None;
        }
        if !editing_hdr {
            return;
        }
        for (i, light) in self.document.lights.iter().enumerate() {
            if !can_move(light) {
                continue;
            }
            let selected = self.selected == Some(i);
            if light.shape == LightShape::CompositeLight {
                if selected {
                    draw_composite_handle(&painter, rect, light.position());
                }
                continue;
            }
            let color = if selected {
                egui::Color32::from_rgb(255, 194, 77)
            } else {
                egui::Color32::WHITE
            };
            let stroke = egui::Stroke::new(if selected { 2.0_f32 } else { 1.0_f32 }, color);
            draw_light_guide(&painter, rect, light, selected, stroke);
        }
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
            if let Some(images) = image {
                if images.canvas_revision == self.canvas_revision {
                    self.canvas_image = Some(images.canvas);
                    self.displayed_canvas_ev = None;
                    if self.preview_visible.load(Ordering::Relaxed) {
                        completed = images.environment;
                    }
                }
                ctx.request_repaint();
            }
        }
        let exposure = self.lighting.canvas_exposure_ev;
        if let Some(canvas) = &self.canvas_image
            && self.displayed_canvas_ev != Some(exposure)
        {
            let color = canvas_texture(size, canvas.display_pixels(exposure));
            if let Some(texture) = &mut self.texture {
                texture.set(color, egui::TextureOptions::LINEAR);
            } else {
                self.texture =
                    Some(ctx.load_texture("HDR canvas", color, egui::TextureOptions::LINEAR));
            }
            self.displayed_canvas_ev = Some(exposure);
        }
        if self.dirty && self.preview_job.is_none() {
            // Coalesce edits while the worker renders. The next job uses the latest document.
            let (tx, job) = mpsc::channel();
            let document = solo_document(self.document.clone(), self.solo);
            let at_root = self.composite_parents.is_empty();
            let visible = self.preview_visible.clone();
            let environment = (!at_root && visible.load(Ordering::Relaxed))
                .then(|| self.snapshot_with_canvas(document.clone(), true));
            let canvas_revision = self.canvas_revision;
            let ctx = ctx.clone();
            self.preview_job = Some(job);
            std::thread::spawn(move || {
                let canvas = Arc::new(document.compose_preview(PREVIEW_WIDTH, PREVIEW_HEIGHT));
                let environment = if at_root && visible.load(Ordering::Relaxed) {
                    Some(canvas.clone())
                } else {
                    environment
                        .and_then(|doc| {
                            doc.compose_preview_while(PREVIEW_WIDTH, PREVIEW_HEIGHT, || {
                                visible.load(Ordering::Relaxed)
                            })
                        })
                        .map(Arc::new)
                };
                let _ = tx.send(PreviewImages {
                    canvas,
                    environment,
                    canvas_revision,
                });
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
            && let Some(pixels) = if let Some(id) = self.solo {
                (self.document.lights[index].id == id)
                    .then(|| solo_document(self.document.clone(), Some(id)))
                    .and_then(|document| document.falloff_preview(0, PREVIEW_WIDTH, PREVIEW_HEIGHT))
            } else {
                self.document
                    .falloff_preview(index, PREVIEW_WIDTH, PREVIEW_HEIGHT)
            }
        {
            let color = canvas_texture(size, pixels);
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

    fn editor_key(
        ctx: &egui::Context,
        editor: &mut Editor,
        key: egui::Key,
        modifiers: egui::Modifiers,
        pressed: bool,
    ) {
        let input = egui::RawInput {
            modifiers,
            events: vec![egui::Event::Key {
                key,
                physical_key: Some(key),
                pressed,
                repeat: false,
                modifiers,
            }],
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            let (before, _) = editor.begin_edit_frame(ctx);
            editor.finish_edit_frame(ctx, before);
        });
    }

    #[test]
    fn preview_exposures_do_not_change_the_document_or_recompose_hdr() {
        let ctx = egui::Context::default();
        let mut editor = Editor::default();
        editor.add_light();
        let document = editor.project_document();
        let canvas = Arc::new(document.compose_preview(PREVIEW_WIDTH, PREVIEW_HEIGHT));
        editor.canvas_image = Some(canvas.clone());
        editor.dirty = false;

        let update = |editor: &mut Editor| {
            ctx.run(egui::RawInput::default(), |ctx| {
                assert!(editor.refresh(ctx).is_none());
                assert!(editor.preview_job.is_none());
            })
            .textures_delta
            .set
        };
        update(&mut editor);
        let texture = editor.texture.as_ref().unwrap().id();

        editor.lighting.render_preview_exposure_ev = 2.0;
        assert!(update(&mut editor).iter().all(|(id, _)| *id != texture));
        editor.lighting.canvas_exposure_ev = -2.0;
        assert!(update(&mut editor).iter().any(|(id, _)| *id == texture));
        assert!(Arc::ptr_eq(editor.canvas_image.as_ref().unwrap(), &canvas));
        assert!(editor.project_document() == document);
    }

    #[test]
    fn nudge_and_delete_keyboard_shortcuts_can_be_undone_and_redone() {
        let mut editor = Editor::default();
        let index = editor.add_light();
        let initial = editor.document.lights[index].position();
        let ctx = egui::Context::default();
        let key = |editor: &mut Editor, key, modifiers, pressed| {
            editor_key(&ctx, editor, key, modifiers, pressed);
        };
        key(
            &mut editor,
            egui::Key::ArrowRight,
            egui::Modifiers::NONE,
            true,
        );
        key(
            &mut editor,
            egui::Key::ArrowRight,
            egui::Modifiers::NONE,
            false,
        );
        let nudged = editor.document.lights[index].position();
        assert!((nudged.x - initial.x - 1.0 / 360.0).abs() < 1e-6);
        key(&mut editor, egui::Key::Z, egui::Modifiers::CTRL, true);
        assert_eq!(editor.document.lights[index].position(), initial);
        key(&mut editor, egui::Key::Z, egui::Modifiers::CTRL, false);
        key(&mut editor, egui::Key::Y, egui::Modifiers::CTRL, true);
        assert_eq!(editor.document.lights[index].position(), nudged);
        key(&mut editor, egui::Key::Delete, egui::Modifiers::NONE, true);
        assert!(editor.document.lights.is_empty());
        key(&mut editor, egui::Key::Z, egui::Modifiers::CTRL, true);
        assert_eq!(editor.document.lights[index].position(), nudged);
    }

    #[test]
    fn modified_arrows_nudge_dimensions_and_intensity_within_slider_ranges() {
        use egui::{Key, Modifiers};
        let ctx = egui::Context::default();
        let mut editor = Editor::default();
        let index = editor.add_light();
        let original = editor.document.lights[index].position();
        let values = |editor: &Editor| {
            let light = &editor.document.lights[index];
            (light.width_degrees, light.height_degrees, light.intensity)
        };
        let press = |editor: &mut Editor, key, modifiers| {
            editor_key(&ctx, editor, key, modifiers, true);
            editor_key(&ctx, editor, key, modifiers, false);
        };
        editor_key(&ctx, &mut editor, Key::ArrowRight, Modifiers::SHIFT, true);
        editor_key(&ctx, &mut editor, Key::ArrowRight, Modifiers::SHIFT, true);
        editor_key(&ctx, &mut editor, Key::ArrowRight, Modifiers::SHIFT, false);
        assert_eq!(values(&editor), (42.0, 40.0, 10.0));
        press(&mut editor, Key::Z, Modifiers::CTRL);
        assert_eq!(values(&editor), (40.0, 40.0, 10.0));
        press(&mut editor, Key::Y, Modifiers::CTRL);
        press(&mut editor, Key::ArrowLeft, Modifiers::SHIFT);
        press(&mut editor, Key::ArrowUp, Modifiers::SHIFT);
        press(
            &mut editor,
            Key::ArrowUp,
            Modifiers::CTRL | Modifiers::COMMAND,
        );
        assert_eq!(values(&editor), (41.0, 41.0, 11.0));
        press(&mut editor, Key::ArrowDown, Modifiers::SHIFT);
        press(&mut editor, Key::ArrowDown, Modifiers::CTRL);
        assert_eq!(values(&editor), (41.0, 40.0, 10.0));
        assert_eq!(editor.document.lights[index].position(), original);

        let light = &mut editor.document.lights[index];
        light.width_degrees = *WIDTH_RANGE.end();
        light.height_degrees = *HEIGHT_RANGE.start();
        light.intensity = *INTENSITY_RANGE.start();
        press(&mut editor, Key::ArrowRight, Modifiers::SHIFT);
        press(&mut editor, Key::ArrowDown, Modifiers::SHIFT);
        press(&mut editor, Key::ArrowDown, Modifiers::CTRL);
        assert_eq!(values(&editor), (180.0, 1.0, 0.0));

        editor.document.lights[index].shape = LightShape::GradientBackground;
        press(&mut editor, Key::ArrowLeft, Modifiers::SHIFT);
        press(
            &mut editor,
            Key::ArrowUp,
            Modifiers::CTRL | Modifiers::SHIFT,
        );
        press(&mut editor, Key::ArrowUp, Modifiers::CTRL);
        assert_eq!(values(&editor), (180.0, 1.0, 1.0));
        editor.document.lights[index].locked = true;
        press(&mut editor, Key::ArrowUp, Modifiers::CTRL);
        assert_eq!(values(&editor), (180.0, 1.0, 1.0));
    }

    #[test]
    fn solo_isolates_a_disabled_multiply_light_without_modifying_the_document() {
        let mut editor = Editor::default();
        editor.add_light();
        let index = editor.add_light();
        let light = &mut editor.document.lights[index];
        light.enabled = false;
        light.blend = BlendMode::Multiply;
        editor.solo = Some(light.id);
        let solo = solo_document(editor.document.clone(), editor.solo);
        assert_eq!(solo.lights.len(), 1);
        assert!(solo.lights[0].enabled);
        assert!(solo.lights[0].blend == BlendMode::Add);
        let export = editor.environment_snapshot();
        assert_eq!(export.lights.len(), 2);
        assert!(!export.lights[index].enabled);
        assert!(export.lights[index].blend == BlendMode::Multiply);
    }

    #[test]
    fn composite_drag_keeps_the_grabbed_child_under_the_pointer() {
        let mut doc = Document::default();
        let group = doc.add_light();
        let mut children = Document::default();
        let child = children.add_light();
        let grabbed = glam::Vec2::new(0.75, 0.5);
        children.lights[child].set_position(grabbed);
        doc.lights[group].shape = LightShape::CompositeLight;
        doc.lights[group].composite = Some(Box::new(children));

        let end = glam::Vec2::new(0.725, 0.25);
        let drag = CanvasDrag::new(group, &doc.lights[group], grabbed);
        // The pointer has wrapped west across the Z+ canvas seam.
        drag.apply(&mut doc.lights[group], end + glam::Vec2::X);
        assert!(doc.lights[group].hit(end.x, end.y));
        assert!(!doc.lights[group].hit(grabbed.x, grabbed.y));
        assert!(doc.compose(20, 10).pixels[2 * 20 + 14][0] > 9.9);

        let drag = CanvasDrag::new(group, &doc.lights[group], end);
        drag.apply(&mut doc.lights[group], grabbed);
        assert!(doc.lights[group].hit(grabbed.x, grabbed.y));
    }

    #[test]
    fn canvas_fits_wide_and_tall_panes_at_two_to_one() {
        for (pane, expected) in [
            ([960.0, 200.0], [400.0, 200.0]),
            ([960.0, 700.0], [960.0, 480.0]),
        ] {
            let bounds =
                egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(pane[0], pane[1]));
            let canvas = fit_canvas(bounds).unwrap();
            assert_eq!(canvas.center(), bounds.center());
            assert_eq!(canvas.size(), egui::vec2(expected[0], expected[1]));
        }
    }

    #[test]
    fn hidden_preview_composes_only_the_active_composite_canvas() {
        let mut editor = Editor::default();
        let index = editor.add_light();
        editor.document.lights[index].shape = LightShape::CompositeLight;
        editor.edit_composite(index);
        editor.add_light();
        let ctx = egui::Context::default();
        let mut render = |visible| {
            editor.set_preview_visible(visible);
            editor.refresh(&ctx);
            editor
                .preview_job
                .take()
                .unwrap()
                .recv_timeout(std::time::Duration::from_secs(3))
                .unwrap()
        };
        let hidden = render(false);
        assert!(hidden.environment.is_none());
        assert!(hidden.canvas.pixels.iter().any(|pixel| pixel[0] > 0.0));
        let visible = render(true);
        assert_eq!(visible.environment.unwrap().pixels, hidden.canvas.pixels);
    }

    #[test]
    fn reordering_keeps_the_selected_layer_and_clears_canvas_drag() {
        let mut editor = Editor::default();
        for _ in 0..4 {
            editor.add_light();
        }
        editor.selected = Some(1);
        editor.canvas_drag = Some(CanvasDrag::new(
            1,
            &editor.document.lights[1],
            glam::Vec2::ZERO,
        ));
        editor.reorder_layer(1, 4);
        assert_eq!(editor.selected, Some(0));
        assert!(editor.canvas_drag.is_none());
        editor.reorder_layer(4, 0);
        assert_eq!(editor.selected, Some(1));
        let ids: Vec<_> = editor
            .document
            .lights
            .iter()
            .map(|light| light.id)
            .collect();
        assert_eq!(ids, [4, 2, 3, 1]);
    }

    #[test]
    fn three_dimensional_placement_keeps_locked_lights_and_moves_composites() {
        let mut editor = Editor::default();
        let locked = editor.add_light();
        let original = editor.document.lights[locked].position();
        editor.document.lights[locked].locked = true;
        assert_eq!(editor.place_light(glam::Vec3::X, false), Some("Round 2"));
        assert_eq!(editor.document.lights[locked].position(), original);

        let group = editor.selected.unwrap();
        editor.document.lights[group].shape = LightShape::CompositeLight;
        let direction = glam::Vec3::new(0.0, 1.0, 1.0).normalize();
        assert_eq!(editor.place_light(direction, false), Some("Round 2"));
        assert_eq!(
            editor.document.lights[group].position(),
            glam::Vec2::new(0.75, 0.25)
        );
        editor.edit_composite(group);
        editor.place_light(direction, false);
        let child = &editor.document.lights[0];
        assert!(child.position().abs_diff_eq(glam::Vec2::splat(0.5), 1e-6));
    }

    #[test]
    fn editing_nested_composites_preserves_the_environment_snapshot() {
        let mut editor = Editor::default();
        for _ in 0..2 {
            let parent = editor.add_light();
            editor.document.lights[parent].shape = LightShape::CompositeLight;
            editor.edit_composite(parent);
        }
        let light = editor.add_light();
        editor.document.lights[light].name = "Nested light".to_owned();
        let active = editor.environment_snapshot().compose(36, 18);
        assert!(active.pixels.iter().any(|pixel| pixel[0] > 0.0));
        editor.leave_composite();
        assert_eq!(editor.selected, Some(0));
        editor.leave_composite();
        assert_eq!(editor.selected, Some(0));
        assert_eq!(editor.document.compose(36, 18).pixels, active.pixels);
    }

    #[test]
    fn canvas_axes_and_texture_start_at_positive_z() {
        let axes = [
            glam::Vec3::Z,
            glam::Vec3::NEG_X,
            glam::Vec3::NEG_Z,
            glam::Vec3::X,
        ];
        for (column, axis) in axes.into_iter().enumerate() {
            let u = crate::environment::panorama_uv(axis).x;
            assert_eq!(canvas_u(u), column as f32 / 4.0);
        }

        let pixels = (0..8).map(egui::Color32::from_gray).collect();
        let canvas = canvas_texture([4, 2], pixels);
        let columns: Vec<_> = canvas.pixels.iter().map(|pixel| pixel.r()).collect();
        assert_eq!(columns, [3, 0, 1, 2, 7, 4, 5, 6]);
    }

    #[test]
    fn new_light_layers_are_soft_round_lights_and_disable_default_lighting() {
        let mut editor = Editor::default();
        assert!(editor.lighting.default_light);

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

    #[test]
    fn removing_the_last_root_layer_restores_default_lighting() {
        let mut editor = Editor::default();
        let group = editor.add_light();
        editor.document.lights[group].shape = LightShape::CompositeLight;
        editor.edit_composite(group);
        let child = editor.add_light();
        editor.delete_layer(child);
        assert!(!editor.lighting.default_light);
        editor.leave_composite();

        let before = editor.edit_snapshot();
        editor.delete_layer(group);
        editor.finish_edit_frame(&egui::Context::default(), before);
        assert!(editor.lighting.default_light);
        assert!(editor.travel_history(false));
        assert!(!editor.lighting.default_light);
        assert!(editor.travel_history(true));
        assert!(editor.lighting.default_light);
    }
}
