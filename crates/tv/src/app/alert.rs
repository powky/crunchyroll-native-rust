//! The tvOS-style dialog over everything: a title, maybe a message, and Cancelar / Aceptar.

use crate::gfx::{Color, Font, Rect};
use crate::platform::Key;

use super::theme::*;
use super::ui::{Look, Ui};

/// What Aceptar does.
#[derive(Clone, Debug, PartialEq)]
pub enum AlertAction {
    Quit,
    SignOut,
    DeleteProfile(String),
    /// just closes (a notice)
    Close,
}

pub struct Alert {
    title: String,
    message: Option<String>,
    pub action: AlertAction,
    /// 0 is Cancelar, 1 Aceptar (a notice has only Aceptar)
    focus: usize,
}

pub enum Choice {
    Open,
    Cancel,
    Accept(AlertAction),
}

impl Alert {
    pub fn new(title: &str, message: Option<&str>, action: AlertAction) -> Alert {
        Alert { title: title.into(), message: message.map(String::from), action, focus: 0 }
    }

    fn buttons(&self) -> &'static [&'static str] {
        if self.action == AlertAction::Close { &["Aceptar"] } else { &["Cancelar", "Aceptar"] }
    }

    pub fn key(&mut self, key: Key) -> Choice {
        let count = self.buttons().len();
        match key {
            Key::Left => self.focus = self.focus.saturating_sub(1),
            Key::Right => self.focus = (self.focus + 1).min(count - 1),
            Key::Back => return Choice::Cancel,
            Key::Ok => {
                return if self.focus == count - 1 { Choice::Accept(self.action.clone()) } else { Choice::Cancel };
            }
            _ => {}
        }
        Choice::Open
    }

    pub fn draw(&self, ui: &mut Ui) {
        ui.gfx.rect(Rect::new(0.0, 0.0, 1920.0, 1080.0), 0.0, Color(0.0, 0.0, 0.0, 0.5));
        let title_w = ui.gfx.measure(Font::Bold, 36.0, &self.title);
        let lines = self.message.as_ref().map(|m| ui.gfx.wrap(Font::Regular, 24.0, m, 900.0 - 112.0, 6)).unwrap_or_default();
        let message_w = lines.iter().map(|l| ui.gfx.measure(Font::Regular, 24.0, l)).fold(0.0, f32::max);
        let buttons = self.buttons();
        let widths: Vec<f32> = buttons.iter().map(|b| ui.gfx.measure(Font::SemiBold, 23.0, b) + 60.0).collect();
        let buttons_w = widths.iter().sum::<f32>() + 16.0 * (buttons.len() as f32 - 1.0);
        let w = (title_w.max(message_w).max(buttons_w) + 112.0).clamp(640.0, 900.0);
        let h = 48.0 + 44.0 + if lines.is_empty() { 0.0 } else { 16.0 + lines.len() as f32 * 35.0 } + 36.0 + 64.0 + 40.0;
        let card = Rect::new((1920.0 - w) / 2.0, (1080.0 - h) / 2.0, w, h);
        ui.gfx.rect(card.inset(-1.0), 29.0, Color(1.0, 1.0, 1.0, 0.08));
        ui.gfx.rect(card, 28.0, Color(0.15, 0.15, 0.17, 0.97));
        let mut y = card.y + 48.0;
        ui.text_centered(Font::Bold, 36.0, 960.0, y, &self.title, TEXT);
        y += 44.0;
        if !lines.is_empty() {
            y += 16.0;
            for line in &lines {
                ui.text_centered(Font::Regular, 24.0, 960.0, y, line, MUTED);
                y += 35.0;
            }
        }
        y += 36.0;
        let mut x = 960.0 - buttons_w / 2.0;
        for (index, label) in buttons.iter().enumerate() {
            ui.pill(x, y, 64.0, label, None, 23.0, Look::new(index == self.focus, false));
            x += widths[index] + 16.0;
        }
    }
}
