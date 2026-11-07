//! QR code du lien magnet, dessiné directement avec le peintre d'egui.

use egui::{Color32, CornerRadius, Pos2, Rect, Sense, Vec2};
use qrcode::{Color, EcLevel, QrCode};

pub fn draw(ui: &mut egui::Ui, text: &str, size: f32) {
    let code = QrCode::with_error_correction_level(text.as_bytes(), EcLevel::L).or_else(|_| QrCode::new(text.as_bytes()));
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let pa = ui.painter_at(rect);
    pa.rect_filled(rect, CornerRadius::same(14), Color32::WHITE);
    let Ok(code) = code else {
        pa.text(rect.center(), egui::Align2::CENTER_CENTER, "QR ✖", egui::FontId::proportional(18.0), Color32::BLACK);
        return;
    };
    let w = code.width();
    let quiet = 2.0;
    let cell = size / (w as f32 + quiet * 2.0);
    let colors = code.to_colors();
    for y in 0..w {
        for x in 0..w {
            if colors[y * w + x] == Color::Dark {
                let min = Pos2::new(rect.left() + (x as f32 + quiet) * cell, rect.top() + (y as f32 + quiet) * cell);
                pa.rect_filled(Rect::from_min_size(min, Vec2::splat(cell + 0.3)), 0.0, Color32::from_rgb(12, 10, 30));
            }
        }
    }
}
