//! Icônes dessinées au trait (aucune police d'emoji) : nettes à toutes les tailles,
//! de la même épaisseur partout, et de la couleur du texte.

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, Vec2, pos2};
use std::f32::consts::{PI, TAU};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Plus,
    Minus,
    Settings,
    Back,
    Pause,
    Play,
    Trash,
    Link,
    Share,
    Download,
    Upload,
    Eye,
    File,
    Magnet,
    Close,
    Check,
    Qr,
    Globe,
}

/// Dessine `icon` dans `rect` (un carré), avec un trait de la couleur donnée.
pub fn paint(p: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let s = rect.width().min(rect.height());
    let c = rect.center();
    let w = (s * 0.085).max(1.4);
    let st = Stroke::new(w, color);
    // Coordonnées dans un carré de 24 unités centré sur l'icône.
    let u = s / 24.0;
    let at = |x: f32, y: f32| pos2(c.x + (x - 12.0) * u, c.y + (y - 12.0) * u);
    let line = |pts: &[(f32, f32)]| {
        p.add(Shape::line(pts.iter().map(|&(x, y)| at(x, y)).collect(), st));
    };
    let closed = |pts: &[(f32, f32)]| {
        p.add(Shape::closed_line(pts.iter().map(|&(x, y)| at(x, y)).collect(), st));
    };
    let arc = |cx: f32, cy: f32, r: f32, a0: f32, a1: f32| {
        let n = 24;
        let pts: Vec<Pos2> = (0..=n)
            .map(|i| {
                let a = a0 + (a1 - a0) * i as f32 / n as f32;
                at(cx + r * a.cos(), cy + r * a.sin())
            })
            .collect();
        p.add(Shape::line(pts, st));
    };
    match icon {
        Icon::Plus => {
            line(&[(12.0, 5.0), (12.0, 19.0)]);
            line(&[(5.0, 12.0), (19.0, 12.0)]);
        }
        Icon::Minus => line(&[(5.0, 12.0), (19.0, 12.0)]),
        Icon::Close => {
            line(&[(6.0, 6.0), (18.0, 18.0)]);
            line(&[(18.0, 6.0), (6.0, 18.0)]);
        }
        Icon::Check => line(&[(5.0, 12.5), (10.0, 17.5), (19.0, 7.0)]),
        Icon::Back => line(&[(15.0, 5.0), (8.0, 12.0), (15.0, 19.0)]),
        Icon::Settings => {
            // Engrenage : une couronne de 8 dents et un moyeu.
            let teeth = 8;
            let pts: Vec<Pos2> = (0..teeth * 4)
                .map(|i| {
                    let a = (i as f32 / (teeth * 4) as f32) * TAU - PI / 2.0 + PI / (teeth * 4) as f32;
                    let r = if (i / 2) % 2 == 0 { 9.2 } else { 7.0 };
                    at(12.0 + r * a.cos(), 12.0 + r * a.sin())
                })
                .collect();
            p.add(Shape::closed_line(pts, st));
            p.circle(c, 3.0 * u, Color32::TRANSPARENT, st);
        }
        Icon::Pause => {
            p.rect_filled(Rect::from_min_max(at(7.0, 5.0), at(10.0, 19.0)), 1.0 * u, color);
            p.rect_filled(Rect::from_min_max(at(14.0, 5.0), at(17.0, 19.0)), 1.0 * u, color);
        }
        Icon::Play => {
            p.add(Shape::convex_polygon(vec![at(8.0, 5.0), at(19.0, 12.0), at(8.0, 19.0)], color, Stroke::new(w * 0.6, color)));
        }
        Icon::Trash => {
            line(&[(4.0, 7.0), (20.0, 7.0)]);
            line(&[(9.5, 7.0), (10.0, 4.0), (14.0, 4.0), (14.5, 7.0)]);
            closed(&[(6.0, 7.0), (7.0, 20.0), (17.0, 20.0), (18.0, 7.0)]);
            line(&[(10.0, 11.0), (10.0, 16.0)]);
            line(&[(14.0, 11.0), (14.0, 16.0)]);
        }
        Icon::Link => {
            // Deux maillons inclinés.
            let pill = |dx: f32, dy: f32| {
                let r = 3.2;
                let a = -PI / 4.0;
                let (ca, sa) = (a.cos(), a.sin());
                let pts: Vec<Pos2> = (0..=40)
                    .map(|i| {
                        let t = i as f32 / 40.0 * TAU;
                        let (x, y) = if t < PI {
                            (4.0 + r * (t - PI / 2.0).cos(), r * (t - PI / 2.0).sin())
                        } else {
                            (-4.0 + r * (t - PI / 2.0).cos(), r * (t - PI / 2.0).sin())
                        };
                        at(12.0 + dx + x * ca - y * sa, 12.0 + dy + x * sa + y * ca)
                    })
                    .collect();
                p.add(Shape::closed_line(pts, st));
            };
            pill(-2.6, 2.6);
            pill(2.6, -2.6);
        }
        Icon::Share => {
            line(&[(12.0, 3.5), (12.0, 15.0)]);
            line(&[(7.5, 8.0), (12.0, 3.5), (16.5, 8.0)]);
            line(&[(8.0, 11.0), (5.5, 11.0), (5.5, 20.5), (18.5, 20.5), (18.5, 11.0), (16.0, 11.0)]);
        }
        Icon::Download => {
            line(&[(12.0, 4.0), (12.0, 15.0)]);
            line(&[(7.0, 10.5), (12.0, 15.5), (17.0, 10.5)]);
            line(&[(5.0, 15.5), (5.0, 20.0), (19.0, 20.0), (19.0, 15.5)]);
        }
        Icon::Upload => {
            line(&[(12.0, 20.0), (12.0, 5.0)]);
            line(&[(6.5, 10.5), (12.0, 5.0), (17.5, 10.5)]);
        }
        Icon::Eye => {
            let top: Vec<(f32, f32)> = (0..=20)
                .map(|i| {
                    let t = i as f32 / 20.0;
                    (2.5 + 19.0 * t, 12.0 - 6.5 * (t * PI).sin())
                })
                .collect();
            let bottom: Vec<(f32, f32)> = top.iter().map(|&(x, y)| (x, 24.0 - y)).collect();
            line(&top);
            line(&bottom);
            p.circle(at(12.0, 12.0), 3.2 * u, Color32::TRANSPARENT, st);
        }
        Icon::File => {
            closed(&[(6.0, 3.0), (14.0, 3.0), (19.0, 8.0), (19.0, 21.0), (6.0, 21.0)]);
            line(&[(14.0, 3.0), (14.0, 8.0), (19.0, 8.0)]);
        }
        Icon::Magnet => {
            arc(12.0, 11.0, 6.5, 0.0, PI);
            line(&[(5.5, 11.0), (5.5, 4.0)]);
            line(&[(18.5, 11.0), (18.5, 4.0)]);
            line(&[(2.5, 7.0), (8.5, 7.0)]);
            line(&[(15.5, 7.0), (21.5, 7.0)]);
            // Le U du bas est plus épais dans la réalité : on le suggère par l'arc.
            arc(12.0, 11.0, 6.5 + w, 0.15, PI - 0.15);
        }
        Icon::Qr => {
            for (x, y) in [(4.0, 4.0), (14.0, 4.0), (4.0, 14.0)] {
                p.rect_stroke(Rect::from_min_max(at(x, y), at(x + 6.0, y + 6.0)), 1.0 * u, st, egui::StrokeKind::Middle);
            }
            for (x, y) in [(15.0, 15.0), (18.5, 18.5), (15.0, 18.5), (18.5, 15.0)] {
                p.rect_filled(Rect::from_center_size(at(x, y), Vec2::splat(2.4 * u)), 0.5 * u, color);
            }
        }
        Icon::Globe => {
            p.circle(c, 9.0 * u, Color32::TRANSPARENT, st);
            line(&[(3.0, 12.0), (21.0, 12.0)]);
            let mer: Vec<(f32, f32)> = (0..=20)
                .map(|i| {
                    let a = -PI / 2.0 + PI * i as f32 / 20.0;
                    (12.0 + 4.0 * a.cos(), 12.0 + 9.0 * a.sin())
                })
                .collect();
            line(&mer);
            let mer2: Vec<(f32, f32)> = mer.iter().map(|&(x, y)| (24.0 - x, y)).collect();
            line(&mer2);
        }
    }
}

/// Le logo : trois orbites de tailles différentes et leurs pairs, autour d'un noyau.
/// Dessiné une fois, sans animation (aucun rafraîchissement inutile).
pub fn logo(p: &Painter, rect: Rect) {
    use crate::theme::{CYAN, PINK, VIOLET, mix};
    let c = rect.center();
    let s = rect.width() / 2.0;
    let orbits = [(VIOLET, 0.92, 0.3, -24.0_f32, 0.28), (CYAN, 0.74, 0.23, 42.0, 0.62), (PINK, 0.55, 0.17, 100.0, 0.85)];
    for (k, (col, rx, ry, deg, dot)) in orbits.into_iter().enumerate() {
        let tilt = deg.to_radians();
        let at = |a: f32| {
            let (x, y) = (a.cos() * s * rx, a.sin() * s * ry);
            pos2(c.x + x * tilt.cos() - y * tilt.sin(), c.y + x * tilt.sin() + y * tilt.cos())
        };
        let pts: Vec<Pos2> = (0..=64).map(|i| at(i as f32 / 64.0 * TAU)).collect();
        p.add(Shape::line(pts, Stroke::new((s * (0.085 - k as f32 * 0.014)).max(1.3), col)));
        p.circle_filled(at(dot * TAU), (s * (0.11 - k as f32 * 0.017)).max(2.0), col);
    }
    p.circle_filled(c, s * 0.24, mix(VIOLET, CYAN, 0.55));
    p.circle_filled(c - Vec2::splat(s * 0.07), s * 0.08, Color32::from_white_alpha(110));
}
