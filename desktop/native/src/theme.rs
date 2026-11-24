use eframe::egui::{self, Color32, Context, FontFamily, FontId, RichText, Ui};
pub const NAVY: Color32 = Color32::from_rgb(23, 51, 76);
pub const SKY: Color32 = Color32::from_rgb(231, 242, 246);
pub const AMBER: Color32 = Color32::from_rgb(230, 159, 0);
pub const EDGE: Color32 = Color32::from_rgb(114, 141, 159);
pub const PANEL: Color32 = Color32::from_rgb(213, 229, 236);
pub fn setup(ctx: &Context) -> egui::TextureHandle {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "Nunito".into(),
        egui::FontData::from_static(include_bytes!("../assets/Nunito-Text.ttf")).into(),
    );
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "Nunito".into());
    fonts.font_data.insert(
        "Nunito Bold".into(),
        egui::FontData::from_static(include_bytes!("../assets/Nunito-Bold.ttf")).into(),
    );
    let mut bold = fonts.families[&FontFamily::Proportional].clone();
    bold.insert(0, "Nunito Bold".into());
    fonts
        .families
        .insert(FontFamily::Name("Nunito Bold".into()), bold);
    ctx.set_fonts(fonts);
    ctx.set_theme(egui::ThemePreference::Light);
    let mut v = egui::Visuals::light();
    v.override_text_color = Some(NAVY);
    v.panel_fill = SKY;
    v.window_fill = SKY;
    v.extreme_bg_color = Color32::from_rgb(245, 249, 250);
    v.widgets.inactive.bg_fill = PANEL;
    v.widgets.inactive.weak_bg_fill = PANEL;
    v.widgets.inactive.bg_stroke = egui::Stroke::new(1.0_f32, EDGE);
    v.widgets.hovered.bg_fill = SKY;
    v.widgets.hovered.weak_bg_fill = SKY;
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.5_f32, NAVY);
    v.selection.bg_fill = AMBER;
    v.selection.stroke = egui::Stroke::new(1.0_f32, NAVY);
    ctx.all_styles_mut(|style| {
        style.visuals = v.clone();
        style.spacing.item_spacing = egui::vec2(9., 8.);
        style.spacing.button_padding = egui::vec2(13., 7.);
        style.spacing.interact_size.y = 31.;
        for (name, size) in [
            (egui::TextStyle::Body, 16.),
            (egui::TextStyle::Button, 15.),
            (egui::TextStyle::Heading, 25.),
            (egui::TextStyle::Small, 13.),
        ] {
            let family = if name == egui::TextStyle::Heading {
                FontFamily::Name("Nunito Bold".into())
            } else {
                FontFamily::Proportional
            };
            style.text_styles.insert(name, FontId::new(size, family));
        }
    });
    let img = logo();
    ctx.load_texture(
        "OLMS Robotutor",
        egui::ColorImage::from_rgba_unmultiplied(
            [img.width as usize, img.height as usize],
            &img.rgba,
        ),
        egui::TextureOptions::LINEAR,
    )
}
pub fn logo() -> egui::IconData {
    let image = image::load_from_memory(include_bytes!("../../../web/brand/robotutor-logo.png"))
        .expect("bundled Robotutor mark")
        .into_rgba8();
    egui::IconData {
        width: image.width(),
        height: image.height(),
        rgba: image.into_raw(),
    }
}
pub fn action(ui: &mut Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).color(NAVY).strong())
            .fill(AMBER)
            .stroke(egui::Stroke::new(1.0_f32, NAVY)),
    )
}
pub fn card(fill: Color32) -> egui::Frame {
    egui::Frame::new()
        .fill(fill)
        .corner_radius(9)
        .inner_margin(14)
}

/// Keep learner text literal. Format only the tutor's supplied Markdown.
pub fn tutor_text(ui: &mut Ui, text: &str, color: Color32, size: f32) {
    let mut job = egui::text::LayoutJob::default();
    let mut code = false;
    for (i, line) in text.lines().enumerate() {
        if line.trim_start().starts_with("```") {
            code = !code;
            continue;
        }
        let base = egui::TextFormat {
            font_id: FontId::proportional(size),
            color,
            ..Default::default()
        };
        if i > 0 && !job.text.is_empty() {
            job.append("\n", 0., base.clone());
        }
        if code {
            job.append(
                line,
                0.,
                egui::TextFormat {
                    font_id: FontId::monospace(size),
                    ..base
                },
            );
            continue;
        }
        let mut value = line;
        let mut format = base;
        if let Some(heading) = line
            .trim_start_matches('#')
            .strip_prefix(' ')
            .filter(|_| line.starts_with('#'))
        {
            value = heading;
            format.font_id = FontId::new(size + 2., FontFamily::Name("Nunito Bold".into()));
        } else if let Some(item) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            job.append("• ", 0., format.clone());
            value = item;
        }
        while !value.is_empty() {
            let next = ["**", "`"]
                .iter()
                .filter_map(|mark| value.find(mark).map(|n| (n, *mark)))
                .min_by_key(|m| m.0);
            let Some((pos, mark)) = next else {
                job.append(value, 0., format.clone());
                break;
            };
            job.append(&value[..pos], 0., format.clone());
            let tail = &value[pos + mark.len()..];
            if let Some(end) = tail.find(mark) {
                let family = if mark == "`" {
                    FontFamily::Monospace
                } else {
                    FontFamily::Name("Nunito Bold".into())
                };
                job.append(
                    &tail[..end],
                    0.,
                    egui::TextFormat {
                        font_id: FontId::new(size, family),
                        ..format.clone()
                    },
                );
                value = &tail[end + mark.len()..];
            } else {
                job.append(mark, 0., format.clone());
                value = tail;
            }
        }
    }
    ui.add(egui::Label::new(job).selectable(true).wrap());
}
