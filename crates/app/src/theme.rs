//! Couleurs et style : les mêmes que le site (violet électrique vers cyan).

use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle, Vec2, Visuals};

pub const VIOLET: Color32 = Color32::from_rgb(139, 92, 246);
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
    pub dark: bool,
}

pub fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            bg: Color32::from_rgb(7, 7, 13),
            surface: Color32::from_rgb(16, 16, 28),
            surface2: Color32::from_rgb(26, 26, 44),
            border: Color32::from_rgb(40, 40, 66),
            text: Color32::from_rgb(236, 236, 250),
            muted: Color32::from_rgb(150, 150, 180),
            dark,
        }
    } else {
        Palette {
            bg: Color32::from_rgb(246, 245, 255),
            surface: Color32::from_rgb(255, 255, 255),
            surface2: Color32::from_rgb(238, 236, 252),
            border: Color32::from_rgb(220, 216, 245),
            text: Color32::from_rgb(20, 18, 40),
            muted: Color32::from_rgb(100, 98, 130),
            dark,
        }
    }
}

pub fn apply(ctx: &egui::Context, dark: bool, mobile: bool) {
    let p = palette(dark);
    let mut v = if dark { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = p.bg;
    v.window_fill = p.surface;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(18);
    v.window_shadow = Shadow { offset: [0, 12], blur: 40, spread: 0, color: Color32::from_black_alpha(if dark { 160 } else { 40 }) };
    v.menu_corner_radius = CornerRadius::same(12);
    v.extreme_bg_color = if dark { Color32::from_rgb(10, 10, 20) } else { Color32::from_rgb(250, 250, 255) };
    v.faint_bg_color = p.surface2;
    v.code_bg_color = p.surface2;
    v.hyperlink_color = CYAN;
    v.selection.bg_fill = VIOLET.gamma_multiply(0.55);
    v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    v.override_text_color = Some(p.text);
    let r = CornerRadius::same(10);
    for (w, fill, stroke) in [
        (&mut v.widgets.noninteractive, p.surface, p.border),
        (&mut v.widgets.inactive, p.surface2, p.border),
        (&mut v.widgets.hovered, p.surface2.lerp_to_gamma(VIOLET, 0.25), VIOLET),
        (&mut v.widgets.active, VIOLET.gamma_multiply(0.8), VIOLET),
        (&mut v.widgets.open, p.surface2, VIOLET),
    ] {
        w.bg_fill = fill;
        w.weak_bg_fill = fill;
        w.bg_stroke = Stroke::new(1.0, stroke);
        w.corner_radius = r;
    }
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, p.text);
    v.widgets.hovered.fg_stroke = Stroke::new(1.5, p.text);
    v.widgets.active.fg_stroke = Stroke::new(1.5, Color32::WHITE);
    ctx.set_visuals(v);

    let scale = if mobile { 1.12 } else { 1.0 };
    ctx.global_style_mut(|s| {
        s.text_styles = [
            (TextStyle::Heading, FontId::new(22.0 * scale, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(14.5 * scale, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(13.0 * scale, FontFamily::Monospace)),
            (TextStyle::Button, FontId::new(14.5 * scale, FontFamily::Proportional)),
            (TextStyle::Small, FontId::new(11.5 * scale, FontFamily::Proportional)),
        ]
        .into();
        s.spacing.item_spacing = Vec2::new(8.0, 8.0);
        s.spacing.button_padding = if mobile { Vec2::new(14.0, 9.0) } else { Vec2::new(12.0, 6.0) };
        s.spacing.interact_size = Vec2::new(40.0, if mobile { 40.0 } else { 28.0 });
        s.spacing.window_margin = Margin::same(18);
        s.spacing.scroll.floating = true;
    });
}

/// Mélange deux couleurs.
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}
