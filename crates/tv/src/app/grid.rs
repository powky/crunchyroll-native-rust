//! The grid tabs: Buscar, Series and Películas (a search field, Orden, Presentación, genre pills, then the titles),
//! Simulcast (a season to step through) and Mi Lista (a banner for the focused show over the grid). The grid holds
//! four wide cards a row and asks for pages of 25 as they come into view.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};

use crunchy_core::content::{Audio, Browse, Named, Sort};
use crunchy_core::model::{Card, Kind};

use crate::gfx::{Color, Font, Icon, Rect};
use crate::platform::Key;

use super::controls::{DropNav, Dropdown, FieldNav, PillRow, SearchField};
use super::theme::*;
use super::topbar::{Tab, local_date, parse_iso, short_datetime};
use super::ui::{CARD_PICTURE, HeroSize, HeroView, Look, Ui};
use super::worker::{Command, Event, Query};
use super::{Env, Nav, Page, State};

const PAGE: usize = 25;
const COLS: usize = 4;
const CELL_W: f32 = 468.0;
const CELL_H: f32 = 340.0;
const ART_W: f32 = 420.0;
const ART_H: f32 = 236.0;
const LIFT: f32 = 24.0;

static NEXT_KEY: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Subtitle {
    /// "2019 · 3 temporadas", else the synopsis
    Facts,
    /// when it was last updated ("04/10/25, 7:30 p. m")
    Date,
}

pub enum GridNav {
    Stay,
    /// Up from the first row
    Up,
    Open(Card),
}

/// A grid of wide cards, filled a page at a time.
pub struct CardGrid {
    key: u64,
    pub query: Query,
    slots: Vec<Option<Card>>,
    total: Option<usize>,
    asked: HashSet<usize>,
    pub focus: usize,
    scroll: f32,
    subtitle: Subtitle,
    /// load every page at once and sort by date, newest first (simulcast)
    by_date: bool,
    failed: bool,
    /// the first page arrived since the last reset
    pub arrived: bool,
}

impl CardGrid {
    pub fn new(query: Query, subtitle: Subtitle, by_date: bool) -> CardGrid {
        CardGrid {
            key: NEXT_KEY.fetch_add(1, Ordering::Relaxed),
            query,
            slots: Vec::new(),
            total: None,
            asked: HashSet::new(),
            focus: 0,
            scroll: 0.0,
            subtitle,
            by_date,
            failed: false,
            arrived: false,
        }
    }

    /// Starts over with another query.
    pub fn reset(&mut self, query: Query) {
        *self = CardGrid::new(query, self.subtitle, self.by_date);
    }

    fn ask(&mut self, worker: &super::worker::Worker, page: usize) {
        if self.asked.insert(page) {
            worker.send(Command::Grid { key: self.key, query: self.query.clone(), start: page * PAGE, count: PAGE });
        }
    }

    pub fn event(&mut self, event: &Event, worker: &super::worker::Worker) {
        match event {
            Event::Grid { key, start, page } if *key == self.key => {
                self.arrived = true;
                let total = if matches!(self.query, Query::MyList) {
                    // shows of the watchlist: duplicates fold, so the count is what came
                    self.slots.iter().flatten().count() + page.cards.len()
                } else {
                    page.total.max(start + page.cards.len())
                };
                if matches!(self.query, Query::MyList) {
                    let mut cards: Vec<Card> = self.slots.drain(..).flatten().collect();
                    for card in &page.cards {
                        if !cards.iter().any(|c| c.id == card.id) {
                            cards.push(card.clone());
                        }
                    }
                    self.slots = cards.into_iter().map(Some).collect();
                    self.total = Some(self.slots.len());
                    // the next page of the watchlist, if there's more
                    if page.total > start + PAGE {
                        self.ask(worker, start / PAGE + 1);
                    }
                    return;
                }
                self.total = Some(total);
                if self.slots.len() < total {
                    self.slots.resize(total, None);
                }
                for (offset, card) in page.cards.iter().enumerate() {
                    if let Some(slot) = self.slots.get_mut(start + offset) {
                        *slot = Some(card.clone());
                    }
                }
                if self.by_date {
                    for page in 0..total.div_ceil(PAGE) {
                        self.ask(worker, page);
                    }
                    let mut cards: Vec<Card> = self.slots.iter().flatten().cloned().collect();
                    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
                    // newest first; dates still to come after the ones already out
                    cards.sort_by_key(|card| {
                        match card.date.as_deref().and_then(parse_iso) {
                            Some(at) if at <= now => (0, -at),
                            Some(at) => (1, at),
                            None => (2, 0),
                        }
                    });
                    let loaded = cards.len();
                    self.slots = cards.into_iter().map(Some).chain(std::iter::repeat_n(None, total.saturating_sub(loaded))).collect();
                }
            }
            Event::GridFailed(key) if *key == self.key => {
                self.failed = true;
                self.total.get_or_insert(0);
            }
            _ => {}
        }
    }

    pub fn len(&self) -> usize {
        self.total.unwrap_or(0)
    }

    pub fn focused(&self) -> Option<&Card> {
        self.slots.get(self.focus)?.as_ref()
    }

    pub fn key(&mut self, key: Key) -> GridNav {
        let len = self.len();
        if len == 0 {
            return if key == Key::Up { GridNav::Up } else { GridNav::Stay };
        }
        match key {
            Key::Left if self.focus % COLS > 0 => self.focus -= 1,
            Key::Right if self.focus % COLS < COLS - 1 && self.focus + 1 < len => self.focus += 1,
            Key::Up => {
                if self.focus < COLS {
                    return GridNav::Up;
                }
                self.focus -= COLS;
            }
            Key::Down => {
                if self.focus + COLS < len {
                    self.focus += COLS;
                } else if self.focus / COLS < (len - 1) / COLS {
                    self.focus = len - 1;
                }
            }
            Key::Ok => {
                if let Some(card) = self.focused() {
                    return GridNav::Open(card.clone());
                }
            }
            _ => {}
        }
        GridNav::Stay
    }

    /// Draws the grid between `top` and `bottom`, with soft edges when `fades`.
    pub fn draw(&mut self, ui: &mut Ui, top: f32, bottom: f32, focused: bool, fades: bool) {
        if self.total.is_none() {
            self.ask(ui.worker, 0);
            for row in 0..2 {
                for col in 0..COLS {
                    bone(ui, 48.0 + col as f32 * CELL_W, top + LIFT + row as f32 * CELL_H);
                }
            }
            return;
        }
        let len = self.len();
        if len == 0 {
            let text = if self.failed { "No se pudo cargar" } else { "Vacío" };
            ui.text_centered(Font::SemiBold, 28.0, 960.0, top + 120.0, text, DIM);
            return;
        }
        self.focus = self.focus.min(len - 1);
        let height = bottom - top;
        let visible_rows = ((height - LIFT) / CELL_H).floor().max(1.0) as usize;
        let focus_row = self.focus / COLS;
        let target = focus_row.saturating_sub(visible_rows.saturating_sub(1)) as f32 * CELL_H;
        self.scroll = approach(self.scroll, target, ui.dt, 14.0);
        let first_row = (self.scroll / CELL_H).floor() as usize;
        ui.gfx.clip(Some(Rect::new(0.0, top, 1920.0, height)));
        for row in first_row..=(first_row + visible_rows + 1) {
            for col in 0..COLS {
                let index = row * COLS + col;
                if index >= len {
                    break;
                }
                let x = 48.0 + col as f32 * CELL_W;
                let y = top + LIFT + row as f32 * CELL_H - self.scroll;
                match self.slots.get(index).and_then(|slot| slot.as_ref()) {
                    Some(card) => {
                        let subtitle = match self.subtitle {
                            Subtitle::Date => card.date.as_deref().and_then(short_datetime).unwrap_or_else(|| card_subtitle(card)),
                            Subtitle::Facts => {
                                let facts = card_subtitle(card);
                                if facts.is_empty() { card.description.replace('\n', " ") } else { facts }
                            }
                        };
                        wide_card(ui, card, Rect::new(x, y, ART_W, ART_H), focused && index == self.focus, &subtitle);
                    }
                    None => {
                        if self.by_date {
                            // all pages are on their way
                        } else {
                            let page = index / PAGE;
                            self.ask(ui.worker, page);
                        }
                        bone(ui, x, y);
                    }
                }
            }
        }
        // the next rows' pictures
        for card in self.slots.iter().skip((first_row + visible_rows + 1) * COLS).take(COLS).flatten() {
            if let Some(url) = &card.image {
                ui.images.prefetch(ui.worker, url, CARD_PICTURE);
            }
        }
        ui.gfx.clip(None);
        if fades {
            ui.gfx.gradient(Rect::new(0.0, top, 1920.0, 32.0), BG, BG.alpha(0.0), true);
            ui.gfx.gradient(Rect::new(0.0, bottom - 32.0, 1920.0, 32.0), BG.alpha(0.0), BG, true);
        }
    }
}

fn bone(ui: &mut Ui, x: f32, y: f32) {
    ui.skeleton(Rect::new(x, y, ART_W, ART_H), RADIUS);
    ui.skeleton(Rect::new(x, y + ART_H + 22.0, ART_W * 0.72, 20.0), 6.0);
    ui.skeleton(Rect::new(x, y + ART_H + 54.0, ART_W * 0.46, 16.0), 6.0);
}

/// A grid card: art with no overlays, the title and one line under it.
fn wide_card(ui: &mut Ui, card: &Card, r: Rect, focused: bool, subtitle: &str) {
    let art = if focused { r.scaled(1.08) } else { r };
    if focused {
        ui.gfx.rect(art.inset(-4.0), RADIUS + 4.0, TEXT);
    }
    ui.picture(card.image.as_deref(), art, RADIUS, 1.0, CARD_PICTURE);
    let text_y = r.y + r.h + 18.0;
    let title = card.title.replace('\n', " ");
    let color = if focused { TEXT } else { Color(0.96, 0.96, 0.97, 0.92) };
    ui.gfx.text(Font::SemiBold, 22.0, r.x, text_y, &title, color, Some(r.w));
    if !subtitle.is_empty() {
        ui.gfx.text(Font::Regular, 19.0, r.x, text_y + 32.0, subtitle, DIM, Some(r.w));
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Zone {
    Header(usize),
    Pills,
    Grid,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind2 {
    /// Buscar, Series or Películas
    Catalog,
    Simulcast,
    MyList,
}

pub struct GridPage {
    tab: Tab,
    kind: Kind2,
    zone: Zone,
    grid: CardGrid,
    field: SearchField,
    sort: Dropdown,
    audio: Dropdown,
    pills: PillRow,
    categories: Vec<Named>,
    seasons: Vec<Named>,
    season: usize,
    started: bool,
    /// focus moves into the grid once the first page arrives
    grab: bool,
    /// Mi Lista: the banner's genres for the focused show
    genres: Option<(String, Vec<String>)>,
}

const SORTS: [(&str, Sort); 3] = [("Reciente", Sort::NewlyAdded), ("Popular", Sort::Popularity), ("Alfabético", Sort::Alphabetical)];
const AUDIOS: [(&str, Audio); 3] = [("Todo", Audio::All), ("Subtitulado", Audio::Subbed), ("Doblado", Audio::Dubbed)];

impl GridPage {
    fn base(tab: Tab, kind: Kind2, query: Query, subtitle: Subtitle, sort: usize) -> GridPage {
        let sorts: Vec<&str> = SORTS.iter().map(|(name, _)| *name).collect();
        let audios: Vec<&str> = AUDIOS.iter().map(|(name, _)| *name).collect();
        let by_date = kind == Kind2::Simulcast;
        GridPage {
            tab,
            kind,
            zone: Zone::Grid,
            grid: CardGrid::new(query, subtitle, by_date),
            field: SearchField::new(720.0),
            sort: Dropdown::new(&sorts, sort, 300.0),
            audio: Dropdown::new(&audios, 0, 300.0),
            pills: PillRow::new(vec!["Todo".into()]),
            categories: Vec::new(),
            seasons: Vec::new(),
            season: 0,
            started: false,
            grab: true,
            genres: None,
        }
    }

    pub fn catalog(tab: Tab) -> GridPage {
        let mut page = GridPage::base(tab, Kind2::Catalog, Query::Browse(Browse::default()), Subtitle::Facts, 1);
        page.grid.query = page.browse();
        page
    }

    pub fn simulcast() -> GridPage {
        GridPage::base(Tab::Simulcast, Kind2::Simulcast, Query::Browse(Browse::default()), Subtitle::Date, 0)
    }

    pub fn my_list() -> GridPage {
        GridPage::base(Tab::MyList, Kind2::MyList, Query::MyList, Subtitle::Facts, 0)
    }

    fn browse(&self) -> Query {
        let media = match self.tab {
            Tab::Series => Some("series"),
            Tab::Movies => Some("movie_listing"),
            _ => None,
        };
        let category = self.pills.selected.checked_sub(1).and_then(|i| self.categories.get(i)).map(|c| c.id.clone());
        let seasonal_tag = (self.kind == Kind2::Simulcast).then(|| self.seasons.get(self.season).map(|s| s.id.clone())).flatten();
        Query::Browse(Browse {
            media,
            sort: SORTS[self.sort.selected].1,
            audio: AUDIOS[self.audio.selected].1,
            category,
            seasonal_tag,
            query: Some(self.field.applied.clone()).filter(|q| !q.trim().is_empty()),
        })
    }

    fn refresh(&mut self) {
        let query = self.browse();
        if query != self.grid.query {
            self.grid.reset(query);
        }
    }

    /// The header's controls, left to right.
    fn header_len(&self) -> usize {
        match self.kind {
            Kind2::Catalog => 3,
            Kind2::Simulcast => 4,
            Kind2::MyList => 0,
        }
    }

    fn grid_top(&self) -> f32 {
        match self.kind {
            Kind2::Catalog => 292.0,
            Kind2::Simulcast => 216.0,
            Kind2::MyList => 596.0,
        }
    }

    /// The season the simulcast opens on: the newest, unless it hasn't started yet by the calendar.
    fn initial_season(seasons: &[Named]) -> usize {
        let (_, month, day) = local_date();
        let md = month * 100 + day;
        let current = if !(321..1221).contains(&md) {
            "winter"
        } else if md < 621 {
            "spring"
        } else if md < 923 {
            "summer"
        } else {
            "fall"
        };
        match seasons.first() {
            Some(first) if first.id.starts_with(current) => 0,
            _ => 1.min(seasons.len().saturating_sub(1)),
        }
    }
}

impl Page for GridPage {
    fn shown(&mut self, env: &mut Env) {
        if !self.started {
            self.started = true;
            match self.kind {
                Kind2::Catalog => env.worker.send(Command::Categories),
                Kind2::Simulcast => env.worker.send(Command::SeasonalTags),
                Kind2::MyList => {}
            }
        } else if self.kind == Kind2::MyList {
            // the list may have changed on a show's page
            self.grid.reset(Query::MyList);
        }
        self.grab = true;
    }

    fn event(&mut self, event: &Event, env: &mut Env) {
        match event {
            Event::Categories(list) if self.kind == Kind2::Catalog => {
                self.categories = list.clone();
                let mut labels = vec!["Todo".to_string()];
                labels.extend(list.iter().map(|c| c.title.clone()));
                self.pills.labels = labels;
            }
            Event::SeasonalTags(list) if self.kind == Kind2::Simulcast => {
                if self.seasons.is_empty() {
                    self.seasons = list.clone();
                    self.season = Self::initial_season(list);
                    self.refresh();
                }
            }
            Event::Genres(id, genres) if self.kind == Kind2::MyList => self.genres = Some((id.clone(), genres.clone())),
            Event::Watchlist(..) if self.kind == Kind2::MyList => self.grid.reset(Query::MyList),
            other => self.grid.event(other, env.worker),
        }
    }

    fn take_focus(&mut self) -> bool {
        if self.grab && self.grid.arrived && self.grid.len() > 0 {
            self.grab = false;
            self.zone = Zone::Grid;
            return true;
        }
        false
    }

    fn key(&mut self, key: Key, env: &mut Env) -> Nav {
        self.grab = false;
        // an open keyboard or dropdown takes every key
        if self.field.keyboard.is_some() {
            match self.field.key(key) {
                FieldNav::Search => self.refresh(),
                FieldNav::Up | FieldNav::Used | FieldNav::Pass => {}
            }
            return Nav::Stay;
        }
        let open = if self.sort.open.is_some() {
            Some(&mut self.sort)
        } else if self.audio.open.is_some() {
            Some(&mut self.audio)
        } else {
            None
        };
        if let Some(dropdown) = open {
            if let DropNav::Changed = dropdown.key(key) {
                self.refresh();
            }
            return Nav::Stay;
        }
        match self.zone {
            Zone::Header(i) => {
                // catalog: field, Orden, Presentación; simulcast: Orden, Presentación, previous, next
                let control = if self.kind == Kind2::Catalog { i } else { i + 1 };
                if key == Key::Ok {
                    match control {
                        0 => {
                            self.field.key(Key::Ok);
                        }
                        1 => {
                            self.sort.key(Key::Ok);
                        }
                        2 => {
                            self.audio.key(Key::Ok);
                        }
                        3 if self.season + 1 < self.seasons.len() => {
                            self.season += 1;
                            self.refresh();
                        }
                        4 if self.season > 0 => {
                            self.season -= 1;
                            self.refresh();
                        }
                        _ => {}
                    }
                    return Nav::Stay;
                }
                match key {
                    Key::Left => self.zone = Zone::Header(i.saturating_sub(1)),
                    Key::Right => self.zone = Zone::Header((i + 1).min(self.header_len() - 1)),
                    Key::Up => return Nav::Bar,
                    Key::Down => {
                        self.zone = if self.kind == Kind2::Catalog {
                            self.pills.focus = self.pills.selected;
                            Zone::Pills
                        } else {
                            Zone::Grid
                        };
                    }
                    Key::Back => return Nav::Back,
                    _ => {}
                }
            }
            Zone::Pills => match key {
                Key::Left => self.pills.focus = self.pills.focus.saturating_sub(1),
                Key::Right => self.pills.focus = (self.pills.focus + 1).min(self.pills.labels.len() - 1),
                Key::Ok => {
                    self.pills.selected = self.pills.focus;
                    self.refresh();
                }
                Key::Up => self.zone = Zone::Header(0),
                Key::Down => self.zone = Zone::Grid,
                Key::Back => return Nav::Back,
                _ => {}
            },
            Zone::Grid => match self.grid.key(key) {
                GridNav::Up => match self.kind {
                    Kind2::Catalog => {
                        self.pills.focus = self.pills.selected;
                        self.zone = Zone::Pills;
                    }
                    Kind2::Simulcast => self.zone = Zone::Header(0),
                    Kind2::MyList => return Nav::Bar,
                },
                GridNav::Open(card) => return Nav::Open(card),
                GridNav::Stay => {
                    if key == Key::Back {
                        return Nav::Back;
                    }
                }
            },
        }
        let _ = env;
        Nav::Stay
    }

    fn draw(&mut self, ui: &mut Ui, state: &State, focused: bool) {
        if self.field.tick() {
            self.refresh();
        }
        let header_y = 136.0 + (72.0 - 64.0) / 2.0;
        let mut drop_lists: Vec<(&Dropdown, f32)> = Vec::new();
        match self.kind {
            Kind2::Catalog => {
                let focus = |i| focused && self.zone == Zone::Header(i);
                self.field.draw(ui, 48.0, header_y, focus(0));
                let sort_x = 48.0 + 720.0 + 24.0;
                self.sort.draw(ui, sort_x, header_y + 2.0, focus(1));
                let audio_x = sort_x + 300.0 + 16.0;
                self.audio.draw(ui, audio_x, header_y + 2.0, focus(2));
                drop_lists.push((&self.sort, sort_x));
                drop_lists.push((&self.audio, audio_x));
                self.pills.draw(ui, 48.0, 232.0, 1824.0, focused && self.zone == Zone::Pills);
            }
            Kind2::Simulcast => {
                let focus = |i| focused && self.zone == Zone::Header(i);
                let mut x = 48.0;
                self.sort.draw(ui, x, header_y + 2.0, focus(0));
                drop_lists.push((&self.sort, x));
                x += 300.0 + 16.0;
                self.audio.draw(ui, x, header_y + 2.0, focus(1));
                drop_lists.push((&self.audio, x));
                x += 300.0 + 16.0;
                let cy = 136.0 + 36.0;
                let older = self.season + 1 < self.seasons.len();
                let newer = self.season > 0;
                if older {
                    ui.circle_button(x + 24.0, cy, 48.0, Icon::Back, Look::new(focus(2), false));
                }
                x += 48.0 + 16.0;
                let title = self.seasons.get(self.season).map(|s| s.title.as_str()).unwrap_or("Simulcast");
                x += ui.gfx.text(Font::SemiBold, 28.0, x, cy - 18.0, title, TEXT, Some(600.0)) + 16.0;
                if newer {
                    ui.circle_button(x + 24.0, cy, 48.0, Icon::ChevronRight, Look::new(focus(3), false));
                }
            }
            Kind2::MyList => self.draw_banner(ui, state),
        }
        let top = self.grid_top();
        let fades = self.kind != Kind2::MyList;
        self.grid.draw(ui, top, 1056.0, focused && self.zone == Zone::Grid, fades);
        for (dropdown, x) in drop_lists {
            dropdown.draw_list(ui, x, header_y + 2.0);
        }
        self.field.draw_keyboard(ui);
    }
}

impl GridPage {
    /// Mi Lista's top half: the focused show's logo, facts and synopsis, its genres and its art.
    fn draw_banner(&mut self, ui: &mut Ui, state: &State) {
        let Some(card) = self.grid.focused().cloned() else { return };
        if !state.settings.lite && self.genres.as_ref().is_none_or(|(id, _)| *id != card.id) {
            self.genres = Some((card.id.clone(), Vec::new()));
            ui.worker.send(Command::Genres(card.id.clone()));
        }
        let x = 48.0 + 16.0;
        let view = HeroView {
            card: &card,
            logo: card.logo.as_deref(),
            logo_known: true,
            size: HeroSize::Compact,
            lines: 3,
            width: 1000.0 - 48.0,
        };
        let bottom = ui.hero(x, 136.0 + 8.0, &view, 1.0);
        if !state.settings.lite {
            if let Some((id, genres)) = &self.genres
                && *id == card.id
                && !genres.is_empty()
            {
                ui.gfx.text(Font::Regular, 20.0, x, bottom + 14.0, &genres.join(" · "), DIM, Some(1000.0 - 48.0));
            }
            let poster = Rect::new(1896.0 - 16.0 - 560.0, 136.0 + 8.0, 560.0, 315.0);
            ui.picture(card.backdrop.as_deref().or(card.image.as_deref()), poster, RADIUS, 1.0, 900);
        }
        let _ = Kind::Series;
    }
}
