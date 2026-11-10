//! Couleurs, polices et style : sobre, contrasté, aux couleurs du site.

use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle, Vec2, Visuals};
use std::sync::Arc;

pub const VIOLET: Color32 = Color32::from_rgb(139, 92, 246);
pub const VIOLET_DARK: Color32 = Color32::from_rgb(112, 66, 222);
pub const CYAN: Color32 = Color32::from_rgb(34, 211, 238);
pub const PINK: Color32 = Color32::from_rgb(236, 72, 153);
pub const GREEN: Color32 = Color32::from_rgb(52, 211, 153);
pub const AMBER: Color32 = Color32::from_rgb(251, 191, 36);
pub const RED: Color32 = Color32::from_rgb(248, 113, 113);

#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Color32,
    pub surface: Color32,
    pub surface2: Color32,
    pub border: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub faint: Color32,
    pub dark: bool,
}

pub fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            bg: Color32::from_rgb(10, 10, 16),
            surface: Color32::from_rgb(20, 20, 30),
            surface2: Color32::from_rgb(29, 29, 42),
            border: Color32::from_rgb(38, 38, 54),
            text: Color32::from_rgb(242, 242, 248),
            muted: Color32::from_rgb(146, 146, 166),
            faint: Color32::from_rgb(92, 92, 112),
            dark,
        }
    } else {
        Palette {
            bg: Color32::from_rgb(246, 246, 250),
            surface: Color32::from_rgb(255, 255, 255),
            surface2: Color32::from_rgb(240, 239, 247),
            border: Color32::from_rgb(226, 224, 238),
            text: Color32::from_rgb(18, 17, 30),
            muted: Color32::from_rgb(104, 102, 124),
            faint: Color32::from_rgb(160, 158, 178),
            dark,
        }
    }
}

/// La graisse demi-grasse d'Inter, pour les titres et les boutons.
pub fn semibold() -> FontFamily {
    FontFamily::Name("semibold".into())
}

pub fn font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

pub fn font_semibold(size: f32) -> FontId {
    FontId::new(size, semibold())
}

/// Inter (licence SIL OFL), réduite aux alphabets latin, grec et cyrillique :
/// 270 Kio au lieu des 1,4 Mio des polices par défaut d'egui.
pub fn install_fonts(ctx: &egui::Context) {
    let mut f = FontDefinitions::empty();
    f.font_data.insert("inter".into(), Arc::new(FontData::from_static(include_bytes!("../assets/fonts/Inter-400.ttf"))));
    f.font_data.insert("inter-semibold".into(), Arc::new(FontData::from_static(include_bytes!("../assets/fonts/Inter-600.ttf"))));
    f.families.insert(FontFamily::Proportional, vec!["inter".into()]);
    f.families.insert(FontFamily::Monospace, vec!["inter".into()]);
    f.families.insert(semibold(), vec!["inter-semibold".into(), "inter".into()]);
    ctx.set_fonts(f);
}

pub fn apply(ctx: &egui::Context, dark: bool, mobile: bool) {
    let p = palette(dark);
    let mut v = if dark { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = p.bg;
    v.window_fill = p.surface;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(20);
    v.window_shadow = Shadow { offset: [0, 16], blur: 48, spread: 0, color: Color32::from_black_alpha(if dark { 150 } else { 36 }) };
    v.popup_shadow = Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(if dark { 120 } else { 30 }) };
    v.menu_corner_radius = CornerRadius::same(14);
    v.extreme_bg_color = p.surface2;
    v.faint_bg_color = p.surface2;
    v.code_bg_color = p.surface2;
    v.hyperlink_color = if dark { Color32::from_rgb(167, 139, 250) } else { VIOLET_DARK };
    v.selection.bg_fill = VIOLET.gamma_multiply(0.45);
    v.selection.stroke = Stroke::new(1.5, VIOLET);
    v.override_text_color = Some(p.text);
    v.text_cursor.stroke = Stroke::new(2.0, VIOLET);
    v.slider_trailing_fill = true;
    let r = CornerRadius::same(12);
    for (w, fill, stroke) in [
        (&mut v.widgets.noninteractive, p.surface, p.border),
        (&mut v.widgets.inactive, p.surface2, Color32::TRANSPARENT),
        (&mut v.widgets.hovered, p.surface2, p.border),
        (&mut v.widgets.active, p.surface2, VIOLET),
        (&mut v.widgets.open, p.surface2, VIOLET),
    ] {
        w.bg_fill = fill;
        w.weak_bg_fill = fill;
        w.bg_stroke = Stroke::new(1.0, stroke);
        w.corner_radius = r;
        w.expansion = 0.0;
    }
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    v.widgets.inactive.fg_stroke = Stroke::new(1.5, p.text);
    v.widgets.hovered.fg_stroke = Stroke::new(1.5, p.text);
    v.widgets.active.fg_stroke = Stroke::new(1.5, p.text);
    ctx.set_visuals(v);

    // Sur téléphone : un peu plus grand, et des cibles d'au moins 44 points.
    let k = if mobile { 1.06 } else { 1.0 };
    ctx.global_style_mut(|s| {
        s.text_styles = [
            (TextStyle::Heading, FontId::new(21.0 * k, semibold())),
            (TextStyle::Body, FontId::new(15.0 * k, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(13.0 * k, FontFamily::Monospace)),
            (TextStyle::Button, FontId::new(15.0 * k, semibold())),
            (TextStyle::Small, FontId::new(12.5 * k, FontFamily::Proportional)),
        ]
        .into();
        s.spacing.item_spacing = Vec2::new(8.0, 8.0);
        s.spacing.button_padding = Vec2::new(14.0, if mobile { 11.0 } else { 7.0 });
        s.spacing.interact_size = Vec2::new(40.0, if mobile { 44.0 } else { 32.0 });
        s.spacing.window_margin = Margin::same(20);
        s.spacing.menu_margin = Margin::same(10);
        s.spacing.icon_width = 20.0;
        s.spacing.icon_spacing = 8.0;
        s.spacing.slider_width = if mobile { 180.0 } else { 220.0 };
        s.spacing.scroll.floating = true;
        s.spacing.scroll.bar_width = 6.0;
        s.interaction.selectable_labels = false;
        s.animation_time = 0.12;
    });
}

/// Mélange deux couleurs.
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}
