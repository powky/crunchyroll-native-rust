//! The screens before the profiles: the skeleton while the session starts, and the sign-in code.

use crate::gfx::{Font, Icon, Rect};

use super::theme::*;
use super::ui::Ui;

pub const AVATAR_PICTURE: u32 = 480;

/// The Crunchyroll logo where the top bar has it.
pub fn brand(ui: &mut Ui) {
    ui.gfx.icon(Icon::Logo, 48.0, 53.0, 38.0, ACCENT);
}

pub fn draw_starting(ui: &mut Ui) {
    brand(ui);
    ui.skeleton(Rect::new(PAD, 200.0, 560.0, 110.0), 12.0);
    ui.skeleton(Rect::new(PAD, 340.0, 780.0, 30.0), 8.0);
    ui.skeleton(Rect::new(PAD, 390.0, 700.0, 30.0), 8.0);
    for index in 0..5 {
        ui.skeleton(Rect::new(PAD + index as f32 * CARD_STEP, 740.0, CARD_W, CARD_H), RADIUS);
    }
}

pub fn draw_sign_in(ui: &mut Ui, code: Option<&str>, expired: bool) {
    brand(ui);
    ui.gfx.text(Font::Bold, 64.0, PAD, 220.0, "Inicia sesión", TEXT, None);
    ui.gfx.text(Font::Regular, 30.0, PAD, 330.0, "1. En tu móvil o computadora, entra en", MUTED, None);
    ui.gfx.text(Font::SemiBold, 40.0, PAD, 380.0, crunchy_core::auth::ACTIVATE_URL, TEXT, None);
    ui.gfx.text(Font::Regular, 30.0, PAD, 470.0, "2. Escribe este código:", MUTED, None);
    match code {
        Some(code) if !expired => {
            let mut x = PAD;
            for ch in code.to_uppercase().chars() {
                let r = Rect::new(x, 530.0, 96.0, 120.0);
                ui.gfx.rect(r, 18.0, SURFACE);
                ui.text_centered(Font::Bold, 64.0, r.x + r.w / 2.0, r.y + 22.0, &ch.to_string(), TEXT);
                x += 112.0;
            }
            ui.gfx.text(Font::Regular, 24.0, PAD, 690.0, "La pantalla avanzará sola en cuanto lo escribas.", MUTED, None);
        }
        Some(_) => {
            ui.gfx.text(Font::SemiBold, 30.0, PAD, 560.0, "El código caducó. Pulsa OK para pedir otro.", TEXT, None);
        }
        None => {
            for index in 0..6 {
                ui.skeleton(Rect::new(PAD + index as f32 * 112.0, 530.0, 96.0, 120.0), 18.0);
            }
        }
    }
}
