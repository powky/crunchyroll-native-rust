//! The app's settings (saved in `settings.json` in the data folder) and the Settings page: a side column with the
//! gear tile, and a list of sections and rows; a row with options opens them in place of the list. The profile's
//! languages are saved on Crunchyroll instead.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::gfx::{Color, Font, Icon, Rect};
use crate::platform::Key;

use super::alert::{Alert, AlertAction};
use super::languages::language_name;
use super::theme::*;
use super::topbar::Tab;
use super::ui::Ui;
use super::worker::{Command, Event};
use super::{Env, Nav, Page, State};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    #[serde(skip)]
    path: PathBuf,
    /// adaptive, 2160p, 1080p, 720p, 480p, 360p, 240p (for the player)
    pub video_quality: String,
    /// hardsub or softsub (for the player)
    pub subtitles: String,
    /// seek thumbnails (for the player)
    pub previews: bool,
    /// "continue watching" first on the home, else second
    pub continue_first: bool,
    /// the lite interface drops Mi Lista's poster and genres
    pub lite: bool,
    pub clock_24h: bool,
    /// tabs hidden from the top bar, by key
    pub hidden_tabs: Vec<String>,
    /// the profile chosen last, focused first in the picker
    pub last_profile: Option<String>,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            path: PathBuf::new(),
            video_quality: "adaptive".into(),
            subtitles: "hardsub".into(),
            previews: true,
            continue_first: false,
            lite: false,
            clock_24h: true,
            hidden_tabs: Vec::new(),
            last_profile: None,
        }
    }
}

impl Settings {
    pub fn load(dir: &Path) -> Settings {
        let path = dir.join("settings.json");
        let mut settings: Settings =
            std::fs::read(&path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
        settings.path = path;
        settings
    }

    pub fn save(&self) {
        if let Ok(json) = serde_json::to_vec_pretty(self) {
            let tmp = self.path.with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(tmp, &self.path);
            }
        }
    }

    pub fn tab_hidden(&self, tab: Tab) -> bool {
        self.hidden_tabs.iter().any(|key| key == tab.key())
    }
}

/// What a row does.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Row {
    Quality,
    Subtitles,
    Previews,
    FirstRow,
    Interface,
    Clock,
    TabShown(Tab),
    ContentLanguage,
    AudioLanguage,
    SubtitleLanguage,
    SignOut,
}

impl Row {
    fn label(self) -> &'static str {
        match self {
            Row::Quality => "Calidad de vídeo",
            Row::Subtitles => "Subtítulos",
            Row::Previews => "Vistas previas",
            Row::FirstRow => "Primera fila",
            Row::Interface => "Interfaz",
            Row::Clock => "Reloj",
            Row::TabShown(tab) => tab.label(),
            Row::ContentLanguage => "Títulos y sinopsis",
            Row::AudioLanguage => "Audio preferido",
            Row::SubtitleLanguage => "Subtítulos preferidos",
            Row::SignOut => "Cerrar Sesión",
        }
    }

    fn icon(self) -> Icon {
        match self {
            Row::Quality => Icon::Tv,
            Row::Subtitles | Row::SubtitleLanguage => Icon::Bubble,
            Row::Previews => Icon::Photo,
            Row::FirstRow => Icon::Rows,
            Row::Interface => Icon::Sparkles,
            Row::Clock => Icon::Clock,
            Row::TabShown(Tab::Simulcast) => Icon::Tv,
            Row::TabShown(Tab::Music) => Icon::Note,
            Row::TabShown(Tab::MyList) => Icon::Heart,
            Row::TabShown(_) => Icon::Film,
            Row::ContentLanguage => Icon::TextSize,
            Row::AudioLanguage => Icon::Speaker,
            Row::SignOut => Icon::Logout,
        }
    }
}

const CONTENT_LANGUAGES: [&str; 11] =
    ["ar-SA", "de-DE", "en-US", "es-419", "es-ES", "fr-FR", "it-IT", "pt-BR", "pt-PT", "ru-RU", "hi-IN"];

fn sections() -> Vec<(&'static str, Vec<Row>)> {
    vec![
        ("REPRODUCCIÓN", vec![Row::Quality, Row::Subtitles, Row::Previews]),
        ("INICIO", vec![Row::FirstRow, Row::Interface, Row::Clock]),
        (
            "BARRA SUPERIOR",
            vec![Tab::Simulcast, Tab::Series, Tab::Movies, Tab::Music, Tab::MyList].into_iter().map(Row::TabShown).collect(),
        ),
        ("IDIOMA", vec![Row::ContentLanguage, Row::AudioLanguage, Row::SubtitleLanguage]),
        ("CUENTA", vec![Row::SignOut]),
    ]
}

/// The options of a row: (stored value, label).
fn options(row: Row, state: &State) -> Vec<(String, String)> {
    let pairs = |list: &[(&str, &str)]| list.iter().map(|(v, l)| (v.to_string(), l.to_string())).collect();
    match row {
        Row::Quality => pairs(&[
            ("adaptive", "Automático"),
            ("2160p", "4K (2160p)"),
            ("1080p", "Full HD (1080p)"),
            ("720p", "HD (720p)"),
            ("480p", "SD (480p)"),
            ("360p", "360p"),
            ("240p", "240p"),
        ]),
        Row::Subtitles => pairs(&[("hardsub", "Incrustados en el vídeo (Hardsub)"), ("softsub", "Dibujados por la app (Softsub)")]),
        Row::Previews => pairs(&[("yes", "Sí"), ("no", "No")]),
        Row::FirstRow => pairs(&[("suggestions", "Sugerencias"), ("continue", "Seguir viendo")]),
        Row::Interface => pairs(&[("full", "Completo"), ("lite", "Ligero")]),
        Row::Clock => pairs(&[("24", "24 horas"), ("12", "12 horas (a. m. / p. m.)")]),
        Row::TabShown(_) => pairs(&[("show", "Mostrar"), ("hide", "Ocultar")]),
        Row::ContentLanguage => CONTENT_LANGUAGES.iter().map(|code| (code.to_string(), language_name(code))).collect(),
        Row::AudioLanguage => {
            let mut list = vec![("ja-JP".to_string(), language_name("ja-JP"))];
            if let Some((audio, _)) = &state.account.languages {
                list.extend(audio.iter().filter(|(code, _)| code != "ja-JP").map(|(code, _)| (code.clone(), language_name(code))));
            }
            list
        }
        Row::SubtitleLanguage => {
            let mut list = vec![("off".to_string(), "Desactivar".to_string())];
            if let Some((_, subtitles)) = &state.account.languages {
                list.extend(subtitles.iter().map(|(code, _)| (code.clone(), language_name(code))));
            }
            list
        }
        Row::SignOut => Vec::new(),
    }
}

/// The row's current value, as stored.
fn value(row: Row, state: &State) -> Option<String> {
    let s = &state.settings;
    let profile = state.account.profile();
    Some(match row {
        Row::Quality => s.video_quality.clone(),
        Row::Subtitles => s.subtitles.clone(),
        Row::Previews => if s.previews { "yes" } else { "no" }.into(),
        Row::FirstRow => if s.continue_first { "continue" } else { "suggestions" }.into(),
        Row::Interface => if s.lite { "lite" } else { "full" }.into(),
        Row::Clock => if s.clock_24h { "24" } else { "12" }.into(),
        Row::TabShown(tab) => if s.tab_hidden(tab) { "hide" } else { "show" }.into(),
        Row::ContentLanguage => profile?.content_language.clone(),
        Row::AudioLanguage => profile?.audio_language.clone(),
        Row::SubtitleLanguage => profile?.subtitle_language.clone(),
        Row::SignOut => return None,
    })
}

fn value_label(row: Row, state: &State) -> Option<String> {
    let value = value(row, state)?;
    match row {
        Row::ContentLanguage | Row::AudioLanguage => Some(language_name(&value)),
        Row::SubtitleLanguage => Some(if value == "off" { "Desactivar".into() } else { language_name(&value) }),
        _ => options(row, state).into_iter().find(|(v, _)| *v == value).map(|(_, label)| label),
    }
}

pub struct SettingsPage {
    /// shown on its own (from the profile picker), not under the top bar
    standalone: bool,
    focus: usize,
    scroll: f32,
    /// the row whose options are open, the option focused, and the list's scroll
    open: Option<(Row, usize, f32)>,
}

const ROW_H: f32 = 76.0;
const ROW_GAP: f32 = 8.0;
const SECTION_GAP: f32 = 26.0;
const SECTION_TITLE: f32 = 38.0;
const LIST_X: f32 = 864.0;
const LIST_W: f32 = 1020.0;

impl SettingsPage {
    pub fn new(standalone: bool) -> SettingsPage {
        SettingsPage { standalone, focus: 0, scroll: 0.0, open: None }
    }

    fn rows() -> Vec<Row> {
        sections().into_iter().flat_map(|(_, rows)| rows).collect()
    }

    fn apply(&mut self, row: Row, chosen: &str, env: &mut Env) -> Nav {
        let s = &mut env.state.settings;
        match row {
            Row::Quality => s.video_quality = chosen.into(),
            Row::Subtitles => s.subtitles = chosen.into(),
            Row::Previews => s.previews = chosen == "yes",
            Row::FirstRow => s.continue_first = chosen == "continue",
            Row::Interface => s.lite = chosen == "lite",
            Row::Clock => s.clock_24h = chosen == "24",
            Row::TabShown(tab) => {
                s.hidden_tabs.retain(|key| key != tab.key());
                if chosen == "hide" {
                    s.hidden_tabs.push(tab.key().to_string());
                }
            }
            Row::ContentLanguage | Row::AudioLanguage | Row::SubtitleLanguage => {
                let Some(profile) = env.state.account.profile().cloned() else { return Nav::Stay };
                let field = match row {
                    Row::ContentLanguage => "preferred_communication_language",
                    Row::AudioLanguage => "preferred_content_audio_language",
                    _ => "preferred_content_subtitle_language",
                };
                let value = if chosen == "off" { serde_json::Value::Null } else { json!(chosen) };
                env.worker.send(Command::UpdateProfile { id: profile.id.clone(), fields: json!({ field: value }) });
                // shown at once; the profile list that comes back confirms it
                if let Some(p) = env.state.account.profiles.list.iter_mut().find(|p| p.id == profile.id) {
                    match row {
                        Row::ContentLanguage => p.content_language = chosen.into(),
                        Row::AudioLanguage => p.audio_language = chosen.into(),
                        _ => p.subtitle_language = chosen.into(),
                    }
                }
                return Nav::Stay;
            }
            Row::SignOut => return Nav::Stay,
        }
        Nav::SettingsChanged
    }
}

impl Page for SettingsPage {
    fn shown(&mut self, env: &mut Env) {
        if env.state.account.languages.is_none() {
            env.worker.send(Command::Languages);
        }
    }

    fn event(&mut self, _event: &Event, _env: &mut Env) {}

    fn key(&mut self, key: Key, env: &mut Env) -> Nav {
        if let Some((row, focus, _)) = self.open.as_mut() {
            let count = options(*row, env.state).len();
            match key {
                Key::Up => *focus = focus.saturating_sub(1),
                Key::Down => *focus = (*focus + 1).min(count.saturating_sub(1)),
                Key::Back => self.open = None,
                Key::Ok => {
                    let row = *row;
                    let chosen = options(row, env.state).get(*focus).map(|(v, _)| v.clone());
                    self.open = None;
                    if let Some(chosen) = chosen {
                        return self.apply(row, &chosen, env);
                    }
                }
                _ => {}
            }
            return Nav::Stay;
        }
        let rows = Self::rows();
        match key {
            Key::Up => {
                if self.focus == 0 {
                    return if self.standalone { Nav::Stay } else { Nav::Bar };
                }
                self.focus -= 1;
            }
            Key::Down => self.focus = (self.focus + 1).min(rows.len() - 1),
            Key::Back => return Nav::Back,
            Key::Ok => match rows[self.focus] {
                Row::SignOut => {
                    return Nav::Alert(Alert::new(
                        "Cerrar Sesión",
                        Some("¿Estás seguro de que quieres cerrar sesión?"),
                        AlertAction::SignOut,
                    ));
                }
                row => {
                    let list = options(row, env.state);
                    let current = value(row, env.state);
                    let focus = list.iter().position(|(v, _)| Some(v) == current.as_ref()).unwrap_or(0);
                    self.open = Some((row, focus, 0.0));
                }
            },
            _ => {}
        }
        Nav::Stay
    }

    fn draw(&mut self, ui: &mut Ui, state: &State, focused: bool) {
        let (top, bottom) = if self.standalone { (0.0, 1080.0) } else { (136.0, 1056.0) };
        // side column, centred in the page's height
        let column_h = 300.0 + 40.0 + 68.0 + 10.0 + 28.0;
        let y0 = top + (bottom - top - column_h) / 2.0;
        let tile = Rect::new(48.0 + (620.0 - 300.0) / 2.0, y0, 300.0, 300.0);
        ui.gfx.gradient_rounded(tile, 66.0, Color::rgb(0x3a3a42), Color::rgb(0x1c1c21), true);
        ui.gfx.icon_centered(Icon::Gear, tile.x + 150.0, tile.y + 150.0, 190.0, Color::rgb(0xd1d1d6));
        ui.text_centered(Font::Bold, 56.0, 358.0, y0 + 340.0, "Ajustes", TEXT);
        let version = concat!("Crunchy ", env!("CARGO_PKG_VERSION"));
        ui.text_centered(Font::Regular, 22.0, 358.0, y0 + 418.0, version, DIM);

        let list_top = if self.standalone { 110.0 } else { 146.0 };
        let list_bottom = if self.standalone { 1020.0 } else { 1056.0 };
        ui.gfx.clip(Some(Rect::new(LIST_X - 20.0, list_top, LIST_W + 40.0, list_bottom - list_top)));
        if let Some((row, focus, scroll)) = self.open.as_mut() {
            let list = options(*row, state);
            let current = value(*row, state);
            let target = (*focus as f32 * (ROW_H + ROW_GAP) + SECTION_TITLE - (list_bottom - list_top) / 2.0).max(0.0);
            *scroll = approach(*scroll, target, ui.dt, 14.0);
            let mut y = list_top - *scroll;
            section_title(ui, &row.label().to_uppercase(), y);
            y += SECTION_TITLE;
            for (index, (v, label)) in list.iter().enumerate() {
                let look = focused && index == *focus;
                let r = row_rect(y, look);
                draw_row_box(ui, r, look);
                let ink = if look { BG } else { TEXT };
                ui.gfx.text(Font::Regular, 25.0, r.x + 26.0, r.y + 23.0, label, ink, Some(r.w - 100.0));
                if Some(v) == current.as_ref() {
                    ui.gfx.icon(Icon::Check, r.x + r.w - 26.0 - 26.0, r.y + 25.0, 26.0, ink);
                }
                y += ROW_H + ROW_GAP;
            }
        } else {
            let mut y = list_top;
            let mut index = 0;
            let mut focus_y = 0.0;
            // where the focused row sits, to scroll to it
            for (_, rows) in sections() {
                y += SECTION_TITLE;
                for _ in rows {
                    if index == self.focus {
                        focus_y = y - list_top;
                    }
                    y += ROW_H + ROW_GAP;
                    index += 1;
                }
                y += SECTION_GAP;
            }
            let target = (focus_y - (list_bottom - list_top) / 2.0 + ROW_H).clamp(0.0, (y - list_bottom).max(0.0));
            self.scroll = approach(self.scroll, target, ui.dt, 14.0);
            let mut y = list_top - self.scroll;
            let mut index = 0;
            for (title, rows) in sections() {
                section_title(ui, title, y);
                y += SECTION_TITLE;
                for row in rows {
                    let look = focused && index == self.focus;
                    let r = row_rect(y, look);
                    draw_row_box(ui, r, look);
                    let ink = if look { BG } else { TEXT };
                    let soft = if look { Color(0.043, 0.043, 0.055, 0.62) } else { MUTED };
                    ui.gfx.icon(row.icon(), r.x + 26.0, r.y + 24.0, 28.0, ink);
                    ui.gfx.text(Font::Regular, 25.0, r.x + 72.0, r.y + 23.0, row.label(), ink, Some(r.w - 600.0));
                    let has_options = row != Row::SignOut;
                    let mut right = r.x + r.w - 26.0;
                    if has_options {
                        ui.gfx.icon(Icon::ChevronRight, right - 22.0, r.y + 27.0, 22.0, if look { soft } else { DIM });
                        right -= 22.0 + 14.0;
                    }
                    let shown = value_label(row, state);
                    match shown {
                        Some(text) => {
                            let w = ui.gfx.measure(Font::Regular, 25.0, &text).min(520.0);
                            ui.gfx.text(Font::Regular, 25.0, right - w, r.y + 23.0, &text, soft, Some(520.0));
                        }
                        None if has_options => ui.skeleton(Rect::new(right - 140.0, r.y + 27.0, 140.0, 22.0), 6.0),
                        None => {}
                    }
                    y += ROW_H + ROW_GAP;
                    index += 1;
                }
                y += SECTION_GAP;
            }
        }
        ui.gfx.clip(None);
    }
}

fn section_title(ui: &mut Ui, title: &str, y: f32) {
    let mut x = LIST_X + 12.0 + 24.0;
    for ch in title.chars() {
        x += ui.gfx.text(Font::Bold, 20.0, x, y, &ch.to_string(), DIM, None) + 1.2;
    }
}

fn row_rect(y: f32, focused: bool) -> Rect {
    let r = Rect::new(LIST_X + 12.0, y, LIST_W - 24.0, ROW_H);
    if focused { r.scaled(1.02) } else { r }
}

fn draw_row_box(ui: &mut Ui, r: Rect, focused: bool) {
    ui.gfx.rect(r, 14.0, if focused { TEXT } else { Color(1.0, 1.0, 1.0, 0.07) });
}
