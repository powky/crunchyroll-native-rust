//! The on-screen keyboard search types with: a grid of keys in LG's layout (digits, letters, punctuation, then
//! space), moved through with the arrows. A native app gets no system keyboard, so it draws its own.

use crate::gfx::{Color, Font, Icon, Rect, Renderer};
use crate::platform::Key;

use super::theme::*;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Cap {
    Char(char),
    Space,
    Backspace,
    Clear,
    Shift,
    Symbols,
    Done,
}

/// Each row: keys and how many columns each one spans (12 columns).
fn rows(symbols: bool) -> Vec<Vec<(Cap, usize)>> {
    let chars = |text: &str| text.chars().map(|c| (Cap::Char(c), 1)).collect::<Vec<_>>();
    let (first, second, third) = if symbols {
        ("!@#$%&*()-_", "+=/\\:;\"'?¿¡", "[]{}<>~^|`")
    } else {
        ("qwertyuiopñ", "asdfghjkl'-", "zxcvbnm,.!?")
    };
    let mut digits = chars("1234567890");
    digits.push((Cap::Backspace, 2));
    let mut row1 = chars(first);
    row1.push((Cap::Clear, 1));
    let mut row2 = chars(second);
    row2.push((Cap::Char(if symbols { '€' } else { '@' }), 1));
    let mut row3 = vec![(Cap::Shift, 1)];
    row3.extend(chars(third));
    vec![digits, row1, row2, row3, vec![(Cap::Symbols, 2), (Cap::Space, 8), (Cap::Done, 2)]]
}

pub enum Outcome {
    /// nothing for the screen to do
    None,
    /// the text changed
    Changed,
    /// Done, or Back with something typed
    Done,
    /// Up from the top row: focus leaves the keyboard
    LeaveUp,
}

#[derive(Default)]
pub struct Keyboard {
    pub text: String,
    row: usize,
    col: usize,
    shift: bool,
    symbols: bool,
}

const KEY_W: f32 = 96.0;
const KEY_H: f32 = 76.0;
const GAP: f32 = 10.0;
pub const HEIGHT: f32 = 5.0 * KEY_H + 4.0 * GAP + 2.0 * 28.0;

impl Keyboard {
    /// A keyboard that starts with `text` typed.
    pub fn with_text(text: &str) -> Keyboard {
        Keyboard { text: text.to_string(), ..Default::default() }
    }

    pub fn handle(&mut self, key: Key) -> Outcome {
        let layout = rows(self.symbols);
        match key {
            Key::Left => self.col = self.col.saturating_sub(1),
            Key::Right => self.col = (self.col + 1).min(layout[self.row].len() - 1),
            Key::Up => {
                if self.row == 0 {
                    return Outcome::LeaveUp;
                }
                self.move_row(&layout, self.row - 1);
            }
            Key::Down => {
                if self.row + 1 < layout.len() {
                    self.move_row(&layout, self.row + 1);
                }
            }
            Key::Ok => {
                let (cap, _) = layout[self.row][self.col.min(layout[self.row].len() - 1)];
                match cap {
                    Cap::Char(c) => {
                        if self.shift {
                            self.text.extend(c.to_uppercase());
                            self.shift = false;
                        } else {
                            self.text.push(c);
                        }
                        return Outcome::Changed;
                    }
                    Cap::Space => {
                        if !self.text.is_empty() && !self.text.ends_with(' ') {
                            self.text.push(' ');
                            return Outcome::Changed;
                        }
                    }
                    Cap::Backspace => {
                        if self.text.pop().is_some() {
                            return Outcome::Changed;
                        }
                    }
                    Cap::Clear => {
                        if !self.text.is_empty() {
                            self.text.clear();
                            return Outcome::Changed;
                        }
                    }
                    Cap::Shift => self.shift = !self.shift,
                    Cap::Symbols => {
                        self.symbols = !self.symbols;
                        self.col = self.col.min(rows(self.symbols)[self.row].len() - 1);
                    }
                    Cap::Done => return Outcome::Done,
                }
            }
            Key::Back => return Outcome::Done,
            Key::Other(_) => {}
        }
        Outcome::None
    }

    /// Keeps the focus over the same spot when moving to a row with different spans.
    fn move_row(&mut self, layout: &[Vec<(Cap, usize)>], to: usize) {
        let center = column_start(&layout[self.row], self.col) as f32 + layout[self.row][self.col].1 as f32 / 2.0;
        let mut best = 0;
        let mut best_distance = f32::MAX;
        for index in 0..layout[to].len() {
            let c = column_start(&layout[to], index) as f32 + layout[to][index].1 as f32 / 2.0;
            if (c - center).abs() < best_distance {
                best_distance = (c - center).abs();
                best = index;
            }
        }
        self.row = to;
        self.col = best;
    }

    /// Width the keyboard takes.
    pub fn width() -> f32 {
        12.0 * KEY_W + 11.0 * GAP + 2.0 * 28.0
    }

    pub fn draw(&self, gfx: &mut Renderer, x: f32, y: f32, focused: bool) {
        let panel = Rect::new(x, y, Self::width(), HEIGHT);
        gfx.rect(panel, 28.0, Color::rgb(0x1c1c21).alpha(0.97));
        let layout = rows(self.symbols);
        for (r, row) in layout.iter().enumerate() {
            for (c, (cap, span)) in row.iter().enumerate() {
                let start = column_start(row, c) as f32;
                let key = Rect::new(
                    x + 28.0 + start * (KEY_W + GAP),
                    y + 28.0 + r as f32 * (KEY_H + GAP),
                    *span as f32 * KEY_W + (*span as f32 - 1.0) * GAP,
                    KEY_H,
                );
                let focus = focused && r == self.row && c == self.col;
                let (fill, ink) = if focus { (TEXT, BG) } else { (Color(1.0, 1.0, 1.0, 0.08), TEXT) };
                gfx.rect(if focus { key.scaled(1.06) } else { key }, 14.0, fill);
                let cx = key.x + key.w / 2.0;
                let cy = key.y + key.h / 2.0;
                match cap {
                    Cap::Char(ch) => {
                        let label: String = if self.shift { ch.to_uppercase().collect() } else { ch.to_string() };
                        let w = gfx.measure(Font::Regular, 36.0, &label);
                        gfx.text(Font::Regular, 36.0, cx - w / 2.0, cy - 22.0, &label, ink, None);
                    }
                    Cap::Backspace => {
                        gfx.icon_centered(Icon::Back, cx, cy, 34.0, ink);
                    }
                    Cap::Space => label(gfx, cx, cy, "espacio", ink),
                    Cap::Clear => gfx.icon_centered(Icon::Xmark, cx, cy, 30.0, ink),
                    Cap::Shift => {
                        let ink = if self.shift && !focus { ACCENT } else { ink };
                        label(gfx, cx, cy, "Mayús", ink);
                    }
                    Cap::Symbols => label(gfx, cx, cy, if self.symbols { "abc" } else { "#+=" }, ink),
                    Cap::Done => label(gfx, cx, cy, "Buscar", ink),
                }
            }
        }
    }
}

fn label(gfx: &mut Renderer, cx: f32, cy: f32, text: &str, ink: Color) {
    let w = gfx.measure(Font::SemiBold, 26.0, text);
    gfx.text(Font::SemiBold, 26.0, cx - w / 2.0, cy - 16.0, text, ink, None);
}

fn column_start(row: &[(Cap, usize)], index: usize) -> usize {
    row.iter().take(index).map(|(_, span)| span).sum()
}
