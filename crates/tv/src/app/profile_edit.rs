//! Editing a profile, or creating one: its avatar (picked from Crunchyroll's albums), name, username (for a new
//! one), and its three languages; then Save, or Delete for a profile that isn't the account's own.

use crunchy_core::account::{Album, Profile, avatar_url};
use serde_json::{Map, Value, json};

use crate::gfx::{Color, Font, Icon, Rect};
use crate::platform::Key;

use super::alert::{Alert, AlertAction};
use super::i18n::{Text, t};
use super::keyboard::{self, Keyboard, Outcome};
use super::languages::language_name;
use super::signin::AVATAR_PICTURE;
use super::theme::*;
use super::ui::{Look, Ui};
use super::worker::{Command, Event};
use super::{Env, Nav, Page, State};

const CONTENT_LANGUAGES: [&str; 11] =
    ["ar-SA", "de-DE", "en-US", "es-419", "es-ES", "fr-FR", "it-IT", "pt-BR", "pt-PT", "ru-RU", "hi-IN"];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Row {
    Avatar,
    Name,
    Username,
    Content,
    Audio,
    Subtitles,
    Save,
    Delete,
}

impl Row {
    fn label(self) -> &'static str {
        match self {
            Row::Avatar => t("Avatar"),
            Row::Name => t("Name"),
            Row::Username => t("Username"),
            Row::Content => t("Content language"),
            Row::Audio => t("Audio language"),
            Row::Subtitles => t("Subtitle language"),
            Row::Save => t("Save"),
            Row::Delete => t("Delete"),
        }
    }

    fn icon(self) -> Icon {
        match self {
            Row::Avatar => Icon::Photo,
            Row::Name | Row::Username => Icon::User,
            Row::Content => Icon::TextSize,
            Row::Audio => Icon::Speaker,
            Row::Subtitles => Icon::Bubble,
            Row::Save => Icon::Check,
            Row::Delete => Icon::Trash,
        }
    }
}

struct Picker {
    album: usize,
    focus: usize,
    /// the focus is on the album pills (else the avatars)
    on_albums: bool,
    scroll: f32,
}

pub struct ProfileEdit {
    original: Option<Profile>,
    profile: Profile,
    focus: usize,
    /// a row's options open, with the focus
    open: Option<(Row, usize)>,
    keyboard: Option<Keyboard>,
    picker: Option<Picker>,
    albums: Vec<Album>,
    usernames: Vec<String>,
    saving: bool,
    done: bool,
}

const ROW_H: f32 = 76.0;
const ROW_GAP: f32 = 8.0;
const LIST_X: f32 = 864.0;
const LIST_W: f32 = 1020.0;

impl ProfileEdit {
    pub fn edit(profile: Profile) -> ProfileEdit {
        ProfileEdit::with(Some(profile.clone()), profile)
    }

    pub fn create() -> ProfileEdit {
        let profile = Profile {
            content_language: "es-419".into(),
            audio_language: "ja-JP".into(),
            subtitle_language: "es-419".into(),
            ..Default::default()
        };
        ProfileEdit::with(None, profile)
    }

    fn with(original: Option<Profile>, profile: Profile) -> ProfileEdit {
        ProfileEdit {
            original,
            profile,
            focus: 0,
            open: None,
            keyboard: None,
            picker: None,
            albums: Vec::new(),
            usernames: Vec::new(),
            saving: false,
            done: false,
        }
    }

    fn rows(&self) -> Vec<Row> {
        let mut rows = vec![Row::Avatar, Row::Name, Row::Username, Row::Content, Row::Audio, Row::Subtitles, Row::Save];
        if self.original.as_ref().is_some_and(|p| !p.is_primary) {
            rows.push(Row::Delete);
        }
        rows
    }

    fn options(&self, row: Row, state: &State) -> Vec<(String, String)> {
        match row {
            Row::Username => self.usernames.iter().map(|u| (u.clone(), u.clone())).collect(),
            Row::Content => CONTENT_LANGUAGES.iter().map(|c| (c.to_string(), language_name(c))).collect(),
            Row::Audio => {
                let mut list = vec![("ja-JP".to_string(), language_name("ja-JP"))];
                if let Some((audio, _)) = &state.account.languages {
                    list.extend(audio.iter().filter(|(c, _)| c != "ja-JP").map(|(c, _)| (c.clone(), language_name(c))));
                }
                list
            }
            Row::Subtitles => {
                let mut list = vec![("off".to_string(), t("None").to_string())];
                if let Some((_, subtitles)) = &state.account.languages {
                    list.extend(subtitles.iter().map(|(c, _)| (c.clone(), language_name(c))));
                }
                list
            }
            _ => Vec::new(),
        }
    }

    fn value(&self, row: Row) -> String {
        match row {
            Row::Avatar => self.profile.avatar.clone(),
            Row::Name => self.profile.name.clone(),
            Row::Username => self.profile.username.clone(),
            Row::Content => language_name(&self.profile.content_language),
            Row::Audio => language_name(&self.profile.audio_language),
            Row::Subtitles => language_name(&self.profile.subtitle_language),
            Row::Save | Row::Delete => String::new(),
        }
    }

    fn save(&mut self, env: &mut Env) -> Nav {
        let name = self.profile.name.trim().to_string();
        let valid = (2..=50).contains(&name.chars().count()) && name.chars().all(|c| c.is_ascii_alphanumeric());
        if !valid {
            return Nav::Toast(Text::Key("Only letters and numbers, at least 2 characters."));
        }
        let p = &self.profile;
        let subtitles = if p.subtitle_language == "off" { Value::Null } else { json!(p.subtitle_language) };
        let all = [
            ("profile_name", json!(name)),
            ("avatar", json!(p.avatar)),
            ("preferred_communication_language", json!(p.content_language)),
            ("preferred_content_audio_language", json!(p.audio_language)),
            ("preferred_content_subtitle_language", subtitles),
        ];
        let mut fields = Map::new();
        match &self.original {
            Some(original) => {
                let before = [
                    json!(original.name),
                    json!(original.avatar),
                    json!(original.content_language),
                    json!(original.audio_language),
                    if original.subtitle_language == "off" { Value::Null } else { json!(original.subtitle_language) },
                ];
                for ((key, value), old) in all.into_iter().zip(before) {
                    if value != old {
                        fields.insert(key.into(), value);
                    }
                }
                if fields.is_empty() {
                    return Nav::Back;
                }
                env.worker.send(Command::UpdateProfile { id: original.id.clone(), fields: Value::Object(fields) });
            }
            None => {
                if p.avatar.is_empty() || p.username.is_empty() {
                    return Nav::Toast(Text::Key("Choose an avatar and a username."));
                }
                for (key, value) in all {
                    fields.insert(key.into(), value);
                }
                fields.insert("username".into(), json!(p.username));
                env.worker.send(Command::CreateProfile { fields: Value::Object(fields) });
            }
        }
        self.saving = true;
        Nav::Stay
    }

    fn key_picker(&mut self, key: Key) {
        let Some(picker) = self.picker.as_mut() else { return };
        let cols = 8;
        let count = self.albums.get(picker.album).map(|a| a.avatars.len()).unwrap_or(0);
        if picker.on_albums {
            match key {
                Key::Left => {
                    picker.album = picker.album.saturating_sub(1);
                    picker.focus = 0;
                }
                Key::Right => {
                    picker.album = (picker.album + 1).min(self.albums.len().saturating_sub(1));
                    picker.focus = 0;
                }
                Key::Down if count > 0 => picker.on_albums = false,
                Key::Back => self.picker = None,
                _ => {}
            }
            return;
        }
        match key {
            Key::Left if picker.focus % cols > 0 => picker.focus -= 1,
            Key::Right if picker.focus % cols < cols - 1 && picker.focus + 1 < count => picker.focus += 1,
            Key::Up => {
                if picker.focus < cols {
                    picker.on_albums = true;
                } else {
                    picker.focus -= cols;
                }
            }
            Key::Down if picker.focus + cols < count => picker.focus += cols,
            Key::Ok => {
                if let Some(avatar) = self.albums.get(picker.album).and_then(|a| a.avatars.get(picker.focus)) {
                    self.profile.avatar = avatar.clone();
                }
                self.picker = None;
            }
            Key::Back => self.picker = None,
            _ => {}
        }
    }
}

impl Page for ProfileEdit {
    fn shown(&mut self, env: &mut Env) {
        env.worker.send(Command::Avatars);
        if self.original.is_none() {
            env.worker.send(Command::Usernames);
        }
        if env.state.account.languages.is_none() {
            env.worker.send(Command::Languages);
        }
    }

    fn event(&mut self, event: &Event, _env: &mut Env) {
        match event {
            Event::Avatars(albums) => {
                self.albums = albums.clone();
                if self.profile.avatar.is_empty()
                    && let Some(first) = albums.first().and_then(|a| a.avatars.first())
                {
                    self.profile.avatar = first.clone();
                }
            }
            Event::Usernames(names) => {
                self.usernames = names.clone();
                if self.profile.username.is_empty()
                    && let Some(first) = names.first()
                {
                    self.profile.username = first.clone();
                }
            }
            Event::ProfileSaved if self.saving => self.done = true,
            Event::Error(_) => self.saving = false,
            _ => {}
        }
    }

    fn finished(&self) -> bool {
        self.done
    }

    fn key(&mut self, key: Key, env: &mut Env) -> Nav {
        if self.saving {
            return Nav::Stay;
        }
        if self.picker.is_some() {
            self.key_picker(key);
            return Nav::Stay;
        }
        if let Some(keyboard) = self.keyboard.as_mut() {
            match keyboard.handle(key) {
                Outcome::Done | Outcome::LeaveUp => {
                    self.profile.name = keyboard.text.trim().to_string();
                    self.keyboard = None;
                }
                Outcome::Changed => self.profile.name = keyboard.text.clone(),
                Outcome::None => {}
            }
            return Nav::Stay;
        }
        if let Some((row, focus)) = self.open {
            let list = self.options(row, env.state);
            match key {
                Key::Up => self.open = Some((row, focus.saturating_sub(1))),
                Key::Down => self.open = Some((row, (focus + 1).min(list.len().saturating_sub(1)))),
                Key::Back => self.open = None,
                Key::Ok => {
                    if let Some((value, _)) = list.get(focus) {
                        match row {
                            Row::Username => self.profile.username = value.clone(),
                            Row::Content => self.profile.content_language = value.clone(),
                            Row::Audio => self.profile.audio_language = value.clone(),
                            Row::Subtitles => self.profile.subtitle_language = value.clone(),
                            _ => {}
                        }
                    }
                    self.open = None;
                }
                _ => {}
            }
            return Nav::Stay;
        }
        let rows = self.rows();
        match key {
            Key::Up => self.focus = self.focus.saturating_sub(1),
            Key::Down => self.focus = (self.focus + 1).min(rows.len() - 1),
            Key::Back => return Nav::Back,
            Key::Ok => match rows[self.focus] {
                Row::Avatar => {
                    let album = self.albums.iter().position(|a| a.avatars.contains(&self.profile.avatar)).unwrap_or(0);
                    let focus = self.albums.get(album).and_then(|a| a.avatars.iter().position(|v| *v == self.profile.avatar)).unwrap_or(0);
                    self.picker = Some(Picker { album, focus, on_albums: false, scroll: 0.0 });
                }
                Row::Name => self.keyboard = Some(Keyboard::with_text(&self.profile.name)),
                Row::Username if self.original.is_some() => {}
                Row::Save => return self.save(env),
                Row::Delete => {
                    if let Some(original) = &self.original {
                        return Nav::Alert(Alert::new(
                            Text::Key("Delete Profile"),
                            Some(Text::Key("Are you sure you want to delete this profile?")),
                            AlertAction::DeleteProfile(original.id.clone()),
                        ));
                    }
                }
                row => {
                    let list = self.options(row, env.state);
                    let current = match row {
                        Row::Username => self.profile.username.clone(),
                        Row::Content => self.profile.content_language.clone(),
                        Row::Audio => self.profile.audio_language.clone(),
                        _ => self.profile.subtitle_language.clone(),
                    };
                    let focus = list.iter().position(|(v, _)| *v == current).unwrap_or(0);
                    self.open = Some((row, focus));
                }
            },
            _ => {}
        }
        Nav::Stay
    }

    fn draw(&mut self, ui: &mut Ui, state: &State, focused: bool) {
        if let Some(picker) = self.picker.as_mut() {
            draw_picker(ui, &self.albums, picker, &self.profile.avatar);
            return;
        }
        // side column: the avatar and the name
        let title = if self.original.is_some() { t("Profile") } else { t("New profile") };
        ui.text_centered(Font::Bold, 56.0, 358.0, 220.0, title, TEXT);
        let avatar = Rect::new(358.0 - 150.0, 330.0, 300.0, 300.0);
        ui.picture(avatar_url(&self.profile.avatar).as_deref(), avatar, 150.0, 1.0, AVATAR_PICTURE);
        let name = if self.profile.name.is_empty() { t("No name") } else { &self.profile.name };
        ui.text_centered(Font::SemiBold, 34.0, 358.0, 670.0, name, TEXT);

        let rows = self.rows();
        let top = (1080.0 - rows.len() as f32 * (ROW_H + ROW_GAP)) / 2.0;
        if let Some((row, focus)) = self.open {
            let list = self.options(row, state);
            let current = match row {
                Row::Username => self.profile.username.clone(),
                Row::Content => self.profile.content_language.clone(),
                Row::Audio => self.profile.audio_language.clone(),
                _ => self.profile.subtitle_language.clone(),
            };
            let visible = ((1080.0 - 200.0) / (ROW_H + ROW_GAP)) as usize;
            let first = focus.saturating_sub(visible / 2).min(list.len().saturating_sub(visible));
            ui.gfx.text(Font::Bold, 20.0, LIST_X + 36.0, 100.0, &row.label().to_uppercase(), DIM, None);
            for (index, (value, label)) in list.iter().enumerate().skip(first).take(visible) {
                let y = 140.0 + (index - first) as f32 * (ROW_H + ROW_GAP);
                let look = focused && index == focus;
                let r = Rect::new(LIST_X + 12.0, y, LIST_W - 24.0, ROW_H);
                ui.gfx.rect(if look { r.scaled(1.02) } else { r }, 14.0, if look { TEXT } else { Color(1.0, 1.0, 1.0, 0.07) });
                let ink = if look { BG } else { TEXT };
                ui.gfx.text(Font::Regular, 25.0, r.x + 26.0, r.y + 23.0, label, ink, Some(r.w - 100.0));
                if *value == current {
                    ui.gfx.icon(Icon::Check, r.x + r.w - 52.0, r.y + 25.0, 26.0, ink);
                }
            }
        } else {
            for (index, row) in rows.iter().enumerate() {
                let y = top + index as f32 * (ROW_H + ROW_GAP);
                let look = Look::new(focused && index == self.focus && self.keyboard.is_none(), false);
                let r = Rect::new(LIST_X + 12.0, y, LIST_W - 24.0, ROW_H);
                let action = matches!(row, Row::Save | Row::Delete);
                let fill = if look.focused {
                    TEXT
                } else if *row == Row::Save {
                    Color(1.0, 1.0, 1.0, 0.16)
                } else {
                    Color(1.0, 1.0, 1.0, 0.07)
                };
                ui.gfx.rect(if look.focused { r.scaled(1.02) } else { r }, 14.0, fill);
                let ink = if look.focused { BG } else if *row == Row::Delete { Color::rgb(0xff6b5e) } else { TEXT };
                let soft = if look.focused { Color(0.043, 0.043, 0.055, 0.62) } else { MUTED };
                ui.gfx.icon(row.icon(), r.x + 26.0, r.y + 24.0, 28.0, ink);
                let label = if *row == Row::Save && self.saving { t("Saving…") } else { row.label() };
                ui.gfx.text(Font::Regular, 25.0, r.x + 72.0, r.y + 23.0, label, ink, Some(r.w - 600.0));
                if !action {
                    let editable = !(*row == Row::Username && self.original.is_some());
                    let mut right = r.x + r.w - 26.0;
                    if editable {
                        ui.gfx.icon(Icon::ChevronRight, right - 22.0, r.y + 27.0, 22.0, if look.focused { soft } else { DIM });
                        right -= 36.0;
                    }
                    let value = if *row == Row::Avatar { String::new() } else { self.value(*row) };
                    let w = ui.gfx.measure(Font::Regular, 25.0, &value).min(520.0);
                    ui.gfx.text(Font::Regular, 25.0, right - w, r.y + 23.0, &value, soft, Some(520.0));
                    if *row == Row::Avatar {
                        let d = 52.0;
                        ui.picture(avatar_url(&self.profile.avatar).as_deref(), Rect::new(right - d, r.y + 12.0, d, d), d / 2.0, 1.0, 120);
                    }
                }
            }
        }
        if let Some(keyboard) = &self.keyboard {
            let w = Keyboard::width();
            keyboard.draw(ui.gfx, (1920.0 - w) / 2.0, 1080.0 - keyboard::HEIGHT - 24.0, true);
        }
    }
}

fn draw_picker(ui: &mut Ui, albums: &[Album], picker: &mut Picker, current: &str) {
    ui.gfx.text(Font::Bold, 44.0, PAD, 48.0, t("Choose an avatar"), TEXT, None);
    // album pills
    let mut x = PAD;
    for (index, album) in albums.iter().enumerate() {
        let look = Look::new(picker.on_albums && index == picker.album, index == picker.album);
        if x > 1872.0 {
            break;
        }
        x += ui.pill_plain(x, 120.0, 48.0, &album.title, None, 22.0, look) + 6.0;
    }
    let Some(album) = albums.get(picker.album) else {
        ui.text_centered(Font::SemiBold, 28.0, 960.0, 400.0, t("Loading…"), DIM);
        return;
    };
    let cols = 8;
    let size = 170.0;
    let step = 222.0;
    let top = 210.0;
    let row = picker.focus / cols;
    picker.scroll = approach(picker.scroll, row.saturating_sub(2) as f32 * step, ui.dt, 14.0);
    ui.gfx.clip(Some(Rect::new(0.0, top - 20.0, 1920.0, 1080.0 - top + 20.0)));
    for (index, avatar) in album.avatars.iter().enumerate() {
        let x = PAD + 20.0 + (index % cols) as f32 * step;
        let y = top + (index / cols) as f32 * step - picker.scroll;
        if y < top - step || y > 1080.0 {
            continue;
        }
        let mut r = Rect::new(x, y, size, size);
        let focused = !picker.on_albums && index == picker.focus;
        if focused {
            r = r.scaled(1.08);
            ui.gfx.rect(r.inset(-5.0), r.w, TEXT);
        } else if avatar == current {
            ui.gfx.rect(r.inset(-4.0), r.w, ACCENT);
        }
        let url = format!("https://static.crunchyroll.com/assets/avatar/170x170/{avatar}");
        ui.picture(Some(&url), r, r.w / 2.0, 1.0, 170);
    }
    ui.gfx.clip(None);
}
