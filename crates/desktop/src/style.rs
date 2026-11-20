use eframe::egui::{self, Color32 as C, FontFamily, FontId, Margin, RichText, Stroke, Vec2};
use std::sync::Arc;

const COBALT: C = C::from_rgb(37, 88, 200);
const CREAM: C = C::from_rgb(255, 253, 247);
const YELLOW: C = C::from_rgb(255, 210, 63);
const CORAL: C = C::from_rgb(255, 106, 98);

#[derive(Clone, Copy)]
pub struct Palette {
    pub paper: C,
    pub surface: C,
    pub well: C,
    pub ink: C,
    pub muted: C,
    pub accent: C,
    pub ink_accent: C,
    pub border: C,
    pub success: C,
    pub error: C,
}
impl Palette {
    pub fn new(dark: bool) -> Self {
        if dark {
            Self {
                paper: C::from_rgb(23, 26, 33),
                surface: C::from_rgb(34, 39, 49),
                well: C::from_rgb(33, 47, 71),
                ink: C::from_rgb(242, 245, 252),
                muted: C::from_rgb(180, 190, 207),
                accent: C::from_rgb(153, 186, 255),
                ink_accent: C::from_rgb(13, 32, 68),
                border: C::from_rgb(67, 77, 95),
                success: C::from_rgb(137, 224, 176),
                error: C::from_rgb(255, 155, 143),
            }
        } else {
            Self {
                paper: C::from_rgb(246, 247, 250),
                surface: C::WHITE,
                well: C::from_rgb(235, 240, 251),
                ink: C::from_rgb(27, 33, 45),
                muted: C::from_rgb(81, 92, 112),
                accent: COBALT,
                ink_accent: C::WHITE,
                border: C::from_rgb(204, 211, 225),
                success: C::from_rgb(15, 106, 64),
                error: C::from_rgb(160, 53, 38),
            }
        }
    }
    pub fn for_ui(ui: &egui::Ui) -> Self {
        Self::new(ui.visuals().dark_mode)
    }
    pub fn card(self, space: i8) -> egui::Frame {
        egui::Frame::new()
            .fill(self.surface)
            .stroke(Stroke::new(1., self.border))
            .corner_radius(12)
            .inner_margin(Margin::same(space))
    }
}

pub fn strong(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text).font(FontId::new(size, FontFamily::Name("UI Semibold".into())))
}
pub fn secondary(text: impl Into<String>, p: Palette) -> RichText {
    RichText::new(text).size(13.).color(p.muted)
}
pub fn button(text: &str) -> egui::Button<'_> {
    egui::Button::new(strong(text, 14.))
        .min_size(Vec2::new(0., 31.))
        .corner_radius(7)
}
pub fn primary(text: &str, p: Palette) -> egui::Button<'_> {
    egui::Button::new(strong(text, 14.).color(p.ink_accent))
        .min_size(Vec2::new(104., 34.))
        .corner_radius(8)
        .fill(p.accent)
        .stroke(Stroke::new(1., p.accent))
}
pub fn configure(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let fallback = || {
        (
            egui::FontData::from_static(include_bytes!("../assets/fonts/Barlow-Regular.ttf")),
            egui::FontData::from_static(include_bytes!("../assets/fonts/Barlow-SemiBold.ttf")),
        )
    };
    // The macOS system face stays on the machine; the bundled OFL fallback works elsewhere.
    #[cfg(target_os = "macos")]
    let (body, semibold) = if let Ok(bytes) = std::fs::read("/System/Library/Fonts/SFNS.ttf") {
        let mut system = egui::FontData::from_owned(bytes);
        system.tweak.coords.push(b"wght", 400.);
        let body = system.clone();
        system.tweak.coords = egui::epaint::text::VariationCoords::new([(b"wght", 610.)]);
        (body, system)
    } else {
        fallback()
    };
    #[cfg(not(target_os = "macos"))]
    let (body, semibold) = fallback();
    fonts.font_data.insert("UI Body".into(), Arc::new(body));
    fonts
        .font_data
        .insert("UI Semibold".into(), Arc::new(semibold));
    let family = fonts.families.entry(FontFamily::Proportional).or_default();
    family.insert(0, "UI Body".into());
    let mut heavy = family.clone();
    heavy.insert(0, "UI Semibold".into());
    fonts
        .families
        .insert(FontFamily::Name("UI Semibold".into()), heavy);
    ctx.set_fonts(fonts);
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        let p = Palette::new(theme == egui::Theme::Dark);
        ctx.style_mut_of(theme, |s| {
            s.spacing.item_spacing = Vec2::new(9., 8.);
            s.spacing.button_padding = Vec2::new(11., 5.);
            s.spacing.interact_size.y = 25.;
            s.text_styles
                .insert(egui::TextStyle::Body, FontId::proportional(14.));
            s.text_styles
                .insert(egui::TextStyle::Button, FontId::proportional(14.));
            let v = &mut s.visuals;
            v.panel_fill = p.paper;
            v.window_fill = p.surface;
            v.override_text_color = Some(p.ink);
            v.hyperlink_color = p.accent;
            v.window_stroke = Stroke::new(1., p.border);
            v.window_corner_radius = 12.into();
            v.selection.bg_fill = p.well;
            v.selection.stroke = Stroke::new(1.5, p.accent);
            v.faint_bg_color = p.well;
            v.extreme_bg_color = p.paper;
            v.widgets.inactive.weak_bg_fill = p.surface;
            v.widgets.inactive.bg_stroke = Stroke::new(1., p.border);
            v.widgets.hovered.weak_bg_fill = p.well;
            v.widgets.hovered.bg_stroke = Stroke::new(1., p.accent);
            v.widgets.active.weak_bg_fill = p.well;
            v.widgets.active.bg_stroke = Stroke::new(1.5, p.accent);
        });
    }
}

pub fn brand(ui: &mut egui::Ui, size: f32) {
    let (r, response) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, "PixMinima"));
    let painter = ui.painter();
    painter.rect_filled(r, size * 0.24, COBALT);
    for (x, color) in [(6.0, CREAM), (11.5, YELLOW), (17.0, CORAL)] {
        let origin = r.min + size * Vec2::new(x / 32., 22. / 32.);
        painter.line_segment(
            [origin, origin + size * Vec2::new(9. / 32., -12. / 32.)],
            Stroke::new(0.09 * size, color),
        );
    }
}

// Paired photo outlines give the empty drop surface the same meaning in each language.
pub fn photographs(ui: &mut egui::Ui, p: Palette) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(52., 40.), egui::Sense::hover());
    let pen = Stroke::new(1.8, p.accent);
    let painter = ui.painter();
    let back = egui::Rect::from_min_size(r.min + Vec2::new(1., 1.), Vec2::new(33., 26.));
    painter.rect_stroke(back, 5., pen, egui::StrokeKind::Inside);
    let front = egui::Rect::from_min_size(r.min + Vec2::new(10., 8.), Vec2::new(39., 29.));
    painter.rect_filled(front, 5., p.well);
    painter.rect_stroke(front, 5., pen, egui::StrokeKind::Inside);
    let pt = |x, y| front.min + Vec2::new(x, y);
    painter.circle_filled(pt(11., 8.), 2.7, YELLOW);
    painter.circle_stroke(pt(11., 8.), 2.7, Stroke::new(0.8, p.accent));
    painter.line_segment([pt(4., 24.), pt(14., 14.)], pen);
    painter.line_segment([pt(14., 14.), pt(19., 18.)], pen);
    painter.line_segment([pt(19., 18.), pt(25., 11.)], pen);
    painter.line_segment([pt(25., 11.), pt(35., 22.)], pen);
}
