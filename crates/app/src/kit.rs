//! Petits composants dessinés à la main, pour une interface cohérente :
//! boutons, puces, barres et anneau de progression, cartes, titres de section.

use crate::icons::{self, Icon};
use crate::theme::{Palette, VIOLET, VIOLET_DARK, font, font_semibold};
use egui::{Align2, Color32, CornerRadius, Response, Sense, Stroke, Ui, Vec2, pos2, vec2};
use mxtorrent_core::State;

pub enum Kind {
    Primary,
    Secondary,
    Danger,
    Ghost,
}

/// Bouton avec icône facultative. `full` : toute la largeur disponible.
pub fn button(ui: &mut Ui, p: Palette, kind: Kind, icon: Option<Icon>, text: &str, full: bool) -> Response {
    let h = if ui.ctx().content_rect().width() < 760.0 { 48.0 } else { 40.0 };
    let galley = ui.painter().layout_no_wrap(text.to_string(), font_semibold(15.0), Color32::WHITE);
    let icon_w = if icon.is_some() { 20.0 + 8.0 } else { 0.0 };
    let content_w = galley.size().x + icon_w;
    let w = if full { ui.available_width() } else { content_w + 36.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
    if ui.is_rect_visible(rect) {
        let pressed = resp.is_pointer_button_down_on();
        let hovered = resp.hovered();
        let (fill, fg, stroke) = match kind {
            Kind::Primary => (
                if pressed {
                    VIOLET_DARK
                } else if hovered {
                    crate::theme::mix(VIOLET, Color32::WHITE, 0.08)
                } else {
                    VIOLET
                },
                Color32::WHITE,
                Stroke::NONE,
            ),
            Kind::Secondary => {
                (if pressed { p.border } else { p.surface2 }, p.text, Stroke::new(1.0, if hovered { p.faint } else { p.border }))
            }
            Kind::Danger => (if pressed { p.border } else { p.surface2 }, crate::theme::RED, Stroke::new(1.0, p.border)),
            Kind::Ghost => (if pressed || hovered { p.surface2 } else { Color32::TRANSPARENT }, p.text, Stroke::NONE),
        };
        let r = CornerRadius::same(14);
        ui.painter().rect(rect, r, fill, stroke, egui::StrokeKind::Inside);
        let x0 = rect.center().x - content_w / 2.0;
        if let Some(ic) = icon {
            let ir = egui::Rect::from_center_size(pos2(x0 + 10.0, rect.center().y), Vec2::splat(20.0));
            icons::paint(ui.painter(), ir, ic, fg);
        }
        ui.painter().text(pos2(x0 + icon_w, rect.center().y), Align2::LEFT_CENTER, text, font_semibold(15.0), fg);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Bouton rond avec une icône seule.
pub fn icon_button(ui: &mut Ui, p: Palette, icon: Icon, size: f32, hint: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    if ui.is_rect_visible(rect) {
        let fill = if resp.is_pointer_button_down_on() {
            p.border
        } else if resp.hovered() {
            p.surface2
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().circle_filled(rect.center(), size / 2.0, fill);
        icons::paint(ui.painter(), rect.shrink(size * 0.24), icon, p.text);
    }
    resp.on_hover_text(hint).on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Action en tuile : icône au-dessus, libellé dessous (barre d'actions des détails).
pub fn tile(ui: &mut Ui, p: Palette, icon: Icon, text: &str, color: Color32, w: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 64.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let fill = if resp.is_pointer_button_down_on() { p.border } else { p.surface };
        ui.painter().rect(rect, CornerRadius::same(16), fill, Stroke::new(1.0, p.border), egui::StrokeKind::Inside);
        icons::paint(ui.painter(), egui::Rect::from_center_size(rect.center() - vec2(0.0, 10.0), Vec2::splat(22.0)), icon, color);
        ui.painter().text(rect.center() + vec2(0.0, 15.0), Align2::CENTER_CENTER, text, font_semibold(12.5), color);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Contrôle segmenté (filtres, onglets) ; renvoie l'index choisi.
pub fn segmented(ui: &mut Ui, p: Palette, items: &[String], selected: &mut usize) -> bool {
    let h = 36.0;
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(12), p.surface2);
    let n = items.len().max(1);
    let seg = (w - 6.0) / n as f32;
    let mut changed = false;
    for (i, label) in items.iter().enumerate() {
        let r = egui::Rect::from_min_size(pos2(rect.left() + 3.0 + seg * i as f32, rect.top() + 3.0), vec2(seg, h - 6.0));
        let resp = ui.interact(r, ui.scope_id().with(("seg", i, label)), Sense::click());
        if resp.clicked() && *selected != i {
            *selected = i;
            changed = true;
        }
        let on = *selected == i;
        if on {
            ui.painter().rect(r, CornerRadius::same(10), p.surface, Stroke::new(1.0, p.border), egui::StrokeKind::Inside);
        }
        ui.painter().text(r.center(), Align2::CENTER_CENTER, label, font_semibold(13.5), if on { p.text } else { p.muted });
    }
    changed
}

/// Petite pastille d'état.
pub fn chip(ui: &mut Ui, text: &str, color: Color32) {
    let galley = ui.painter().layout_no_wrap(text.to_string(), font_semibold(12.0), color);
    let (rect, _) = ui.allocate_exact_size(galley.size() + vec2(18.0, 8.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(255), color.gamma_multiply(0.15));
    ui.painter().circle_filled(pos2(rect.left() + 8.0, rect.center().y), 3.0, color);
    ui.painter().galley(pos2(rect.left() + 14.0, rect.top() + 4.0), galley, color);
}

pub fn state_color(s: State) -> Color32 {
    match s {
        State::FetchingMetadata => crate::theme::AMBER,
        State::Downloading => VIOLET,
        State::Seeding => crate::theme::GREEN,
        State::Paused => Color32::from_rgb(130, 130, 150),
    }
}

/// Barre de progression pleine, sans animation.
pub fn bar(ui: &mut Ui, p: Palette, frac: f32, color: Color32, h: f32) {
    let w = ui.available_width().max(h * 2.0);
    let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let r = CornerRadius::same((h / 2.0) as u8);
    ui.painter().rect_filled(rect, r, p.surface2);
    let f = frac.clamp(0.0, 1.0);
    if f > 0.0 {
        let fill = egui::Rect::from_min_size(rect.min, vec2((w * f).max(h), h));
        ui.painter().rect_filled(fill, r, color);
    }
}

/// Anneau de progression avec le pourcentage au centre.
pub fn ring(ui: &mut Ui, p: Palette, frac: f32, color: Color32, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let c = rect.center();
    let r = size / 2.0 - 6.0;
    let w = 8.0;
    ui.painter().circle_stroke(c, r, Stroke::new(w, p.surface2));
    let f = frac.clamp(0.0, 1.0);
    if f > 0.0 {
        let n = (96.0 * f).ceil().max(2.0) as usize;
        let a0 = -std::f32::consts::FRAC_PI_2;
        let pts: Vec<egui::Pos2> = (0..=n)
            .map(|i| {
                let a = a0 + std::f32::consts::TAU * f * i as f32 / n as f32;
                c + vec2(a.cos(), a.sin()) * r
            })
            .collect();
        ui.painter().add(egui::Shape::line(pts, Stroke::new(w, color)));
        if f < 1.0 {
            for a in [a0, a0 + std::f32::consts::TAU * f] {
                ui.painter().circle_filled(c + vec2(a.cos(), a.sin()) * r, w / 2.0, color);
            }
        }
    }
    let pct = format!("{:.0} %", (f * 100.0).floor());
    ui.painter().text(c, Align2::CENTER_CENTER, pct, font_semibold(size * 0.2), p.text);
}

/// Carte : fond, bordure fine, coins arrondis.
pub fn card<R>(ui: &mut Ui, p: Palette, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(p.surface)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(18))
        .inner_margin(16)
        .show(ui, |ui| {
            ui.set_width(ui.available_width().max(1.0));
            ui.spacing_mut().interact_size.y = 22.0;
            add(ui)
        })
        .inner
}

/// Titre de section, discret, en capitales.
pub fn section(ui: &mut Ui, p: Palette, text: &str) {
    ui.add_space(6.0);
    ui.label(egui::RichText::new(text.to_uppercase()).font(font_semibold(11.5)).color(p.muted).extra_letter_spacing(0.8));
}

pub fn muted(ui: &mut Ui, p: Palette, text: &str) {
    ui.label(egui::RichText::new(text).font(font(13.0)).color(p.muted));
}

/// Ligne « libellé … valeur » des listes de réglages et de détails (hauteur fixe).
pub fn row(ui: &mut Ui, p: Palette, label: &str, value: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
    let pa = ui.painter_at(rect);
    let lg = pa.layout_no_wrap(label.to_string(), font(14.5), p.muted);
    let max_v = (rect.width() - lg.size().x - 16.0).max(40.0);
    let mut job = egui::text::LayoutJob::simple_singleline(value.to_string(), font_semibold(14.5), p.text);
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_v);
    let vg = pa.layout_job(job);
    pa.galley(pos2(rect.left(), rect.center().y - lg.size().y / 2.0), lg, p.muted);
    pa.galley(pos2(rect.right() - vg.size().x, rect.center().y - vg.size().y / 2.0), vg, p.text);
}

/// Réglage numérique : « − valeur + ».
pub fn stepper(ui: &mut Ui, p: Palette, label: &str, value: &mut usize, min: usize, max: usize, step: usize) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.set_min_height(44.0);
        ui.label(egui::RichText::new(label).font(font(14.5)));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if icon_button(ui, p, Icon::Plus, 36.0, "+").clicked() && *value < max {
                *value = (*value + step).min(max);
                changed = true;
            }
            ui.add_sized(vec2(34.0, 36.0), egui::Label::new(egui::RichText::new(value.to_string()).font(font_semibold(15.0))));
            if icon_button(ui, p, Icon::Minus, 36.0, "−").clicked() && *value > min {
                *value = value.saturating_sub(step).max(min);
                changed = true;
            }
        });
    });
    changed
}

/// Champ de texte aux couleurs de l'application.
pub fn field(ui: &mut Ui, p: Palette, text: &mut String, hint: &str) -> Response {
    egui::Frame::new()
        .fill(p.surface2)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(egui::Margin::symmetric(14, 2))
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::singleline(text)
                    .hint_text(egui::RichText::new(hint).color(p.faint))
                    .frame(egui::Frame::NONE)
                    .desired_width(f32::INFINITY)
                    .font(font(15.5)),
            )
        })
        .inner
}
