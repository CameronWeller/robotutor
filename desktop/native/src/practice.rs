use crate::{runtime::s, theme};
use eframe::egui::{self, Ui};
use serde_json::{Map, Value};
pub fn editor(
    ui: &mut Ui,
    form: &Value,
    fields: &mut Map<String, Value>,
    draft: &Value,
    point: &mut usize,
) -> (bool, Option<String>) {
    let mut changed = false;
    let mut cmd = None;
    let mut points = Vec::new();
    for f in form["fields"].as_array().into_iter().flatten() {
        let key = s(&f["id"]);
        let label = s(&f["label"]);
        let kind = s(&f["kind"]);
        if kind == "arrow" {
            continue;
        }
        if kind == "timing" {
            if ui.button("Tap beat").clicked() {
                cmd = Some("/tap".into())
            }
            continue;
        }
        if kind == "point" {
            points.push((key.to_string(), label.to_string()));
        }
        let mut value = fields
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        ui.label(label);
        let old = value.clone();
        if let Some(choices) = f["choices"].as_array().filter(|c| !c.is_empty()) {
            egui::ComboBox::from_id_salt(key)
                .selected_text(if value.is_empty() {
                    "Choose…"
                } else {
                    &value
                })
                .show_ui(ui, |ui| {
                    for c in choices {
                        ui.selectable_value(&mut value, s(c).into(), s(c));
                    }
                });
        } else if matches!(kind, "rubric" | "document") {
            ui.add(
                egui::TextEdit::multiline(&mut value)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY),
            );
        } else {
            ui.add(
                egui::TextEdit::singleline(&mut value).hint_text(if kind == "point" {
                    "x,y"
                } else {
                    ""
                }),
            );
        }
        if value != old {
            fields.insert(key.into(), value.into());
            changed = true;
        }
    }
    if !points.is_empty() {
        *point = (*point).min(points.len() - 1);
        egui::ComboBox::from_label("Plot point")
            .selected_text(&points[*point].1)
            .show_ui(ui, |ui| {
                for (i, p) in points.iter().enumerate() {
                    ui.selectable_value(point, i, &p.1);
                }
            });
        let (area, response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width().min(370.), 155.),
            egui::Sense::click(),
        );
        let graph = egui::Rect::from_min_max(
            area.min + egui::vec2(24., 5.),
            area.max - egui::vec2(8., 20.),
        );
        let pp = |x: f32, y: f32| {
            egui::pos2(
                graph.left() + graph.width() * x / 10.,
                graph.bottom() - graph.height() * y / 10.,
            )
        };
        for i in 0..=10 {
            let n = i as f32;
            let pen = egui::Stroke::new(1.0_f32, theme::EDGE);
            ui.painter().line_segment([pp(n, 0.), pp(n, 10.)], pen);
            ui.painter().line_segment([pp(0., n), pp(10., n)], pen);
            if i % 2 == 0 {
                ui.painter().text(
                    pp(n, 0.),
                    egui::Align2::CENTER_TOP,
                    i,
                    egui::FontId::proportional(12.),
                    theme::NAVY,
                );
                ui.painter().text(
                    pp(0., n) - egui::vec2(4., 0.),
                    egui::Align2::RIGHT_CENTER,
                    i,
                    egui::FontId::proportional(12.),
                    theme::NAVY,
                );
            }
        }
        for (id, _) in &points {
            if let Some((x, y)) = fields
                .get(id)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim_matches(['(', ')'])
                .split_once(',')
            {
                if let (Ok(x), Ok(y)) = (x.trim().parse::<f32>(), y.trim().parse::<f32>()) {
                    if (0.0..=10.).contains(&x) && (0.0..=10.).contains(&y) {
                        ui.painter().circle(
                            pp(x, y),
                            4.5,
                            theme::AMBER,
                            egui::Stroke::new(1.0_f32, theme::NAVY),
                        );
                    }
                }
            }
        }
        if response.clicked() {
            if let Some(pos) = response
                .interact_pointer_pos()
                .filter(|p| graph.contains(*p))
            {
                let x = ((pos.x - graph.left()) * 10. / graph.width()).round();
                let y = ((graph.bottom() - pos.y) * 10. / graph.height()).round();
                fields.insert(points[*point].0.clone(), format!("{x},{y}").into());
                *point = (*point + 1) % points.len();
                changed = true;
            }
        }
    }
    if form["fields"]
        .as_array()
        .is_some_and(|list| list.iter().any(|field| field["kind"] == "arrow"))
    {
        ui.collapsing("Draw a direction arrow", |ui| {
            ui.label("Drag from the driver to the follower, or choose a direction below.");
            let (rect, response) = ui.allocate_exact_size(
                egui::vec2(ui.available_width().min(300.), 68.),
                egui::Sense::drag(),
            );
            let pedal = egui::pos2(rect.left() + 45., rect.center().y);
            let wheel = egui::pos2(rect.right() - 45., rect.center().y);
            for (center, label) in [(pedal, "Pedal"), (wheel, "Wheel")] {
                ui.painter().text(
                    center,
                    egui::Align2::CENTER_CENTER,
                    label,
                    egui::FontId::proportional(15.),
                    theme::NAVY,
                );
            }
            let current = fields.get("arrow").and_then(Value::as_str).unwrap_or("");
            let forward = "Pedal → Wheel";
            let reverse = "Wheel → Pedal";
            let left = pedal + egui::vec2(28., 0.);
            let right = wheel - egui::vec2(28., 0.);
            if current == forward || current == reverse {
                let (start, end) = if current == forward {
                    (left, right)
                } else {
                    (right, left)
                };
                ui.painter()
                    .arrow(start, end - start, egui::Stroke::new(2., theme::NAVY));
            }
            let mut selected = None;
            if response.drag_started() {
                if let Some(start) = ui.input(|i| i.pointer.press_origin()) {
                    ui.data_mut(|d| d.insert_temp(response.id, start));
                }
            }
            if response.drag_stopped() {
                if let Some((start, end)) = ui
                    .data_mut(|d| d.remove_temp::<egui::Pos2>(response.id))
                    .zip(ui.input(|i| i.pointer.latest_pos()))
                {
                    if rect.contains(start)
                        && rect.contains(end)
                        && (start.x < rect.center().x) != (end.x < rect.center().x)
                    {
                        selected = Some(if start.x < end.x { forward } else { reverse });
                    }
                }
            }
            ui.horizontal_wrapped(|ui| {
                for direction in [forward, reverse] {
                    if ui
                        .selectable_label(
                            current == direction,
                            if direction == forward {
                                "Pedal to wheel"
                            } else {
                                "Wheel to pedal"
                            },
                        )
                        .clicked()
                    {
                        selected = Some(direction);
                    }
                }
                if !current.is_empty() && ui.button("Clear arrow").clicked() {
                    selected = Some("");
                }
            });
            if let Some(direction) = selected {
                if direction.is_empty() {
                    fields.remove("arrow");
                } else {
                    fields.insert("arrow".into(), direction.into());
                }
                changed = true;
            }
        });
    }
    if form["id"] == "measure" && ui.button("Run measurement").clicked() {
        cmd = Some("/run".into());
    }
    for (key, label) in [
        ("measurements", "Measurements"),
        ("intervals", "Intervals (ms)"),
    ] {
        if let Some(v) = draft[key].as_array().filter(|a| !a.is_empty()) {
            ui.label(format!(
                "{label} · {}",
                serde_json::to_string(v).unwrap_or_default()
            ));
        }
    }
    if ui.button("Ask for a hint").clicked() {
        cmd = Some("/hint".into());
    }
    (changed, cmd)
}
