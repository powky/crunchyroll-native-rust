//! Controls of the grid headers: a dropdown, the search field (typed into with the on-screen keyboard) and a row of
//! pills that slides to keep the focused one in view.

use std::time::{Duration, Instant};

use crate::gfx::{Color, Font, Icon, Rect};
use crate::platform::Key;

use super::keyboard::{self, Keyboard, Outcome};
use super::theme::*;
use super::ui::{Look, Ui};

pub struct Dropdown {
    pub options: Vec<String>,
    pub selected: usize,
    /// open, with the option focused
    pub open: Option<usize>,
    pub width: f32,
}

pub enum DropNav {
    /// the key was the dropdown's
    Used,
    /// an option was picked
    Changed,
    /// the key wasn't for it (it's closed)
    Pass,
}

const DROP_H: f32 = 60.0;

impl Dropdown {
    pub fn new(options: &[&str], selected: usize, width: f32) -> Dropdown {
        Dropdown { options: options.iter().map(|s| s.to_string()).collect(), selected, open: None, width }
    }

    pub fn key(&mut self, key: Key) -> DropNav {
        let Some(focus) = self.open.as_mut() else {
            if key == Key::Ok {
                self.open = Some(self.selected);
                return DropNav::Used;
            }
            return DropNav::Pass;
        };
        match key {
            Key::Up => *focus = focus.saturating_sub(1),
            Key::Down => *focus = (*focus + 1).min(self.options.len() - 1),
            Key::Back => self.open = None,
            Key::Ok => {
                let chosen = *focus;
                self.open = None;
                if chosen != self.selected {
                    self.selected = chosen;
                    return DropNav::Changed;
                }
            }
            _ => {}
        }
        DropNav::Used
    }

    pub fn draw(&self, ui: &mut Ui, x: f32, y: f32, focused: bool) {
        let r = Rect::new(x, y, self.width, DROP_H);
        let (fill, ink) = if focused && self.open.is_none() { (TEXT, BG) } else { (Color::rgb(0x2c2c31), TEXT) };
        ui.gfx.rect(r, DROP_H / 2.0, fill);
        if focused && self.open.is_some() {
            ui.gfx.rect(r.inset(-2.0), DROP_H / 2.0 + 2.0, TEXT);
            ui.gfx.rect(r, DROP_H / 2.0, Color::rgb(0x2c2c31));
        }
        let label = self.options.get(self.selected).map(String::as_str).unwrap_or("");
        ui.gfx.text(Font::SemiBold, 28.0, r.x + 24.0, r.y + 13.0, label, ink, Some(r.w - 80.0));
        let icon = if self.open.is_some() { Icon::ChevronDown } else { Icon::ChevronDown };
        ui.gfx.icon(icon, r.x + r.w - 24.0 - 24.0, r.y + 18.0, 24.0, ink);
    }

    /// The open list, drawn last so it covers what's under it.
    pub fn draw_list(&self, ui: &mut Ui, x: f32, y: f32) {
        let Some(focus) = self.open else { return };
        let row_h = 60.0;
        let panel = Rect::new(x, y + DROP_H + 10.0, self.width.max(300.0), 24.0 + self.options.len() as f32 * row_h);
        ui.gfx.rect(panel.inset(-1.0), 25.0, Color(1.0, 1.0, 1.0, 0.10));
        ui.gfx.rect(panel, 24.0, Color(0.141, 0.141, 0.165, 0.98));
        for (index, option) in self.options.iter().enumerate() {
            let r = Rect::new(panel.x + 12.0, panel.y + 12.0 + index as f32 * row_h, panel.w - 24.0, row_h - 4.0);
            let focused = index == focus;
            if focused {
                ui.gfx.rect(r, 14.0, TEXT);
            }
            let ink = if focused { BG } else { TEXT };
            ui.gfx.text(Font::Regular, 27.0, r.x + 18.0, r.y + 12.0, option, ink, Some(r.w - 70.0));
            if index == self.selected {
                ui.gfx.icon(Icon::Check, r.x + r.w - 44.0, r.y + 16.0, 24.0, ink);
            }
        }
    }
}

/// The search field and its keyboard. The text counts as typed a moment after the last key.
pub struct SearchField {
    pub keyboard: Option<Keyboard>,
    text: String,
    /// the text the screen last searched for
    pub applied: String,
    changed_at: Option<Instant>,
    pub width: f32,
}

pub enum FieldNav {
    Used,
    /// the text to search for changed
    Search,
    /// the keyboard handed the focus back up
    Up,
    Pass,
}

const FIELD_H: f32 = 64.0;
/// Wait after the last key before searching.
const DEBOUNCE: Duration = Duration::from_millis(1200);

impl SearchField {
    pub fn new(width: f32) -> SearchField {
        SearchField { keyboard: None, text: String::new(), applied: String::new(), changed_at: None, width }
    }

    pub fn key(&mut self, key: Key) -> FieldNav {
        let Some(keyboard) = self.keyboard.as_mut() else {
            if key == Key::Ok {
                self.keyboard = Some(Keyboard::with_text(&self.text));
                return FieldNav::Used;
            }
            return FieldNav::Pass;
        };
        match keyboard.handle(key) {
            Outcome::None => FieldNav::Used,
            Outcome::Changed => {
                self.text = keyboard.text.clone();
                self.changed_at = Some(Instant::now());
                FieldNav::Used
            }
            Outcome::Done => {
                self.keyboard = None;
                self.changed_at = None;
                self.commit()
            }
            Outcome::LeaveUp => {
                self.keyboard = None;
                self.changed_at = None;
                match self.commit() {
                    FieldNav::Search => FieldNav::Search,
                    _ => FieldNav::Up,
                }
            }
        }
    }

    fn commit(&mut self) -> FieldNav {
        if self.text.trim() != self.applied.trim() {
            self.applied = self.text.clone();
            FieldNav::Search
        } else {
            FieldNav::Used
        }
    }

    /// Searches once typing has paused.
    pub fn tick(&mut self) -> bool {
        if let Some(at) = self.changed_at
            && at.elapsed() >= DEBOUNCE
        {
            self.changed_at = None;
            return matches!(self.commit(), FieldNav::Search);
        }
        false
    }

    pub fn draw(&self, ui: &mut Ui, x: f32, y: f32, focused: bool) {
        let r = Rect::new(x, y, self.width, FIELD_H);
        let active = focused || self.keyboard.is_some();
        if active {
            ui.gfx.rect(r.inset(-3.0), FIELD_H / 2.0 + 3.0, TEXT);
        }
        ui.gfx.rect(r, FIELD_H / 2.0, if active { Color::rgb(0xffffff) } else { Color(1.0, 1.0, 1.0, 0.12) });
        let (ink, hint) = if active { (Color::rgb(0x000000), Color::rgb(0xaaaaaa)) } else { (TEXT, Color(0.92, 0.92, 0.96, 0.5)) };
        ui.gfx.icon(Icon::Search, r.x + 24.0, r.y + 20.0, 24.0, if active { hint } else { MUTED });
        let text_x = r.x + 64.0;
        if self.text.is_empty() {
            ui.gfx.text(Font::Regular, 26.0, text_x, r.y + 17.0, "Buscar", hint, None);
        } else {
            let w = ui.gfx.text(Font::Regular, 26.0, text_x, r.y + 17.0, &self.text, ink, Some(r.w - 90.0));
            if self.keyboard.is_some() && (ui.t * 2.0) as i32 % 2 == 0 {
                ui.gfx.rect(Rect::new(text_x + w + 2.0, r.y + 16.0, 2.0, 32.0), 0.0, ink);
            }
        }
    }

    /// The keyboard, bottom centre, over everything else.
    pub fn draw_keyboard(&self, ui: &mut Ui) {
        if let Some(keyboard) = &self.keyboard {
            let w = Keyboard::width();
            keyboard.draw(ui.gfx, (1920.0 - w) / 2.0, 1080.0 - keyboard::HEIGHT - 24.0, true);
        }
    }
}

/// A row of pills (categories) that slides so the focused one stays clear of the faded edges.
pub struct PillRow {
    pub labels: Vec<String>,
    pub selected: usize,
    pub focus: usize,
    scroll: f32,
}

impl PillRow {
    pub fn new(labels: Vec<String>) -> PillRow {
        PillRow { labels, selected: 0, focus: 0, scroll: 0.0 }
    }

    pub fn draw(&mut self, ui: &mut Ui, x: f32, y: f32, w: f32, focused: bool) {
        let h = 48.0;
        // where each pill sits along the strip
        let mut starts = Vec::with_capacity(self.labels.len());
        let mut at = 0.0;
        for label in &self.labels {
            starts.push(at);
            at += ui.gfx.measure(Font::SemiBold, 22.0, label) + 44.0 + 6.0;
        }
        let total = at;
        if let Some(start) = starts.get(self.focus).copied() {
            let end = starts.get(self.focus + 1).copied().unwrap_or(total) - 6.0;
            let mut target = self.scroll;
            if start - target < 64.0 {
                target = (start - 64.0).max(0.0);
            }
            if end - target > w - 64.0 {
                target = end - w + 64.0;
            }
            target = target.clamp(0.0, (total - w).max(0.0));
            self.scroll = approach(self.scroll, target, ui.dt, 14.0);
        }
        ui.gfx.clip(Some(Rect::new(x, y - 8.0, w, h + 16.0)));
        for (index, label) in self.labels.iter().enumerate() {
            let px = x + starts[index] - self.scroll;
            if px > x + w || px + 400.0 < x {
                continue;
            }
            ui.pill_plain(px, y, h, label, None, 22.0, Look::new(focused && index == self.focus, index == self.selected));
        }
        ui.gfx.clip(None);
        if self.scroll > 1.0 {
            ui.gfx.gradient(Rect::new(x, y - 8.0, 64.0, h + 16.0), BG, BG.alpha(0.0), false);
        }
        if total - self.scroll > w + 1.0 {
            ui.gfx.gradient(Rect::new(x + w - 64.0, y - 8.0, 64.0, h + 16.0), BG.alpha(0.0), BG, false);
        }
    }
}
