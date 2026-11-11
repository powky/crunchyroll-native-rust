//! The top bar: the logo, a centred capsule of tabs (the search tab last, as an icon), the clock and the profile's
//! avatar, whose menu has Ajustes and Salir. Left/Right move along it, OK switches the page.

use crate::gfx::{Color, Font, Icon, Rect};
use crate::platform::Key;

use super::signin::AVATAR_PICTURE;
use super::theme::*;
use super::ui::{Look, Ui};
use super::{Nav, State};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tab {
    Home,
    Simulcast,
    Series,
    Movies,
    Music,
    MyList,
    Search,
    Settings,
}

impl Tab {
    /// The tabs with a name in the capsule, in order (search follows as an icon).
    pub const NAMED: [Tab; 6] = [Tab::Home, Tab::Simulcast, Tab::Series, Tab::Movies, Tab::Music, Tab::MyList];

    pub fn label(self) -> &'static str {
        match self {
            Tab::Home => "Inicio",
            Tab::Simulcast => "Simulcast",
            Tab::Series => "Series",
            Tab::Movies => "Películas",
            Tab::Music => "Música",
            Tab::MyList => "Mi Lista",
            Tab::Search => "Buscar",
            Tab::Settings => "Ajustes",
        }
    }

    /// The key it's saved under when hidden.
    pub fn key(self) -> &'static str {
        match self {
            Tab::Home => "home",
            Tab::Simulcast => "simulcast",
            Tab::Series => "series",
            Tab::Movies => "movies",
            Tab::Music => "musics",
            Tab::MyList => "my_list",
            Tab::Search => "search",
            Tab::Settings => "config",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Item {
    Tab(Tab),
    Avatar,
}

pub enum BarNav {
    Stay,
    /// OK on a tab: show it
    Show(Tab),
    /// focus goes down to the page
    Down,
    Back,
    Nav(Nav),
}

/// The avatar menu: icon, label, tab it opens (`None` is Salir).
const MENU: [(Icon, &str, Option<Tab>); 2] = [(Icon::Gear, "Ajustes", Some(Tab::Settings)), (Icon::Power, "Salir", None)];

pub struct TopBar {
    /// the tab shown
    pub tab: Tab,
    items: Vec<Item>,
    focus: usize,
    /// the avatar's menu, open, with its focus
    menu: Option<usize>,
}

const BAR_Y: f32 = 40.0;
const BAR_H: f32 = 64.0;
const CAPSULE_H: f32 = 60.0;
const TAB_H: f32 = 48.0;
const TAB_PAD: f32 = 22.0;
const TAB_GAP: f32 = 6.0;
const TAB_FONT: f32 = 22.0;
const AVATAR_D: f32 = 52.0;

impl TopBar {
    pub fn new(state: &State) -> TopBar {
        let mut bar = TopBar { tab: Tab::Home, items: Vec::new(), focus: 0, menu: None };
        bar.settings_changed(state);
        bar
    }

    /// Rebuilds the tabs after the hidden ones changed (the current tab stays while it's shown).
    pub fn settings_changed(&mut self, state: &State) {
        let focused = self.items.get(self.focus).copied();
        let tab = self.tab;
        self.items = Tab::NAMED
            .iter()
            .filter(|t| **t == Tab::Home || **t == tab || !state.settings.tab_hidden(**t))
            .map(|t| Item::Tab(*t))
            .collect();
        self.items.push(Item::Tab(Tab::Search));
        self.items.push(Item::Avatar);
        self.focus = focused.and_then(|item| self.items.iter().position(|i| *i == item)).unwrap_or(0);
    }

    /// Makes `tab` the current one, with the focus on it.
    pub fn select(&mut self, tab: Tab, state: &State) {
        self.tab = tab;
        self.settings_changed(state);
        self.focus = self.items.iter().position(|item| *item == Item::Tab(tab)).unwrap_or(self.items.len() - 1);
    }

    /// Focus lands on the current tab when it comes up from the page.
    pub fn enter(&mut self) {
        self.menu = None;
        if let Some(index) = self.items.iter().position(|item| *item == Item::Tab(self.tab)) {
            self.focus = index;
        } else {
            // a tab from the avatar menu: the avatar
            self.focus = self.items.len() - 1;
        }
    }

    /// The focus is on the current tab with no menu open: a page may take it.
    pub fn idle(&self) -> bool {
        self.menu.is_none() && self.items.get(self.focus) == Some(&Item::Tab(self.tab))
    }

    pub fn key(&mut self, key: Key) -> BarNav {
        if let Some(focus) = self.menu.as_mut() {
            match key {
                Key::Up => *focus = focus.saturating_sub(1),
                Key::Down => *focus = (*focus + 1).min(MENU.len() - 1),
                Key::Back => self.menu = None,
                Key::Ok => {
                    let (_, _, tab) = MENU[*focus];
                    self.menu = None;
                    return match tab {
                        Some(tab) => BarNav::Show(tab),
                        None => BarNav::Nav(Nav::Alert(super::alert::Alert::new(
                            "¿Salir de la aplicación?",
                            None,
                            super::alert::AlertAction::Quit,
                        ))),
                    };
                }
                _ => {}
            }
            return BarNav::Stay;
        }
        match key {
            Key::Left => self.focus = self.focus.saturating_sub(1),
            Key::Right => self.focus = (self.focus + 1).min(self.items.len() - 1),
            Key::Down => return BarNav::Down,
            Key::Ok => match self.items[self.focus] {
                Item::Tab(tab) if tab == self.tab => return BarNav::Down,
                Item::Tab(tab) => return BarNav::Show(tab),
                Item::Avatar => self.menu = Some(0),
            },
            Key::Back => return BarNav::Back,
            _ => {}
        }
        BarNav::Stay
    }

    pub fn draw(&mut self, ui: &mut Ui, state: &State, focused: bool) {
        ui.gfx.icon(Icon::Logo, 48.0, BAR_Y + (BAR_H - 38.0) / 2.0, 38.0, ACCENT);

        // the capsule's width first, to centre it
        let widths: Vec<f32> = self
            .items
            .iter()
            .map(|item| match item {
                Item::Tab(Tab::Search) => TAB_H + TAB_GAP,
                Item::Tab(tab) => ui.gfx.measure(Font::SemiBold, TAB_FONT, tab.label()) + 2.0 * TAB_PAD + TAB_GAP,
                Item::Avatar => 0.0,
            })
            .collect();
        let inner: f32 = widths.iter().sum();
        let capsule = Rect::new(960.0 - (inner + 6.0) / 2.0, BAR_Y + (BAR_H - CAPSULE_H) / 2.0, inner + 6.0, CAPSULE_H);
        ui.gfx.rect(capsule, CAPSULE_H / 2.0, Color(0.141, 0.141, 0.165, 0.62));
        let mut x = capsule.x + 6.0;
        for (index, item) in self.items.iter().enumerate() {
            let has_focus = focused && index == self.focus && self.menu.is_none();
            if let Item::Tab(tab) = item {
                let look = Look::new(has_focus, *tab == self.tab);
                let y = capsule.y + 6.0;
                if *tab == Tab::Search {
                    let (fill, ink) = tab_colors(look);
                    if let Some(fill) = fill {
                        ui.gfx.rect(Rect::new(x, y, TAB_H, TAB_H), TAB_H / 2.0, fill);
                    }
                    ui.gfx.icon_centered(Icon::Search, x + TAB_H / 2.0, y + TAB_H / 2.0, 22.0, ink);
                } else {
                    let w = widths[index] - TAB_GAP;
                    let (fill, ink) = tab_colors(look);
                    if let Some(fill) = fill {
                        ui.gfx.rect(Rect::new(x, y, w, TAB_H), TAB_H / 2.0, fill);
                    }
                    ui.gfx.text(Font::SemiBold, TAB_FONT, x + TAB_PAD, y + 10.0, tab.label(), ink, None);
                }
            }
            x += widths[index];
        }

        // clock and avatar on the right
        let mut avatar = Rect::new(1872.0 - AVATAR_D, BAR_Y + (BAR_H - AVATAR_D) / 2.0, AVATAR_D, AVATAR_D);
        let avatar_focus = focused && self.items[self.focus] == Item::Avatar;
        let clock_right = avatar.x - 22.0;
        if avatar_focus {
            avatar = avatar.scaled(1.15);
            ui.gfx.rect(avatar.inset(-3.0), AVATAR_D, TEXT);
        }
        let url = state.account.profile().and_then(|p| p.avatar_url());
        ui.picture(url.as_deref(), avatar, avatar.w / 2.0, 1.0, AVATAR_PICTURE / 2);
        let (clock, suffix) = clock_text(state.settings.clock_24h);
        let suffix_w = if suffix.is_empty() { 0.0 } else { ui.gfx.measure(Font::SemiBold, 20.0, suffix) + 6.0 };
        let w = ui.gfx.measure(Font::Regular, 28.0, &clock);
        let x = clock_right - suffix_w - w;
        ui.gfx.text(Font::Regular, 28.0, x, BAR_Y + 15.0, &clock, TEXT, None);
        if !suffix.is_empty() {
            ui.gfx.text(Font::SemiBold, 20.0, x + w + 6.0, BAR_Y + 22.0, suffix, TEXT, None);
        }

        if let Some(menu_focus) = self.menu {
            let panel = Rect::new(1872.0 - 324.0, BAR_Y + BAR_H + 12.0, 324.0, 24.0 + 8.0 + MENU.len() as f32 * 68.0 - 4.0);
            ui.gfx.rect(panel.inset(-1.0), 25.0, Color(1.0, 1.0, 1.0, 0.10));
            ui.gfx.rect(panel, 24.0, Color(0.141, 0.141, 0.165, 0.97));
            for (index, (icon, label, _)) in MENU.iter().enumerate() {
                let r = Rect::new(panel.x + 16.0, panel.y + 16.0 + index as f32 * 68.0, panel.w - 32.0, 64.0);
                let look = index == menu_focus;
                let r = if look { r.scaled(1.03) } else { r };
                if look {
                    ui.gfx.rect(r, 32.0, TEXT);
                }
                let ink = if look { BG } else { MUTED };
                ui.gfx.icon(*icon, r.x + 26.0, r.y + 19.0, 26.0, ink);
                ui.gfx.text(Font::SemiBold, 23.0, r.x + 66.0, r.y + 17.0, label, ink, None);
            }
        }
    }
}

fn tab_colors(look: Look) -> (Option<Color>, Color) {
    if look.focused {
        (Some(TEXT), BG)
    } else if look.selected {
        (Some(VEIL_STRONG), TEXT)
    } else {
        (None, MUTED)
    }
}

/// The time now on the TV's clock and time zone, and the 12-hour suffix ("p. m.") when there is one.
pub fn clock_text(h24: bool) -> (String, &'static str) {
    let (hour, minute) = local_time();
    if h24 {
        (format!("{hour:02}:{minute:02}"), "")
    } else {
        let h = match hour % 12 {
            0 => 12,
            h => h,
        };
        (format!("{h}:{minute:02}"), if hour < 12 { "a. m." } else { "p. m." })
    }
}

fn local_time() -> (i32, i32) {
    let tm = local(None);
    (tm.3, tm.4)
}

/// Today: (year, month 1..=12, day).
pub fn local_date() -> (i32, u32, u32) {
    let tm = local(None);
    (tm.0, tm.1, tm.2)
}

/// "04/10/25, 7:30 p. m" for an RFC 3339 time, in the TV's time zone (always 12-hour, like the web app's simulcast).
pub fn short_datetime(iso: &str) -> Option<String> {
    let secs = parse_iso(iso)?;
    let (year, month, day, hour, minute) = local(Some(secs));
    let h = match hour % 12 {
        0 => 12,
        h => h,
    };
    let suffix = if hour < 12 { "a. m" } else { "p. m" };
    Some(format!("{day:02}/{month:02}/{:02}, {h}:{minute:02} {suffix}", year % 100))
}

/// Seconds since 1970 of "2026-10-19T13:00:00.000Z" (UTC).
pub fn parse_iso(iso: &str) -> Option<i64> {
    let n = |range: std::ops::Range<usize>| iso.get(range)?.parse::<i64>().ok();
    let (y, m, d) = (n(0..4)?, n(5..7)?, n(8..10)?);
    let (hh, mm, ss) = (n(11..13).unwrap_or(0), n(14..16).unwrap_or(0), n(17..19).unwrap_or(0));
    // days from the civil date (Howard Hinnant's algorithm)
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + hh * 3600 + mm * 60 + ss)
}

/// (year, month, day, hour, minute) of a time (now when `None`) in the TV's time zone.
fn local(at: Option<i64>) -> (i32, u32, u32, i32, i32) {
    use std::ffi::{c_char, c_int, c_long};
    #[repr(C)]
    struct Tm {
        sec: c_int,
        min: c_int,
        hour: c_int,
        mday: c_int,
        mon: c_int,
        year: c_int,
        wday: c_int,
        yday: c_int,
        isdst: c_int,
        gmtoff: c_long,
        zone: *const c_char,
    }
    unsafe extern "C" {
        fn time(out: *mut c_long) -> c_long;
        fn localtime_r(now: *const c_long, out: *mut Tm) -> *mut Tm;
    }
    unsafe {
        let now: c_long = match at {
            Some(secs) => secs as c_long,
            None => time(std::ptr::null_mut()),
        };
        let mut tm: Tm = std::mem::zeroed();
        if localtime_r(&now, &mut tm).is_null() {
            return (1970, 1, 1, 0, 0);
        }
        (tm.year + 1900, (tm.mon + 1) as u32, tm.mday as u32, tm.hour, tm.min)
    }
}
