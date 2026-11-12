//! A series' or movie's page: its art and facts, a row of actions (play, my list, audio and subtitles, similar
//! shows, its music, mark as watched, stars), then its seasons and episodes (or its movies). Similar and Música
//! swap the episodes for a shelf of their own; audio and subtitles replace the page with two choices.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crunchy_core::content::Show;
use crunchy_core::model::{BACKDROP_WIDTH, Card, Kind};
use serde_json::json;

use crate::gfx::{Color, Font, Icon, Rect};
use crate::platform::Key;

use super::controls::{DropNav, Dropdown};
use super::feed::{DETAIL_DIM_SHADE, DETAIL_SHADE};
use super::i18n::{Text, t, tr};
use super::theme::*;
use super::ui::{Backdrop, HeroSize, HeroView, Look, Ui, row_scroll};
use super::worker::{Command, Event, Query};
use super::{Env, Nav, Page, State};

pub const NO_PLAYER: Text = Text::Key("Playback is coming in a future version.");

static NEXT_KEY: AtomicU64 = AtomicU64::new(1 << 40);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Action {
    Play,
    Watchlist,
    Audio,
    Similar,
    Music,
    Watched,
    Star(u8),
}

impl Action {
    fn hint(self, in_list: bool) -> String {
        match self {
            Action::Play => String::new(),
            Action::Watchlist => if in_list { t("Remove from My List") } else { t("Add to My List") }.into(),
            Action::Audio => t("Audio and subtitles").into(),
            Action::Similar => t("Similar").into(),
            Action::Music => t("Music").into(),
            Action::Watched => t("Mark as watched").into(),
            Action::Star(i) => tr!("Rate {}/5", i),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Zone {
    Actions,
    Seasons,
    Tools,
    Episodes,
    Extra,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ExtraKind {
    Similar,
    Music,
}

struct Extra {
    kind: ExtraKind,
    cards: Option<Vec<Card>>,
    focus: usize,
    scroll: f32,
    key: u64,
}

struct AudioView {
    /// 0 subtitles, 1 audio
    field: usize,
    subtitles: Dropdown,
    audio: Dropdown,
    subtitle_codes: Vec<String>,
    audio_codes: Vec<String>,
}

pub struct ShowPage {
    id: String,
    kind: Kind,
    /// the card the page was opened from, for the hero while the page loads
    card: Card,
    /// an episode the page was opened from: its season opens and it gets the focus
    from: Option<Card>,
    show: Option<Show>,
    failed: bool,
    backdrop: Backdrop,
    zone: Zone,
    action: usize,
    season: usize,
    season_focus: usize,
    /// when the focus came to rest on a season tab (it opens after a moment)
    season_rest: Option<Instant>,
    tabs_scroll: f32,
    tool: usize,
    episodes: HashMap<String, Vec<Card>>,
    requested: HashSet<String>,
    newest_first: bool,
    episode_focus: usize,
    episode_scroll: f32,
    extra: Option<Extra>,
    audio_view: Option<AudioView>,
    marking: bool,
    /// the focus was placed once the page loaded
    placed: bool,
    /// the player closes seen, and whether the page is reloading after one (keeping its place)
    watched: Option<u64>,
    refreshing: bool,
    /// the episode a press of OK would play, and since when (looked up ahead once it has rested a moment)
    warm: Option<(String, Instant, bool)>,
}

const TOP: f32 = 96.0;
const SHELF_TOP: f32 = 640.0;
const EP_W: f32 = 420.0;
const EP_H: f32 = 236.0;
const EP_STEP: f32 = 468.0;

impl ShowPage {
    pub fn new(id: String, kind: Kind, card: Card, from: Option<Card>) -> ShowPage {
        ShowPage {
            id,
            kind,
            card,
            from,
            show: None,
            failed: false,
            backdrop: Backdrop::default(),
            zone: Zone::Actions,
            action: 0,
            season: 0,
            season_focus: 0,
            season_rest: None,
            tabs_scroll: 0.0,
            tool: 0,
            episodes: HashMap::new(),
            requested: HashSet::new(),
            newest_first: false,
            episode_focus: 0,
            episode_scroll: 0.0,
            extra: None,
            audio_view: None,
            marking: false,
            placed: false,
            watched: None,
            refreshing: false,
            warm: None,
        }
    }

    fn actions(&self) -> Vec<Action> {
        let mut list = vec![Action::Play, Action::Watchlist, Action::Audio, Action::Similar, Action::Music];
        if self.kind == Kind::Series {
            list.push(Action::Watched);
        }
        list.extend((1..=5).map(Action::Star));
        list
    }

    /// The list under the actions: a season's episodes, or the movies.
    fn list_id(&self) -> Option<String> {
        if self.kind == Kind::MovieListing {
            return Some(self.id.clone());
        }
        self.show.as_ref()?.seasons.get(self.season).map(|s| s.id.clone())
    }

    fn episodes(&self) -> Option<Vec<Card>> {
        let mut list = self.episodes.get(&self.list_id()?)?.clone();
        if self.newest_first {
            list.reverse();
        }
        Some(list)
    }

    /// What "play" starts: the show's next episode (with its progress), else the first one listed.
    fn to_play(&self) -> Option<Card> {
        self.show.as_ref().and_then(|show| show.up_next.clone()).or_else(|| self.episodes()?.first().cloned())
    }

    fn request_list(&mut self, env_worker: &super::worker::Worker) {
        if let Some(id) = self.list_id()
            && self.requested.insert(id.clone())
        {
            env_worker.send(Command::Episodes(id, self.kind));
        }
    }

    fn select_season(&mut self, index: usize, worker: &super::worker::Worker) {
        if index != self.season {
            self.season = index;
            self.episode_focus = 0;
            self.episode_scroll = 0.0;
        }
        self.request_list(worker);
    }

    /// "Watch S1 · E1", "Continue S2 · E5", or for a movie "Watch" / "Continue".
    fn play_label(&self) -> String {
        let next = self.show.as_ref().and_then(|s| s.up_next.clone());
        let resume = next.as_ref().and_then(|c| c.progress).is_some_and(|p| p > 0.0 && p < 0.98);
        let episode = match (&next, self.kind) {
            (Some(card), Kind::Series) => match &card.episode {
                Some(e) if !e.number.is_empty() => Some(format!("{} · {}", tr!("S{}", e.season), tr!("E{}", e.number))),
                _ => None,
            },
            _ => None,
        };
        match (episode, resume) {
            (Some(episode), true) => tr!("Continue {}", episode),
            (Some(episode), false) => tr!("Watch {}", episode),
            (None, true) => t("Continue").to_string(),
            (None, false) => t("Watch").to_string(),
        }
    }

    fn label_for_season(&self, index: usize) -> String {
        let Some(show) = &self.show else { return String::new() };
        let numbers: Vec<&str> = show.seasons.iter().map(|s| s.display_number.as_str()).collect();
        let unique = numbers.iter().collect::<HashSet<_>>().len() == numbers.len() && numbers.iter().all(|n| !n.is_empty());
        match show.seasons.get(index) {
            Some(season) if unique => tr!("Season {}", season.display_number),
            Some(season) if !season.title.is_empty() => season.title.clone(),
            _ => tr!("Season {}", index + 1),
        }
    }

    fn open_audio_view(&mut self, profile: Option<&crunchy_core::account::Profile>) {
        let Some(show) = &self.show else { return };
        let mut subtitle_codes = vec!["off".to_string()];
        subtitle_codes.extend(show.subtitle_locales.iter().cloned());
        let audio_codes = show.audio_locales.clone();
        // named as they're drawn, in the interface's language
        let names = |codes: &[String]| codes.iter().map(|code| Text::Language(code.clone())).collect::<Vec<_>>();
        let mut subtitles = Dropdown::new(names(&subtitle_codes), 0, 525.0);
        let mut audio = Dropdown::new(names(&audio_codes), 0, 525.0);
        if let Some(profile) = profile {
            subtitles.selected = subtitle_codes.iter().position(|c| *c == profile.subtitle_language).unwrap_or(0);
            audio.selected = audio_codes.iter().position(|c| *c == profile.audio_language).unwrap_or(0);
        }
        self.audio_view = Some(AudioView { field: 0, subtitles, audio, subtitle_codes, audio_codes });
    }

    fn key_actions(&mut self, key: Key, env: &mut Env) -> Nav {
        let actions = self.actions();
        match key {
            Key::Left => self.action = self.action.saturating_sub(1),
            Key::Right => self.action = (self.action + 1).min(actions.len() - 1),
            Key::Down => {
                if self.extra.is_some() {
                    self.zone = Zone::Extra;
                } else if self.kind == Kind::Series && self.show.as_ref().is_some_and(|s| !s.seasons.is_empty()) {
                    self.season_focus = self.season;
                    self.zone = Zone::Seasons;
                } else {
                    self.zone = Zone::Episodes;
                }
            }
            Key::Ok => match actions[self.action] {
                Action::Play => {
                    if let Some(card) = self.to_play() {
                        return Nav::Play(card);
                    }
                }
                Action::Watchlist => {
                    if let Some(show) = self.show.as_mut() {
                        show.in_watchlist = !show.in_watchlist;
                        env.worker.send(Command::SetWatchlist(self.id.clone(), show.in_watchlist));
                    }
                }
                Action::Audio => {
                    let profile = env.state.account.profile().cloned();
                    self.open_audio_view(profile.as_ref());
                }
                Action::Similar | Action::Music => {
                    let kind = if actions[self.action] == Action::Similar { ExtraKind::Similar } else { ExtraKind::Music };
                    if self.extra.as_ref().is_some_and(|e| e.kind == kind) {
                        self.extra = None;
                    } else {
                        let key = NEXT_KEY.fetch_add(1, Ordering::Relaxed);
                        match kind {
                            ExtraKind::Similar => env.worker.send(Command::Grid {
                                key,
                                query: Query::Similar(self.id.clone()),
                                start: 0,
                                count: 30,
                            }),
                            ExtraKind::Music => env.worker.send(Command::Featured(self.id.clone())),
                        }
                        self.extra = Some(Extra { kind, cards: None, focus: 0, scroll: 0.0, key });
                    }
                }
                Action::Watched => env.worker.send(Command::MarkWatched { id: self.id.clone(), season: false }),
                Action::Star(stars) => {
                    if let Some(show) = self.show.as_mut() {
                        show.my_rating = Some(stars);
                        env.worker.send(Command::Rate(self.id.clone(), self.kind, stars));
                    }
                }
            },
            Key::Back => return Nav::Back,
            _ => {}
        }
        Nav::Stay
    }

    fn key_audio(&mut self, key: Key, env: &mut Env) -> Nav {
        let Some(view) = self.audio_view.as_mut() else { return Nav::Stay };
        let dropdown = if view.field == 0 { &mut view.subtitles } else { &mut view.audio };
        match dropdown.key(key) {
            DropNav::Used => return Nav::Stay,
            DropNav::Changed => {
                let (field, code) = if view.field == 0 {
                    ("preferred_content_subtitle_language", view.subtitle_codes.get(view.subtitles.selected).cloned())
                } else {
                    ("preferred_content_audio_language", view.audio_codes.get(view.audio.selected).cloned())
                };
                if let (Some(code), Some(profile)) = (code, env.state.account.profile()) {
                    let value = if code == "off" { serde_json::Value::Null } else { json!(code) };
                    env.worker.send(Command::UpdateProfile { id: profile.id.clone(), fields: json!({ field: value }) });
                }
                return Nav::Stay;
            }
            DropNav::Pass => {}
        }
        match key {
            Key::Up => view.field = 0,
            Key::Down => view.field = 1,
            Key::Back => self.audio_view = None,
            _ => {}
        }
        Nav::Stay
    }
}

impl Page for ShowPage {
    fn shown(&mut self, env: &mut Env) {
        if self.show.is_none() {
            env.worker.send(Command::Show(self.id.clone()));
            if self.kind == Kind::MovieListing {
                self.request_list(env.worker);
            }
        }
    }

    fn event(&mut self, event: &Event, env: &mut Env) {
        match event {
            Event::Show(show) if show.card.id == self.id && self.refreshing => {
                // after the player: the next episode and the stars change, the season and the focus stay
                self.refreshing = false;
                self.show = Some(show.clone());
            }
            Event::Show(show) if show.card.id == self.id => {
                self.show = Some(show.clone());
                // the season of the episode we came from opens, else the season of the one to watch next
                let anchor = self.from.as_ref().or(show.up_next.as_ref());
                if let Some(season_id) = anchor.and_then(|c| c.episode.as_ref()).map(|e| e.season_id.clone())
                    && let Some(index) = show.seasons.iter().position(|s| s.id == season_id)
                {
                    self.season = index;
                }
                self.request_list(env.worker);
            }
            Event::ShowFailed(id) if *id == self.id => self.failed = true,
            Event::Episodes(id, cards) if self.requested.contains(id) => {
                self.episodes.insert(id.clone(), cards.clone());
                if self.list_id().as_ref() == Some(id) {
                    self.marking = false;
                }
            }
            Event::Watchlist(id, on) if *id == self.id => {
                if let Some(show) = self.show.as_mut() {
                    show.in_watchlist = *on;
                }
            }
            Event::Grid { key, page, .. } => {
                if let Some(extra) = self.extra.as_mut().filter(|e| e.key == *key) {
                    extra.cards = Some(page.cards.clone());
                    self.zone = Zone::Extra;
                }
            }
            Event::GridFailed(key) => {
                if let Some(extra) = self.extra.as_mut().filter(|e| e.key == *key) {
                    extra.cards = Some(Vec::new());
                }
            }
            Event::Featured(id, cards) if *id == self.id => {
                if let Some(extra) = self.extra.as_mut().filter(|e| e.kind == ExtraKind::Music) {
                    extra.cards = Some(cards.clone());
                    self.zone = Zone::Extra;
                }
            }
            Event::MarkedWatched(id) if Some(id) == self.list_id().as_ref() => self.marking = false,
            _ => {}
        }
    }

    fn key(&mut self, key: Key, env: &mut Env) -> Nav {
        if self.audio_view.is_some() {
            return self.key_audio(key, env);
        }
        if self.failed {
            return if key == Key::Back { Nav::Back } else { Nav::Stay };
        }
        self.placed = true;
        match self.zone {
            Zone::Actions => return self.key_actions(key, env),
            Zone::Seasons => {
                let count = self.show.as_ref().map(|s| s.seasons.len()).unwrap_or(0);
                match key {
                    Key::Left => {
                        self.season_focus = self.season_focus.saturating_sub(1);
                        self.season_rest = Some(Instant::now());
                    }
                    Key::Right => {
                        if self.season_focus + 1 < count {
                            self.season_focus += 1;
                            self.season_rest = Some(Instant::now());
                        } else {
                            self.tool = 0;
                            self.zone = Zone::Tools;
                        }
                    }
                    Key::Ok => {
                        self.season_rest = None;
                        self.select_season(self.season_focus, env.worker);
                    }
                    Key::Up => self.zone = Zone::Actions,
                    Key::Down => self.zone = Zone::Episodes,
                    Key::Back => return Nav::Back,
                    _ => {}
                }
            }
            Zone::Tools => match key {
                Key::Left => {
                    if self.tool == 0 {
                        self.season_focus = self.show.as_ref().map(|s| s.seasons.len().saturating_sub(1)).unwrap_or(0);
                        self.zone = Zone::Seasons;
                    } else {
                        self.tool = 0;
                    }
                }
                Key::Right => self.tool = 1,
                Key::Ok if self.tool == 0 => {
                    let all_watched = self.episodes().is_some_and(|list| list.iter().all(|c| c.watched));
                    if let Some(id) = self.list_id()
                        && !self.marking
                        && !all_watched
                    {
                        self.marking = true;
                        if let Some(list) = self.episodes.get_mut(&id) {
                            for card in list.iter_mut() {
                                card.watched = true;
                                card.progress = Some(1.0);
                            }
                        }
                        env.worker.send(Command::MarkWatched { id, season: true });
                    }
                }
                Key::Ok => {
                    self.newest_first = !self.newest_first;
                    self.episode_focus = 0;
                }
                Key::Up => self.zone = Zone::Actions,
                Key::Down => self.zone = Zone::Episodes,
                Key::Back => return Nav::Back,
                _ => {}
            },
            Zone::Episodes => {
                let len = self.episodes().map(|l| l.len()).unwrap_or(0);
                match key {
                    Key::Left => self.episode_focus = self.episode_focus.saturating_sub(1),
                    Key::Right => self.episode_focus = (self.episode_focus + 1).min(len.saturating_sub(1)),
                    Key::Up => {
                        if self.kind == Kind::Series {
                            self.season_focus = self.season;
                            self.zone = Zone::Seasons;
                        } else {
                            self.zone = Zone::Actions;
                        }
                    }
                    Key::Ok => {
                        if let Some(card) = self.episodes().and_then(|list| list.get(self.episode_focus).cloned()) {
                            return Nav::Play(card);
                        }
                    }
                    Key::Back => return Nav::Back,
                    _ => {}
                }
            }
            Zone::Extra => {
                let Some(extra) = self.extra.as_mut() else {
                    self.zone = Zone::Episodes;
                    return Nav::Stay;
                };
                let len = extra.cards.as_ref().map(|c| c.len()).unwrap_or(0);
                match key {
                    Key::Left => extra.focus = extra.focus.saturating_sub(1),
                    Key::Right => extra.focus = (extra.focus + 1).min(len.saturating_sub(1)),
                    Key::Up => self.zone = Zone::Actions,
                    Key::Ok => {
                        if let Some(card) = extra.cards.as_ref().and_then(|c| c.get(extra.focus)) {
                            return Nav::Open(card.clone());
                        }
                    }
                    Key::Back => {
                        self.extra = None;
                        self.zone = Zone::Episodes;
                    }
                    _ => {}
                }
            }
        }
        Nav::Stay
    }

    fn draw(&mut self, ui: &mut Ui, state: &State, focused: bool) {
        // the episode OK would play (play, or the episode with the focus) is looked up ahead once the focus rests
        let target = match self.zone {
            Zone::Actions => self.to_play().filter(|card| card.kind == Kind::Episode).map(|card| card.id),
            Zone::Episodes => self.episodes().and_then(|list| list.get(self.episode_focus).map(|card| card.id.clone())),
            _ => None,
        };
        match (&mut self.warm, target) {
            (Some((held, since, sent)), Some(id)) if *held == id => {
                if !*sent && since.elapsed() >= Duration::from_millis(600) {
                    *sent = true;
                    ui.worker.send(Command::Warm { id, audio: state.account.audio() });
                }
            }
            (warm, Some(id)) => *warm = Some((id, Instant::now(), false)),
            (warm, None) => *warm = None,
        }
        // back from the player: what's next and how far each episode was watched come again
        match self.watched {
            None => self.watched = Some(state.watched),
            Some(seen) if seen != state.watched && self.show.is_some() => {
                self.watched = Some(state.watched);
                self.refreshing = true;
                ui.worker.send(Command::Show(self.id.clone()));
                if let Some(id) = self.list_id() {
                    self.requested.remove(&id);
                    self.request_list(ui.worker);
                }
            }
            _ => {}
        }
        // a season tab the focus rested on opens
        if let Some(at) = self.season_rest
            && at.elapsed() >= Duration::from_millis(500)
        {
            self.season_rest = None;
            self.select_season(self.season_focus, ui.worker);
        }
        // the focus starts on play, or on the episode the page was opened from; the list opens at that episode, or
        // at the one to watch next (in place, not sliding there from the first)
        if !self.placed && let Some(show) = &self.show {
            let anchor = match &self.from {
                Some(from) => Some((from.id.clone(), true)),
                None => show.up_next.as_ref().map(|next| (next.id.clone(), false)),
            };
            match anchor {
                Some((id, take_focus)) => {
                    if let Some(list) = self.episodes() {
                        if let Some(index) = list.iter().position(|c| c.id == id) {
                            self.episode_focus = index;
                            self.episode_scroll = index.saturating_sub(2) as f32 * EP_STEP;
                            if take_focus {
                                self.zone = Zone::Episodes;
                            }
                        }
                        self.placed = true;
                    }
                }
                None => self.placed = true,
            }
        }

        let art = self.show.as_ref().and_then(|s| s.hero.clone().or(s.card.backdrop.clone()));
        let wanted = art.or_else(|| matches!(self.card.kind, Kind::Series | Kind::MovieListing).then(|| self.card.backdrop.clone()).flatten());
        let preview = self.show.as_ref().and_then(|s| s.preview.clone());
        let shade = if self.audio_view.is_some() { &DETAIL_DIM_SHADE } else { &DETAIL_SHADE };
        ui.backdrop(&mut self.backdrop, wanted, preview, shade);

        if self.failed {
            ui.gfx.text(Font::Bold, 48.0, PAD, 200.0, t("Couldn’t load"), TEXT, None);
            ui.gfx.text(Font::Regular, 26.0, PAD, 280.0, t("Press Back to return."), MUTED, None);
            return;
        }

        let hero_card = match &self.show {
            Some(show) => Some(show.card.clone()),
            None if matches!(self.card.kind, Kind::Series | Kind::MovieListing) => Some(self.card.clone()),
            None => None,
        };
        let Some(hero_card) = hero_card else {
            ui.skeleton(Rect::new(PAD, TOP + 40.0, 560.0, 140.0), 12.0);
            ui.skeleton(Rect::new(PAD, TOP + 210.0, 700.0, 26.0), 8.0);
            return;
        };

        if let Some(view) = self.audio_view.as_ref() {
            let hero = HeroView {
                card: &hero_card,
                logo: self.show.as_ref().and_then(|s| s.logo.as_deref()),
                logo_known: self.show.is_some(),
                size: HeroSize::Detail,
                lines: 2,
                width: 1100.0,
            };
            let mut y = ui.hero(PAD, TOP, &hero, 1.0) + 36.0;
            let mut at = Vec::with_capacity(2);
            for (index, (label, dropdown)) in [(t("SUBTITLES"), &view.subtitles), (t("AUDIO"), &view.audio)].into_iter().enumerate() {
                field_title(ui, label, PAD + 6.0, y);
                y += 36.0;
                dropdown.draw(ui, PAD, y, focused && view.field == index);
                at.push(y);
                y += 60.0 + 22.0;
            }
            // the open list over everything
            view.subtitles.draw_list(ui, PAD, at[0]);
            view.audio.draw_list(ui, PAD, at[1]);
            return;
        }

        let hero = HeroView {
            card: &hero_card,
            logo: self.show.as_ref().and_then(|s| s.logo.as_deref()),
            logo_known: self.show.is_some(),
            size: HeroSize::Detail,
            lines: 3,
            width: 980.0,
        };
        let bottom = ui.hero(PAD, TOP, &hero, 1.0);
        let actions_y = (bottom + 30.0).min(SHELF_TOP - 64.0 - 24.0);
        self.draw_actions(ui, state, actions_y, focused);
        self.draw_shelf(ui, focused);
    }
}

fn field_title(ui: &mut Ui, text: &str, x: f32, y: f32) {
    let mut cx = x;
    for ch in text.chars() {
        cx += ui.gfx.text(Font::Bold, 18.0, cx, y, &ch.to_string(), DIM, None) + 1.4;
    }
}

impl ShowPage {
    fn draw_actions(&mut self, ui: &mut Ui, _state: &State, y: f32, focused: bool) {
        let Some(show) = self.show.clone() else {
            ui.skeleton(Rect::new(PAD, y, 260.0, 64.0), 32.0);
            for index in 0..5 {
                ui.skeleton(Rect::new(PAD + 276.0 + index as f32 * 80.0, y, 64.0, 64.0), 32.0);
            }
            return;
        };
        let actions = self.actions();
        self.action = self.action.min(actions.len() - 1);
        let in_actions = focused && self.zone == Zone::Actions;
        let mut x = PAD;
        let mut hint = String::new();
        for (index, action) in actions.iter().enumerate() {
            let look = Look::new(in_actions && index == self.action, false);
            match action {
                Action::Play => {
                    let label = self.play_label();
                    x += glass_pill(ui, x, y, &label, look) + 16.0;
                }
                Action::Star(stars) => {
                    if *stars == 1 {
                        x += 8.0;
                    }
                    let filled = show.my_rating.is_some_and(|r| *stars <= r);
                    let cx = x + 24.0;
                    let cy = y + 32.0;
                    if look.focused {
                        ui.gfx.rect(Rect::new(x, y + 8.0, 48.0, 48.0), 24.0, TEXT);
                    }
                    let (icon, ink) = if filled {
                        (Icon::Star, if look.focused { BG } else { GOLD })
                    } else {
                        (Icon::StarOutline, if look.focused { BG } else { MUTED })
                    };
                    ui.gfx.icon_centered(icon, cx, cy, 22.0, ink);
                    x += 48.0 + 6.0;
                }
                other => {
                    let icon = match other {
                        Action::Watchlist => {
                            if show.in_watchlist {
                                Icon::Check
                            } else {
                                Icon::Plus
                            }
                        }
                        Action::Audio => Icon::Bubble,
                        Action::Similar => Icon::Grid,
                        Action::Music => Icon::Note,
                        _ => Icon::CheckCircle,
                    };
                    let selected = match other {
                        Action::Similar => self.extra.as_ref().is_some_and(|e| e.kind == ExtraKind::Similar),
                        Action::Music => self.extra.as_ref().is_some_and(|e| e.kind == ExtraKind::Music),
                        _ => false,
                    };
                    glass_circle(ui, x + 32.0, y + 32.0, icon, Look::new(look.focused, selected));
                    x += 64.0 + 16.0;
                }
            }
            if look.focused {
                hint = action.hint(show.in_watchlist);
            }
        }
        // the hint for the button with the focus, or how much is left of the next episode
        let hint_x = x - 6.0 + 20.0;
        if !hint.is_empty() {
            ui.gfx.text(Font::SemiBold, 20.0, hint_x, y + 20.0, &hint, MUTED, Some(1872.0 - hint_x));
        } else if let Some(next) = &show.up_next {
            ui.resume_line(next, hint_x, y + 20.0, 140.0, 1.0);
        }
    }

    fn draw_shelf(&mut self, ui: &mut Ui, focused: bool) {
        let head_y = SHELF_TOP;
        if let Some(extra) = self.extra.as_mut() {
            let title = if extra.kind == ExtraKind::Similar { t("Similar") } else { t("Music") };
            ui.gfx.text(Font::SemiBold, 28.0, PAD, head_y + 6.0, title, Color(0.96, 0.96, 0.97, 0.92), None);
            let cards_y = head_y + 48.0 + 6.0 + 24.0;
            match &extra.cards {
                None => {
                    for slot in 0..4 {
                        ui.skeleton(Rect::new(PAD + slot as f32 * CARD_STEP, cards_y, CARD_W, CARD_H), RADIUS);
                    }
                }
                Some(cards) if cards.is_empty() => {
                    ui.text_centered(Font::SemiBold, 28.0, 960.0, cards_y + 80.0, t("Empty"), DIM);
                }
                Some(cards) => {
                    extra.scroll = row_scroll(extra.scroll, extra.focus, 3, CARD_STEP, ui.dt);
                    for (index, card) in cards.iter().enumerate() {
                        let x = PAD + index as f32 * CARD_STEP - extra.scroll;
                        if x < -CARD_STEP || x > 1920.0 {
                            continue;
                        }
                        let focus = focused && self.zone == Zone::Extra && index == extra.focus;
                        ui.card(card, Rect::new(x, cards_y, CARD_W, CARD_H), focus, 1.0);
                    }
                }
            }
            return;
        }

        if self.kind == Kind::Series {
            self.draw_season_head(ui, head_y, focused);
        } else {
            ui.gfx.text(Font::SemiBold, 28.0, PAD, head_y + 6.0, t("Movies"), Color(0.96, 0.96, 0.97, 0.92), None);
        }
        let cards_y = head_y + 48.0 + 6.0 + 24.0;
        match self.episodes() {
            None => {
                if self.show.as_ref().is_some_and(|s| s.seasons.is_empty()) && self.kind == Kind::Series {
                    ui.text_centered(Font::SemiBold, 28.0, 960.0, cards_y + 80.0, t("Empty"), DIM);
                    return;
                }
                for slot in 0..4 {
                    let x = PAD + slot as f32 * EP_STEP;
                    ui.skeleton(Rect::new(x, cards_y, EP_W, EP_H), RADIUS);
                    ui.skeleton(Rect::new(x, cards_y + EP_H + 22.0, EP_W * 0.72, 20.0), 6.0);
                    ui.skeleton(Rect::new(x, cards_y + EP_H + 54.0, EP_W * 0.86, 16.0), 6.0);
                }
            }
            Some(list) if list.is_empty() => {
                ui.text_centered(Font::SemiBold, 28.0, 960.0, cards_y + 80.0, t("Empty"), DIM);
            }
            Some(list) => {
                self.episode_focus = self.episode_focus.min(list.len() - 1);
                self.episode_scroll = row_scroll(self.episode_scroll, self.episode_focus, 2, EP_STEP, ui.dt);
                for (index, card) in list.iter().enumerate() {
                    let x = PAD + index as f32 * EP_STEP - self.episode_scroll;
                    if x < -EP_STEP || x > 1920.0 {
                        continue;
                    }
                    let focus = focused && self.zone == Zone::Episodes && index == self.episode_focus;
                    ui.episode(card, Rect::new(x, cards_y, EP_W, EP_H), focus, 1.0);
                }
                for card in list.iter().skip((self.episode_scroll / EP_STEP) as usize + 4).take(2) {
                    if let Some(url) = &card.image {
                        ui.images.prefetch(ui.worker, url, super::ui::CARD_PICTURE);
                    }
                }
            }
        }
        let _ = BACKDROP_WIDTH;
    }

    fn draw_season_head(&mut self, ui: &mut Ui, y: f32, focused: bool) {
        let Some(show) = self.show.clone() else { return };
        // the tools on the right first: the tabs get what's left
        let all_watched = self.episodes().is_some_and(|list| !list.is_empty() && list.iter().all(|c| c.watched));
        let mark_label = t("Mark season as watched");
        let sort_label = if self.newest_first { t("Newest first") } else { t("Oldest first") };
        let tool_w = |ui: &mut Ui, label: &str| ui.gfx.measure(Font::SemiBold, 22.0, label) + 2.0 * 20.0 + 22.0 * 1.1 + 12.0;
        let mark_w = tool_w(ui, mark_label);
        let sort_w = tool_w(ui, sort_label);
        let tools_x = 1872.0 - sort_w - 6.0 - mark_w;
        let in_tools = focused && self.zone == Zone::Tools;
        let disabled = all_watched || self.marking;
        let mark_look = Look::new(in_tools && self.tool == 0, false);
        if disabled && !mark_look.focused {
            // dimmed: drawn in the faint colour
            ui.gfx.icon(Icon::Check, tools_x + 20.0, y + 13.0, 24.0, DIM.alpha(0.6));
            ui.gfx.text(Font::SemiBold, 22.0, tools_x + 20.0 + 24.0 + 12.0, y + 10.0, mark_label, DIM.alpha(0.6), None);
        } else {
            ui.pill_plain(tools_x, y, 48.0, mark_label, Some(Icon::Check), 22.0, mark_look);
        }
        ui.pill_plain(tools_x + mark_w + 6.0, y, 48.0, sort_label, Some(Icon::Sort), 22.0, Look::new(in_tools && self.tool == 1, false));

        // season tabs, sliding to keep the focused one in view
        let strip_w = tools_x - 24.0 - PAD;
        let labels: Vec<String> = (0..show.seasons.len()).map(|i| self.label_for_season(i)).collect();
        let mut starts = Vec::with_capacity(labels.len());
        let mut at = 0.0;
        for label in &labels {
            starts.push(at);
            at += (ui.gfx.measure(Font::SemiBold, 22.0, label) + 44.0).min(420.0) + 6.0;
        }
        let total = at;
        let in_tabs = focused && self.zone == Zone::Seasons;
        let anchor = if in_tabs { self.season_focus } else { self.season };
        if let Some(start) = starts.get(anchor).copied() {
            let end = starts.get(anchor + 1).copied().unwrap_or(total) - 6.0;
            let mut target = self.tabs_scroll;
            if start - target < 64.0 {
                target = (start - 64.0).max(0.0);
            }
            if end - target > strip_w - 64.0 {
                target = end - strip_w + 64.0;
            }
            self.tabs_scroll = approach(self.tabs_scroll, target.clamp(0.0, (total - strip_w).max(0.0)), ui.dt, 14.0);
        }
        ui.gfx.clip(Some(Rect::new(PAD, y - 6.0, strip_w, 60.0)));
        for (index, label) in labels.iter().enumerate() {
            let x = PAD + starts[index] - self.tabs_scroll;
            let look = Look::new(in_tabs && index == self.season_focus, index == self.season);
            let w = (ui.gfx.measure(Font::SemiBold, 22.0, label) + 44.0).min(420.0);
            let (fill, ink) = if look.focused {
                (Some(TEXT), BG)
            } else if look.selected {
                (Some(VEIL_STRONG), TEXT)
            } else {
                (None, MUTED)
            };
            if let Some(fill) = fill {
                ui.gfx.rect(Rect::new(x, y, w, 48.0), 24.0, fill);
            }
            ui.gfx.text(Font::SemiBold, 22.0, x + 22.0, y + 10.0, label, ink, Some(w - 44.0));
        }
        ui.gfx.clip(None);
        if self.tabs_scroll > 1.0 {
            ui.gfx.gradient(Rect::new(PAD, y - 6.0, 64.0, 60.0), BG, BG.alpha(0.0), false);
        }
        if total - self.tabs_scroll > strip_w + 1.0 {
            ui.gfx.gradient(Rect::new(PAD + strip_w - 64.0, y - 6.0, 64.0, 60.0), BG.alpha(0.0), BG, false);
        }
    }
}

/// The play pill over art: a light glass at rest, white with focus. Returns its width.
fn glass_pill(ui: &mut Ui, x: f32, y: f32, label: &str, look: Look) -> f32 {
    let w = ui.gfx.measure(Font::SemiBold, 23.0, label) + 60.0 + 26.0 + 12.0;
    let mut r = Rect::new(x, y, w, 64.0);
    if look.focused {
        r = r.scaled(1.06);
        ui.gfx.rect(r, 32.0, TEXT);
    } else {
        ui.gfx.rect(r.inset(-1.0), 33.0, Color(1.0, 1.0, 1.0, 0.10));
        ui.gfx.rect(r, 32.0, Color(0.3, 0.3, 0.33, 0.72));
    }
    let ink = if look.focused { BG } else { TEXT };
    ui.gfx.icon(Icon::Play, r.x + 30.0, r.y + (r.h - 26.0) / 2.0, 26.0, ink);
    ui.gfx.text(Font::SemiBold, 23.0, r.x + 30.0 + 26.0 + 12.0, r.y + r.h / 2.0 - 15.0, label, ink, None);
    w
}

fn glass_circle(ui: &mut Ui, cx: f32, cy: f32, icon: Icon, look: Look) {
    let d = if look.focused { 64.0 * 1.06 } else { 64.0 };
    let r = Rect::new(cx - d / 2.0, cy - d / 2.0, d, d);
    let ink = if look.focused {
        ui.gfx.rect(r, d / 2.0, TEXT);
        BG
    } else {
        ui.gfx.rect(r.inset(-1.0), d / 2.0 + 1.0, Color(1.0, 1.0, 1.0, 0.10));
        let fill = if look.selected { Color(0.45, 0.45, 0.48, 0.8) } else { Color(0.3, 0.3, 0.33, 0.72) };
        ui.gfx.rect(r, d / 2.0, fill);
        TEXT
    };
    ui.gfx.icon_centered(icon, cx, cy, 26.0, ink);
}
