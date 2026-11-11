//! "¿Quién se irá de aventura?": the account's profiles, each with a pencil to edit it, Agregar when the plan has
//! room, and in the corner Ajustes and Cerrar Sesión. Back asks before leaving the app.

use crate::gfx::{Color, Font, Icon, Rect};
use crate::platform::Key;

use super::alert::{Alert, AlertAction};
use super::signin::AVATAR_PICTURE;
use super::theme::*;
use super::ui::{Look, Ui};
use super::{Env, Nav, State};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Corner(usize),
    Avatar(usize),
    Pencil(usize),
    Add,
}

const CORNER: [Icon; 2] = [Icon::Gear, Icon::Power];

pub struct ProfilesScreen {
    focus: Focus,
}

impl ProfilesScreen {
    pub fn new() -> ProfilesScreen {
        ProfilesScreen { focus: Focus::Avatar(0) }
    }

    pub fn key(&mut self, key: Key, env: &mut Env) -> Nav {
        let profiles = &env.state.account.profiles;
        let count = profiles.list.len();
        let can_add = profiles.can_add;
        let last = count.saturating_sub(1);
        self.focus = match (self.focus, key) {
            (Focus::Corner(i), Key::Left) => Focus::Corner(i.saturating_sub(1)),
            (Focus::Corner(i), Key::Right) => Focus::Corner((i + 1).min(CORNER.len() - 1)),
            (Focus::Corner(_), Key::Down) if count > 0 => Focus::Avatar(0),
            (Focus::Avatar(i), Key::Left) => Focus::Avatar(i.saturating_sub(1)),
            (Focus::Avatar(i), Key::Right) => Focus::Avatar((i + 1).min(last)),
            (Focus::Avatar(_), Key::Up) => Focus::Corner(0),
            (Focus::Avatar(i), Key::Down) => Focus::Pencil(i),
            (Focus::Pencil(i), Key::Left) => Focus::Pencil(i.saturating_sub(1)),
            (Focus::Pencil(i), Key::Right) => Focus::Pencil((i + 1).min(last)),
            (Focus::Pencil(i), Key::Up) => Focus::Avatar(i),
            (Focus::Pencil(_), Key::Down) if can_add => Focus::Add,
            (Focus::Add, Key::Up) if count > 0 => Focus::Pencil(count / 2),
            (_, Key::Back) => {
                return Nav::Alert(Alert::new("¿Salir de la aplicación?", None, AlertAction::Quit));
            }
            (focus, Key::Ok) => {
                return match focus {
                    Focus::Avatar(i) => match env.state.account.profiles.list.get(i) {
                        Some(profile) => Nav::SelectProfile(profile.id.clone()),
                        None => Nav::Stay,
                    },
                    Focus::Pencil(i) => match env.state.account.profiles.list.get(i) {
                        Some(profile) => Nav::Push(Box::new(super::profile_edit::ProfileEdit::edit(profile.clone()))),
                        None => Nav::Stay,
                    },
                    Focus::Add => Nav::Push(Box::new(super::profile_edit::ProfileEdit::create())),
                    Focus::Corner(0) => Nav::Push(Box::new(super::settings::SettingsPage::new(true))),
                    Focus::Corner(_) => Nav::Alert(Alert::new(
                        "Cerrar Sesión",
                        Some("¿Estás seguro de que quieres cerrar sesión?"),
                        AlertAction::SignOut,
                    )),
                };
            }
            (focus, _) => focus,
        };
        Nav::Stay
    }

    pub fn draw(&mut self, ui: &mut Ui, state: &State) {
        // a warm glow from the top
        ui.gfx.gradient(Rect::new(0.0, 0.0, 1920.0, 640.0), Color(1.0, 0.37, 0.0, 0.12), Color(1.0, 0.37, 0.0, 0.0), true);

        let list = &state.account.profiles.list;
        if let Focus::Avatar(i) | Focus::Pencil(i) = self.focus
            && i >= list.len()
        {
            self.focus = Focus::Avatar(list.len().saturating_sub(1));
        }
        // corner buttons
        for (index, icon) in CORNER.iter().enumerate() {
            let cx = 1872.0 - 32.0 - (CORNER.len() - 1 - index) as f32 * 80.0;
            let focused = self.focus == Focus::Corner(index);
            if focused {
                let d = 64.0 * 1.06;
                ui.gfx.rect(Rect::new(cx - d / 2.0, 72.0 - d / 2.0, d, d), d / 2.0, TEXT);
            }
            ui.gfx.icon_centered(*icon, cx, 72.0, 26.0, if focused { BG } else { MUTED });
        }

        let many = list.len() > 3;
        let size = if many { 240.0 } else { 300.0 };
        let column = if many { 300.0 + 32.0 } else { 360.0 + 48.0 };
        let footer = if state.account.profiles.can_add { 72.0 + 64.0 } else { 0.0 };
        let block_h = 68.0 + 80.0 + size + 36.0 + 48.0 + footer;
        let top = (1080.0 - block_h) / 2.0;
        ui.text_centered(Font::Bold, 56.0, 960.0, top, "¿Quién se irá de aventura?", TEXT);
        if list.is_empty() {
            for index in 0..3 {
                let x = 960.0 - 1.5 * column + index as f32 * column + (column - size) / 2.0;
                ui.skeleton(Rect::new(x, top + 148.0, size, size), size / 2.0);
            }
            return;
        }
        let total = list.len() as f32 * column;
        let mut x = 960.0 - total / 2.0;
        let avatar_y = top + 148.0;
        for (index, profile) in list.iter().enumerate() {
            let cx = x + column / 2.0;
            let mut r = Rect::new(cx - size / 2.0, avatar_y, size, size);
            let focused = self.focus == Focus::Avatar(index);
            if focused {
                r = r.scaled(1.08);
                ui.gfx.rect(r.inset(-6.0), r.w, TEXT);
            }
            ui.picture(profile.avatar_url().as_deref(), r, r.w / 2.0, 1.0, AVATAR_PICTURE);
            // the name, with the pencil after it, stays centred under the picture
            let name_size = if many { 30.0 } else { 34.0 };
            let name_w = ui.gfx.measure(Font::SemiBold, name_size, &profile.name).min(column - 80.0);
            let row_w = name_w + 8.0 + 48.0;
            let name_x = cx - row_w / 2.0 + 28.0 - 28.0;
            let name_y = avatar_y + size + 36.0;
            let color = if focused { TEXT } else { MUTED };
            ui.gfx.text(Font::SemiBold, name_size, name_x, name_y + 4.0, &profile.name, color, Some(column - 80.0));
            let pencil = Look::new(self.focus == Focus::Pencil(index), false);
            let pcx = name_x + name_w + 8.0 + 24.0;
            if pencil.focused {
                ui.gfx.rect(Rect::new(pcx - 24.0, name_y - 2.0, 48.0, 48.0), 24.0, TEXT);
            }
            ui.gfx.icon_centered(Icon::Edit, pcx, name_y + 22.0, 24.0, if pencil.focused { BG } else { MUTED });
            x += column;
        }
        if state.account.profiles.can_add {
            let label = "Agregar";
            let w = ui.gfx.measure(Font::SemiBold, 23.0, label) + 60.0 + 26.0 + 12.0;
            let y = avatar_y + size + 36.0 + 48.0 + 72.0;
            ui.pill_plain(960.0 - w / 2.0, y, 64.0, label, Some(Icon::Plus), 23.0, Look::new(self.focus == Focus::Add, false));
        }
    }
}
