//! The player, laid out like the web app's (moonstone's video player in its Apple TV skin). The video fills the
//! screen; a key brings up, over a fade at the bottom, the title and the times, a white progress bar and a row of
//! round buttons: information, mark as watched and the thumbs on the left; the previous episode, play/pause and the
//! next one in the middle; subtitles, audio and quality on the right, each with its menu above it. A button's name
//! shows under it once the focus rests there. With the controls hidden, left and right jump (5, 10, 20, then 30
//! seconds a press in a row); on the bar they move the knob; either way the video goes there once the arrows rest.
//! "Saltar intro" (and the recap, credits and preview) shows, focused, where Crunchyroll marked them; over the
//! credits and the preview it's "Siguiente episodio" when there's one, and "Terminar" after the last. Where the video
//! was left is saved every 15 seconds and on the way out, and at the end the next episode plays.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crunchy_core::content::SkipEvent;
use crunchy_core::model::{BACKDROP_WIDTH, Card, Kind};
use crunchy_player::State as PlayState;

use crate::gfx::{Color, Font, Icon, Rect};
use crate::platform::{self, Key};
#[cfg(not(target_os = "linux"))]
use crate::player::Tracks;
use crate::player::{Playback, Wanted};

use super::theme::*;
use super::worker::{Command, Event, Worker};
use super::ui::{CARD_PICTURE, Ui};
use super::{Env, Nav, Page, State};

/// The controls hide this long after the last key (unless paused).
const HIDE_AFTER: Duration = Duration::from_secs(5);
/// A jump is made once the arrows rest this long.
const JUMP_SETTLE: Duration = Duration::from_millis(700);
/// Jumps in a row grow along this ladder (seconds); past 30 only on the bar with its thumbnails in view, where
/// it's clear where the picture is going.
const JUMP_LADDER: [u64; 6] = [5, 10, 20, 30, 60, 90];
const JUMP_BLIND_MAX: u64 = 30;
/// The seek thumbnails are asked for this long after the picture starts (its first segments get the network).
const THUMBS_AFTER: Duration = Duration::from_secs(4);
const THUMB_W: f32 = 384.0;
const THUMB_H: f32 = 216.0;
/// A rebuffer shows the spinner once it has lasted this long (most are shorter).
const SPINNER_AFTER: Duration = Duration::from_millis(350);
const SAVE_EVERY: Duration = Duration::from_secs(15);
/// The next episode starts this long after the end.
const NEXT_AFTER: Duration = Duration::from_secs(2);
/// A button's name shows under it once the focus rests this long.
const NAME_AFTER: Duration = Duration::from_millis(1200);
/// "Previous" restarts the episode after this much of it, and goes to the one before only earlier.
const RESTART_AFTER: Duration = Duration::from_secs(30);
/// A skip button shows at most this many seconds from the start of what it skips.
const SKIP_FOR: f32 = 15.0;

// Moonstone's layout at 1080p (1rem = 24 px): the controls 2.5rem from the sides and 1.25rem from the bottom.
const SIDE: f32 = 60.0;
const BUTTON: f32 = 84.0;
const GAP: f32 = 18.0;
const ROW_Y: f32 = 966.0;
const BAR_Y: f32 = 921.0;
const BAR_H: f32 = 9.0;
const TITLE_Y: f32 = 826.0;
const TIMES_Y: f32 = 848.0;
/// Where the fade under the controls starts (23rem above the bottom).
const FADE_TOP: f32 = 528.0;
const SKIP_H: f32 = 84.0;
/// How long the still takes to fade away once the video shows.
const STILL_FADE: Duration = Duration::from_millis(600);

const QUALITIES: [(&str, &str); 6] =
    [("Automática", "adaptive"), ("1080p", "1080p"), ("720p", "720p"), ("480p", "480p"), ("360p", "360p"), ("240p", "240p")];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Control {
    Info,
    Watched,
    ThumbDown,
    ThumbUp,
    Previous,
    PlayPause,
    Next,
    Subtitles,
    Audio,
    Quality,
}

impl Control {
    fn name(self, paused: bool) -> &'static str {
        match self {
            Control::Info => "Información",
            Control::Watched => "Marcar como visto",
            Control::ThumbDown => "No me gusta",
            Control::ThumbUp => "Me gusta",
            Control::Previous => "Episodio anterior",
            Control::PlayPause if paused => "Reproducir",
            Control::PlayPause => "Pausar",
            Control::Next => "Episodio siguiente",
            Control::Subtitles => "Subtítulos",
            Control::Audio => "Audio",
            Control::Quality => "Calidad",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Focus {
    Bar,
    Button(Control),
    Skip,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MenuKind {
    Subtitles,
    Audio,
    Quality,
}

/// A menu over its button: labels and the values they stand for.
struct Menu {
    kind: MenuKind,
    title: &'static str,
    options: Vec<(String, String)>,
    selected: usize,
    focus: usize,
}

/// Where the arrows are taking the video: it goes there once they rest.
#[derive(Clone, Copy)]
struct Jump {
    to: Duration,
    at: Instant,
}

pub struct PlayerPage {
    /// what plays (the previous and next buttons change it)
    card: Card,
    wanted: Wanted,
    /// the id last asked of the worker, to know its answer
    requested: Option<String>,
    playback: Option<Arc<Playback>>,
    failed: Option<String>,
    /// the picture has shown (until then the page draws its own background)
    started: bool,
    /// when the version playing was asked for, and when its picture showed (the still fades away over it)
    opened_at: Instant,
    started_at: Option<Instant>,
    playing_since: Option<Instant>,
    /// anything played while the page was open (progress elsewhere needs reloading when it closes)
    ever_played: bool,
    /// where the picture was when it stopped after playing (the TV took the decoder back while paused under the
    /// screensaver, the network went): OK opens it again there
    stopped_at: Option<Duration>,
    /// since when the picture has been waiting for data (a seek, a slow network)
    loading_since: Option<Instant>,
    /// seek thumbnails: wanted (the setting), asked for (the stream's key), and when they came, their times
    previews: bool,
    thumbs_asked: Option<String>,
    thumbs: Option<(String, Vec<u32>)>,
    /// the thumbnail shown last, kept while the next one decodes
    thumb_shown: Option<usize>,
    /// the controls show (they fade in and out)
    controls: bool,
    shown: f32,
    /// the last key
    activity: Instant,
    focus: Focus,
    focus_at: Instant,
    /// the button the focus goes back to from the bar
    last_button: Control,
    menu: Option<Menu>,
    info: bool,
    jump: Option<Jump>,
    /// the last jump's direction, size and time, to double the next one
    steps: Option<(bool, u64, Instant)>,
    /// the skip events of the version playing (its id)
    skips: Vec<SkipEvent>,
    skips_for: String,
    skip_active: Option<usize>,
    /// a skip button Back or a skip put away
    skip_dismissed: Option<usize>,
    /// the profile's thumb on the episode: "up", "down" or none
    rating: Option<String>,
    saved_at: Instant,
    ended_at: Option<Instant>,
    /// the next (true) or previous (false) episode was asked for, to go there
    adjacent: Option<bool>,
    /// the episode after this one, asked for as it starts playing: unknown, none, or it
    next: Option<Option<Card>>,
    next_asked: bool,
    notice: Option<(String, Instant)>,
    /// nothing comes after the end: the page closes
    done: bool,
    /// the simulator's stand-in: no worker requests, a clock instead of a video
    demo: bool,
}

impl PlayerPage {
    /// Plays `card` from where it was left (unless it was watched, or nearly).
    pub fn new(card: Card) -> PlayerPage {
        let duration = card.episode.as_ref().map_or(0, |episode| episode.duration_secs) as f32;
        let from = match card.progress {
            Some(progress) if !card.watched && progress > 0.0 && progress < 0.95 => {
                Duration::from_secs_f32(duration * progress)
            }
            _ => Duration::ZERO,
        };
        let now = Instant::now();
        PlayerPage {
            card,
            wanted: Wanted { audio: None, subtitles: None, max_height: 1080, from },
            requested: None,
            playback: None,
            failed: None,
            started: false,
            opened_at: now,
            started_at: None,
            playing_since: None,
            ever_played: false,
            stopped_at: None,
            loading_since: None,
            previews: true,
            thumbs_asked: None,
            thumbs: None,
            thumb_shown: None,
            controls: true,
            shown: 0.0,
            activity: now,
            focus: Focus::Button(Control::PlayPause),
            focus_at: now,
            last_button: Control::PlayPause,
            menu: None,
            info: false,
            jump: None,
            steps: None,
            skips: Vec::new(),
            skips_for: String::new(),
            skip_active: None,
            skip_dismissed: None,
            rating: None,
            saved_at: now,
            ended_at: None,
            adjacent: None,
            next: None,
            next_asked: false,
            notice: None,
            done: false,
            demo: false,
        }
    }

    /// The player on a made-up episode with a clock for a video, to see the controls in the simulator (`last`: the
    /// series' last episode, nothing after it).
    #[cfg(not(target_os = "linux"))]
    pub fn demo(last: bool) -> PlayerPage {
        let card = Card {
            id: "DEMO".into(),
            kind: Kind::Episode,
            title: "Frieren: Beyond Journey's End".into(),
            episode: Some(crunchy_core::model::EpisodeInfo {
                season: 1,
                season_id: String::new(),
                number: "26".into(),
                name: "La altura de la magia".into(),
                duration_secs: 24 * 60,
            }),
            description: "El examen de magos de primera clase entra en su etapa final. Frieren y sus compañeros \
                          descubren que la mazmorra copia a quien entra en ella."
                .into(),
            ..Card::default()
        };
        let mut next = card.clone();
        next.id = "DEMO2".into();
        if let Some(episode) = next.episode.as_mut() {
            episode.number = "27".into();
            episode.name = "Un verdadero mago".into();
        }
        let mut page = PlayerPage::new(card);
        page.demo = true;
        page.next = Some(if last { None } else { Some(next) });
        page
    }

    fn send(&self, worker: &Worker, command: Command) {
        if !self.demo {
            worker.send(command);
        }
    }

    // ---- playing ----

    /// Asks for `id` (a version of the card) with what's wanted, dropping what played.
    fn play(&mut self, worker: &Worker, id: String) {
        self.playback = None;
        self.started = false;
        self.started_at = None;
        self.playing_since = None;
        self.loading_since = None;
        self.thumb_shown = None;
        self.opened_at = Instant::now();
        self.failed = None;
        self.jump = None;
        self.ended_at = None;
        self.menu = None;
        self.skip_active = None;
        platform::want_video_window(true);
        if self.demo {
            #[cfg(not(target_os = "linux"))]
            self.play_demo();
            return;
        }
        self.requested = Some(id.clone());
        worker.send(Command::Play { id, wanted: self.wanted.clone() });
    }

    #[cfg(not(target_os = "linux"))]
    fn play_demo(&mut self) {
        let audio = self.wanted.audio.clone().unwrap_or_else(|| "ja-JP".into());
        let subtitles = vec!["en-US".to_string(), "es-419".into(), "pt-BR".into()];
        let tracks = Tracks {
            id: "DEMO".into(),
            subtitle: self.wanted.subtitles.clone().filter(|wanted| *wanted != audio && subtitles.contains(wanted)),
            audio,
            audios: vec![("ja-JP".into(), "DEMO".into()), ("es-419".into(), "DEMO".into()), ("en-US".into(), "DEMO".into())],
            subtitles,
        };
        let duration = Duration::from_secs(self.card.episode.as_ref().map_or(1440, |e| e.duration_secs) as u64);
        self.playback = Some(Arc::new(Playback::simulated(duration, self.wanted.from, tracks)));
        self.skips = vec![
            SkipEvent { kind: "intro".into(), start: 2.0, end: 30.0 },
            SkipEvent { kind: "credits".into(), start: 32.0, end: 60.0 },
        ];
        self.rating = Some("up".into());
    }

    /// Plays the same card again from where it is, after a menu changed what's wanted (`id`: another version).
    fn reopen(&mut self, worker: &Worker, id: Option<String>) {
        let position = self.position();
        self.save(worker, position);
        self.wanted.from = position;
        let id = id
            .or_else(|| self.playback.as_ref().map(|playback| playback.tracks.id.clone()))
            .unwrap_or_else(|| self.card.id.clone());
        self.play(worker, id);
    }

    /// Plays another episode from its start, the one playing left at `left_at`.
    fn change(&mut self, worker: &Worker, card: Card, left_at: Duration) {
        self.save(worker, left_at);
        self.card = card;
        self.wanted.from = Duration::ZERO;
        self.rating = None;
        self.skips.clear();
        self.skips_for.clear();
        self.skip_dismissed = None;
        self.adjacent = None;
        self.next = None;
        self.next_asked = false;
        self.controls = true;
        if self.card.kind == Kind::Episode {
            self.send(worker, Command::EpisodeRating(self.card.id.clone()));
        }
        let id = self.card.id.clone();
        self.play(worker, id);
    }

    fn position(&self) -> Duration {
        match (&self.jump, &self.playback) {
            (Some(jump), _) => jump.to,
            (None, Some(playback)) if self.started => playback.status().position,
            _ => self.wanted.from,
        }
    }

    /// Tells Crunchyroll where the video was left.
    fn save(&mut self, worker: &Worker, position: Duration) {
        self.saved_at = Instant::now();
        if self.started && position > Duration::ZERO {
            let seconds = position.as_secs() as u32;
            self.send(worker, Command::SavePlayhead { id: self.card.id.clone(), seconds });
        }
    }

    fn leave(&mut self, worker: &Worker) -> Nav {
        let position = self.position();
        self.leave_at(worker, position)
    }

    fn leave_at(&mut self, worker: &Worker, position: Duration) -> Nav {
        self.save(worker, position);
        Nav::Back
    }

    /// Asks for the episode after this one, so the credits know what to offer.
    fn prefetch_next(&mut self, worker: &Worker) {
        if self.card.kind == Kind::Episode && self.next.is_none() && !self.next_asked {
            self.next_asked = true;
            self.send(worker, Command::Adjacent { id: self.card.id.clone(), forward: true });
        }
    }

    /// Goes to the next (or previous) episode: at once when it's known, else once it arrives.
    fn ask_adjacent(&mut self, worker: &Worker, forward: bool) {
        if forward && let Some(next) = self.next.clone() {
            self.go(worker, forward, next);
        } else if self.adjacent.is_none() {
            self.adjacent = Some(forward);
            self.send(worker, Command::Adjacent { id: self.card.id.clone(), forward });
        }
    }

    fn go(&mut self, worker: &Worker, forward: bool, card: Option<Card>) {
        match card {
            Some(card) => {
                let position = self.position();
                self.change(worker, card, position);
            }
            None if self.ended_at.is_some() => self.done = true,
            None if forward => self.say("No hay un episodio siguiente"),
            None => self.say("No hay un episodio anterior"),
        }
    }

    /// Whether a skip event comes at the end (credits, the next episode's preview).
    fn ending(event: &SkipEvent) -> bool {
        event.kind == "credits" || event.kind == "preview"
    }

    /// What a skip button says and shows: over the end, the next episode or the way out once they're known.
    fn skip_look(&self, event: &SkipEvent) -> (&'static str, Icon) {
        if Self::ending(event) {
            match &self.next {
                Some(Some(_)) => return ("Siguiente episodio", Icon::NextEpisode),
                Some(None) => return ("Terminar", Icon::Xmark),
                None => {}
            }
        }
        let label = match event.kind.as_str() {
            "recap" => "Saltar resumen",
            "intro" => "Saltar intro",
            "credits" => "Saltar créditos",
            _ => "Saltar avance",
        };
        (label, Icon::Skip)
    }

    fn say(&mut self, text: &str) {
        self.notice = Some((text.to_string(), Instant::now()));
    }

    // ---- controls ----

    fn show_controls(&mut self) {
        if !self.controls {
            self.controls = true;
            // the focused button's name waits again, as if the focus had just arrived
            self.focus_at = Instant::now();
            if let Focus::Button(_) = self.focus {
                self.set_focus(Focus::Button(self.last_button));
            }
        }
    }

    fn set_focus(&mut self, focus: Focus) {
        let focus = match focus {
            Focus::Button(control) if !self.order().contains(&control) => Focus::Button(Control::PlayPause),
            other => other,
        };
        if let Focus::Button(control) = focus {
            self.last_button = control;
        }
        if focus != self.focus {
            self.focus = focus;
            self.focus_at = Instant::now();
        }
    }

    /// The buttons: left, middle and right groups.
    fn groups(&self) -> [Vec<Control>; 3] {
        let episode = self.card.kind == Kind::Episode;
        let mut left = vec![Control::Info];
        let mut middle = vec![Control::PlayPause];
        if episode {
            left.extend([Control::Watched, Control::ThumbDown, Control::ThumbUp]);
            middle = vec![Control::Previous, Control::PlayPause, Control::Next];
        }
        let tracks = self.playback.as_ref().map(|playback| &playback.tracks);
        let mut right = Vec::new();
        if tracks.is_some_and(|tracks| !tracks.subtitles.is_empty()) {
            right.push(Control::Subtitles);
        }
        if tracks.is_some_and(|tracks| tracks.audios.len() > 1) {
            right.push(Control::Audio);
        }
        right.push(Control::Quality);
        [left, middle, right]
    }

    fn order(&self) -> Vec<Control> {
        self.groups().concat()
    }

    /// Each button's centre along the row.
    fn layout(&self) -> Vec<(Control, f32)> {
        let [left, middle, right] = self.groups();
        let step = BUTTON + GAP;
        let mut out = Vec::new();
        for (index, control) in left.iter().enumerate() {
            out.push((*control, SIDE + BUTTON / 2.0 + index as f32 * step));
        }
        let width = middle.len() as f32 * step - GAP;
        for (index, control) in middle.iter().enumerate() {
            out.push((*control, (SCREEN_W - width) / 2.0 + BUTTON / 2.0 + index as f32 * step));
        }
        for (index, control) in right.iter().enumerate() {
            let from_end = (right.len() - 1 - index) as f32;
            out.push((*control, SCREEN_W - SIDE - BUTTON / 2.0 - from_end * step));
        }
        out
    }

    fn toggle(&mut self, playback: &Playback) {
        if playback.status().state == PlayState::Paused {
            playback.resume();
        } else {
            playback.pause();
        }
        self.show_controls();
    }

    /// One more jump: 5 seconds, then twice as far each press in a row the same way, up to 30.
    fn jump(&mut self, playback: &Playback, forward: bool) {
        let status = playback.status();
        let now = Instant::now();
        let seeing = self.focus == Focus::Bar && self.controls && self.thumbs.is_some();
        let step = match self.steps {
            Some((direction, step, at)) if direction == forward && now - at < Duration::from_secs(if step <= 10 { 2 } else { 5 }) => {
                let next = JUMP_LADDER.iter().copied().find(|rung| *rung > step).unwrap_or(step);
                if seeing { next } else { next.min(JUMP_BLIND_MAX) }
            }
            _ => JUMP_LADDER[0],
        };
        self.steps = Some((forward, step, now));
        let base = self.jump.map_or(status.position, |jump| jump.to);
        let step = Duration::from_secs(step);
        let to = if forward {
            (base + step).min(status.duration.saturating_sub(Duration::from_secs(1)))
        } else {
            base.saturating_sub(step)
        };
        self.jump = Some(Jump { to, at: now });
    }

    /// OK on the skip button. Over the end it plays the next episode, or leaves after the last (`Nav::Back`), the
    /// episode counted as watched to its end.
    fn skip(&mut self, worker: &Worker, playback: &Playback) -> Nav {
        let ending = self.skip_active.and_then(|index| self.skips.get(index)).is_some_and(Self::ending);
        if ending {
            let end = playback.status().duration;
            match self.next.clone() {
                Some(Some(card)) => {
                    self.change(worker, card, end);
                    return Nav::Stay;
                }
                Some(None) => return self.leave_at(worker, end),
                None => {}
            }
        }
        if let Some(event) = self.skip_active.and_then(|index| self.skips.get(index)) {
            // a few seconds before the end, where there are that many, so nothing after it is lost
            let to = if event.end - event.start > 5.0 { event.end - 5.0 } else { event.end };
            playback.seek(Duration::from_secs_f32(to.max(0.0)));
            self.jump = None;
        }
        self.skip_dismissed = self.skip_active;
        self.controls = false;
        self.set_focus(Focus::Button(Control::PlayPause));
        Nav::Stay
    }

    fn activate(&mut self, control: Control, playback: &Playback, env: &mut Env) {
        match control {
            Control::Info => self.info = true,
            Control::Watched => {
                self.send(env.worker, Command::MarkWatched { id: self.card.id.clone(), season: false });
                self.ask_adjacent(env.worker, true);
            }
            Control::ThumbDown | Control::ThumbUp => {
                let thumb = if control == Control::ThumbUp { "up" } else { "down" };
                let rating = if self.rating.as_deref() == Some(thumb) { None } else { Some(thumb.to_string()) };
                self.rating = rating.clone();
                self.send(env.worker, Command::RateEpisode { id: self.card.id.clone(), rating });
            }
            Control::Previous => {
                if playback.status().position > RESTART_AFTER {
                    playback.seek(Duration::ZERO);
                    playback.resume();
                    self.jump = None;
                } else {
                    self.ask_adjacent(env.worker, false);
                }
            }
            Control::PlayPause => self.toggle(playback),
            Control::Next => self.ask_adjacent(env.worker, true),
            Control::Subtitles => self.open_menu(MenuKind::Subtitles, env.state),
            Control::Audio => self.open_menu(MenuKind::Audio, env.state),
            Control::Quality => self.open_menu(MenuKind::Quality, env.state),
        }
    }

    // ---- menus ----

    fn open_menu(&mut self, kind: MenuKind, state: &State) {
        let Some(tracks) = self.playback.as_ref().map(|playback| &playback.tracks) else { return };
        let (title, options, current): (&'static str, Vec<(String, String)>, String) = match kind {
            MenuKind::Subtitles => {
                let mut options = vec![("Desactivados".to_string(), "off".to_string())];
                options.extend(tracks.subtitles.iter().map(|code| (language_name(state, code, true), code.clone())));
                ("Subtítulos", options, tracks.subtitle.clone().unwrap_or_else(|| "off".into()))
            }
            MenuKind::Audio => {
                let options = tracks.audios.iter().map(|(code, _)| (language_name(state, code, false), code.clone())).collect();
                ("Audio", options, tracks.audio.clone())
            }
            MenuKind::Quality => {
                let options = QUALITIES.iter().map(|(label, value)| (label.to_string(), value.to_string())).collect();
                let current = &state.settings.video_quality;
                let current = if QUALITIES.iter().any(|(_, value)| value == current) { current.clone() } else { "adaptive".into() };
                ("Calidad", options, current)
            }
        };
        let selected = options.iter().position(|(_, value)| *value == current).unwrap_or(0);
        self.menu = Some(Menu { kind, title, options, selected, focus: selected });
    }

    fn menu_key(&mut self, key: Key, env: &mut Env) {
        let Some(menu) = self.menu.as_mut() else { return };
        match key {
            Key::Up => menu.focus = menu.focus.saturating_sub(1),
            Key::Down => menu.focus = (menu.focus + 1).min(menu.options.len().saturating_sub(1)),
            Key::Ok => {
                let (kind, value, changed) = (menu.kind, menu.options[menu.focus].1.clone(), menu.focus != menu.selected);
                self.menu = None;
                if changed {
                    self.choose(kind, value, env);
                }
            }
            Key::Back | Key::Left | Key::Right => self.menu = None,
            _ => {}
        }
    }

    fn choose(&mut self, kind: MenuKind, value: String, env: &mut Env) {
        match kind {
            MenuKind::Subtitles => {
                self.wanted.subtitles = Some(value);
                self.reopen(env.worker, None);
            }
            MenuKind::Audio => {
                let id = self
                    .playback
                    .as_ref()
                    .and_then(|playback| playback.tracks.audios.iter().find(|(code, _)| *code == value))
                    .map(|(_, id)| id.clone());
                self.wanted.audio = Some(value);
                self.reopen(env.worker, id);
            }
            MenuKind::Quality => {
                let height = max_height(&value);
                env.state.settings.video_quality = value;
                env.state.settings.save();
                if height != self.wanted.max_height {
                    self.wanted.max_height = height;
                    self.reopen(env.worker, None);
                }
            }
        }
    }

    // ---- drawing ----

    fn title(&self) -> String {
        match &self.card.episode {
            Some(episode) if !episode.number.is_empty() => {
                format!("{} - E{} - {}", self.card.title, episode.number, episode.name)
            }
            Some(episode) if !episode.name.is_empty() => format!("{} - {}", self.card.title, episode.name),
            _ => self.card.title.clone(),
        }
    }

    /// Before the picture: the episode's still across the screen, drifting slowly closer, its name where the
    /// controls' title will be and a spinner (or why it can't play). `k` fades it all, over the video once it starts.
    fn draw_loading(&self, ui: &mut Ui, failed: Option<&str>, k: f32) {
        let screen = Rect::new(0.0, 0.0, SCREEN_W, SCREEN_H);
        ui.gfx.rect(screen, 0.0, Color(0.0, 0.0, 0.0, k));
        // the big still once it's here; the card's own, already loaded, until then
        let big = self.card.backdrop.as_deref().and_then(|url| ui.images.get(ui.worker, url, BACKDROP_WIDTH));
        let small = self.card.image.as_deref().and_then(|url| ui.images.get(ui.worker, url, CARD_PICTURE));
        if let Some(texture) = big.or(small) {
            let t = (self.opened_at.elapsed().as_secs_f32() / 12.0).min(1.0);
            let zoom = 1.0 + 0.05 * (1.0 - (1.0 - t) * (1.0 - t));
            ui.gfx.image(&texture, screen.scaled(zoom), 0.0, 0.62 * k);
        }
        fade_bottom(ui, 0.85, k);
        ui.gfx.text(Font::SemiBold, 42.0, SIDE, TITLE_Y, &self.title(), TEXT.alpha(k), Some(SCREEN_W - 2.0 * SIDE));
        if let Some(message) = failed {
            match self.stopped_at {
                Some(at) => {
                    ui.gfx.text(Font::SemiBold, 28.0, SIDE, BAR_Y - 10.0, "Se detuvo la reproducción", TEXT, None);
                    let hint = format!("OK para seguir desde {} · Atrás para volver", clock(at));
                    ui.gfx.text(Font::Regular, 24.0, SIDE, BAR_Y + 34.0, &hint, MUTED, None);
                }
                None => {
                    ui.gfx.text(Font::SemiBold, 28.0, SIDE, BAR_Y - 10.0, "No se pudo reproducir", TEXT, None);
                    ui.gfx.text(Font::Regular, 24.0, SIDE, BAR_Y + 34.0, message, MUTED, Some(SCREEN_W - 2.0 * SIDE));
                    ui.gfx.text(Font::Regular, 22.0, SIDE, BAR_Y + 80.0, "Atrás para volver", DIM, None);
                }
            }
            return;
        }
        let status = if self.wanted.from > Duration::from_secs(5) {
            format!("Continuando desde {}", clock(self.wanted.from))
        } else {
            "Cargando…".to_string()
        };
        ui.gfx.text(Font::Regular, 26.0, SIDE, TITLE_Y + 62.0, &status, MUTED.alpha(k), None);
        spinner(ui, SCREEN_W / 2.0, SCREEN_H / 2.0, k);
    }

    /// The seek thumbnail for where the bar is being moved to (the one shown last while it decodes), with the
    /// ones beside it asked for ahead.
    fn scrub_thumb(&mut self, ui: &mut Ui) -> Option<crate::gfx::Texture> {
        let jump = self.jump.filter(|_| self.focus == Focus::Bar && self.controls)?;
        let (key, times) = self.thumbs.as_ref()?;
        let ms = jump.to.as_millis().min(u32::MAX as u128) as u32;
        let index = times.partition_point(|time| *time <= ms).saturating_sub(1);
        let link = |index: usize| format!("bif:{key}:{index}");
        for near in [index + 1, index.wrapping_sub(1), index + 2, index.wrapping_sub(2)] {
            if near < times.len() {
                ui.images.prefetch(ui.worker, &link(near), THUMB_W as u32);
            }
        }
        match ui.images.get(ui.worker, &link(index), THUMB_W as u32) {
            Some(texture) => {
                self.thumb_shown = Some(index);
                Some(texture)
            }
            None => self.thumb_shown.and_then(|shown| ui.images.get(ui.worker, &link(shown), THUMB_W as u32)),
        }
    }

    fn draw_controls(&self, ui: &mut Ui, position: Duration, duration: Duration, paused: bool, k: f32, thumb: Option<crate::gfx::Texture>) {
        fade_bottom(ui, 0.8, k);
        let shown = self.jump.map_or(position, |jump| jump.to);
        let times = format!("{} / {}", clock(shown), clock(duration));
        let times_w = ui.gfx.measure(Font::Regular, 24.0, &times);
        let title_w = SCREEN_W - 2.0 * SIDE - times_w - 48.0;
        let scrubbing = self.focus == Focus::Bar && self.jump.is_some();
        // the time over the knob takes the title's place while the bar is moved
        if !scrubbing {
            ui.gfx.text(Font::SemiBold, 42.0, SIDE, TITLE_Y, &self.title(), TEXT.alpha(k), Some(title_w));
        }
        ui.gfx.text(Font::Regular, 24.0, SCREEN_W - SIDE - times_w, TIMES_Y, &times, Color(0.8, 0.8, 0.8, k), None);

        // the bar: white over a translucent track
        let width = SCREEN_W - 2.0 * SIDE;
        let share = if duration.is_zero() { 0.0 } else { (shown.as_secs_f32() / duration.as_secs_f32()).clamp(0.0, 1.0) };
        ui.gfx.rect(Rect::new(SIDE, BAR_Y, width, BAR_H), BAR_H / 2.0, Color(1.0, 1.0, 1.0, 0.3 * k));
        ui.gfx.rect(Rect::new(SIDE, BAR_Y, (width * share).max(BAR_H), BAR_H), BAR_H / 2.0, Color(0.96, 0.96, 0.97, 0.85 * k));
        let knob_x = SIDE + width * share;
        if self.focus == Focus::Bar && self.controls {
            let d = if self.jump.is_some() { 40.0 } else { 26.0 };
            let cy = BAR_Y + BAR_H / 2.0;
            ui.gfx.rect(Rect::new(knob_x - d / 2.0, cy - d / 2.0 + 3.0, d, d), d / 2.0, Color(0.0, 0.0, 0.0, 0.35 * k));
            ui.gfx.rect(Rect::new(knob_x - d / 2.0, cy - d / 2.0, d, d), d / 2.0, Color(1.0, 1.0, 1.0, k));
            if scrubbing && let Some(texture) = thumb {
                let x = (knob_x - THUMB_W / 2.0).clamp(SIDE, SCREEN_W - SIDE - THUMB_W);
                let r = Rect::new(x, BAR_Y - 86.0 - THUMB_H, THUMB_W, THUMB_H);
                ui.gfx.rect(Rect::new(r.x - 2.0, r.y + 4.0, r.w + 4.0, r.h + 6.0), 16.0, Color(0.0, 0.0, 0.0, 0.35 * k));
                ui.gfx.image(&texture, r, 12.0, k);
                ui.gfx.ring(r, 12.0, 1.5, Color(1.0, 1.0, 1.0, 0.30 * k));
            }
            if scrubbing {
                let label = clock(shown);
                let w = ui.gfx.measure(Font::SemiBold, 26.0, &label) + 36.0;
                let x = (knob_x - w / 2.0).clamp(SIDE, SCREEN_W - SIDE - w);
                let r = Rect::new(x, BAR_Y - 74.0, w, 50.0);
                ui.gfx.rect(r, 25.0, Color(0.141, 0.141, 0.165, 0.92 * k));
                ui.gfx.text(Font::SemiBold, 26.0, r.x + 18.0, r.y + 9.0, &label, TEXT.alpha(k), None);
            }
        }

        // the buttons
        for (control, cx) in self.layout() {
            let focused = self.controls && self.focus == Focus::Button(control);
            let (icon, selected) = match control {
                Control::Info => (Icon::Info, false),
                Control::Watched => (Icon::CheckCircle, false),
                Control::ThumbDown if self.rating.as_deref() == Some("down") => (Icon::ThumbsDown, true),
                Control::ThumbDown => (Icon::ThumbsDownOutline, false),
                Control::ThumbUp if self.rating.as_deref() == Some("up") => (Icon::ThumbsUp, true),
                Control::ThumbUp => (Icon::ThumbsUpOutline, false),
                Control::Previous => (Icon::PrevEpisode, false),
                Control::PlayPause if paused => (Icon::Play, false),
                Control::PlayPause => (Icon::Pause, false),
                Control::Next => (Icon::NextEpisode, false),
                Control::Subtitles => (Icon::Bubble, false),
                Control::Audio => (Icon::Speaker, false),
                Control::Quality => (Icon::Gear, false),
            };
            draw_button(ui, cx, icon, focused, selected, k);
            // its name, like tvOS: under it once the focus rests there
            if focused && self.focus_at.elapsed() >= NAME_AFTER && self.menu.is_none() {
                let name = control.name(paused);
                let w = ui.gfx.measure(Font::SemiBold, 20.0, name);
                let x = (cx - w / 2.0).clamp(SIDE - 20.0, SCREEN_W - SIDE + 20.0 - w);
                ui.gfx.text(Font::SemiBold, 20.0, x + 1.0, ROW_Y + BUTTON + 8.0, name, Color(0.0, 0.0, 0.0, 0.6 * k), None);
                ui.gfx.text(Font::SemiBold, 20.0, x, ROW_Y + BUTTON + 7.0, name, TEXT.alpha(0.92 * k), None);
            }
        }
    }

    /// A jump with the controls hidden: where it goes, top left.
    fn draw_jump(&self, ui: &mut Ui, forward: bool) {
        let Some(jump) = self.jump else { return };
        let label = clock(jump.to);
        let w = 18.0 + 34.0 + 12.0 + ui.gfx.measure(Font::Bold, 34.0, &label) + 22.0;
        let r = Rect::new(30.0, 30.0, w, 60.0);
        ui.gfx.rect(r, 30.0, Color(0.29, 0.29, 0.29, 0.7));
        ui.gfx.icon(if forward { Icon::Skip } else { Icon::Rewind }, r.x + 18.0, r.y + 13.0, 34.0, TEXT);
        ui.gfx.text(Font::Bold, 34.0, r.x + 18.0 + 34.0 + 12.0, r.y + 9.0, &label, TEXT, None);
    }

    fn draw_skip(&self, ui: &mut Ui, event: &SkipEvent) {
        let (label, icon) = self.skip_look(event);
        let w = 44.0 + 32.0 + 14.0 + ui.gfx.measure(Font::Bold, 30.0, label) + 44.0;
        // over the bottom edge, or above the title while the controls show
        let low = SCREEN_H - 60.0 - SKIP_H;
        let high = TITLE_Y - 36.0 - SKIP_H;
        let y = low + (high - low) * self.shown;
        let focused = self.focus == Focus::Skip;
        let r = Rect::new(SCREEN_W - PAD - w, y, w, SKIP_H);
        let r = if focused { r.scaled(1.08) } else { r };
        // a soft shadow under it
        for (grow, drop, alpha) in [(6.0, 10.0, 0.08), (3.0, 6.0, 0.10), (0.0, 3.0, 0.12)] {
            ui.gfx.rect(Rect::new(r.x - grow, r.y + drop - grow, r.w + 2.0 * grow, r.h + 2.0 * grow), r.h / 2.0 + grow, Color(0.0, 0.0, 0.0, alpha));
        }
        ui.gfx.rect(r, r.h / 2.0, if focused { Color(1.0, 1.0, 1.0, 1.0) } else { Color(0.96, 0.96, 0.97, 0.92) });
        let size = if focused { 32.0 * 1.08 } else { 32.0 };
        let text_size = if focused { 30.0 * 1.08 } else { 30.0 };
        let x = r.x + r.h * 0.52;
        ui.gfx.icon(icon, x, r.y + (r.h - size) / 2.0, size, BG);
        ui.gfx.text(Font::Bold, text_size, x + size + 14.0, r.y + r.h / 2.0 - text_size * 0.62, label, BG, None);
    }

    fn draw_menu(&self, ui: &mut Ui, menu: &Menu) {
        let control = match menu.kind {
            MenuKind::Subtitles => Control::Subtitles,
            MenuKind::Audio => Control::Audio,
            MenuKind::Quality => Control::Quality,
        };
        let cx = self.layout().iter().find(|(c, _)| *c == control).map_or(SCREEN_W - SIDE, |(_, x)| *x);
        let row_h = 60.0;
        let visible = menu.options.len().min(6);
        let first = (menu.focus + 1).saturating_sub(visible).min(menu.options.len() - visible);
        let widest = menu.options.iter().map(|(label, _)| ui.gfx.measure(Font::Regular, 27.0, label)).fold(0.0, f32::max);
        let w = (widest + 110.0).clamp(360.0, 560.0);
        let h = 12.0 + 48.0 + visible as f32 * row_h + 12.0;
        let x = (cx - w / 2.0).clamp(PAD, SCREEN_W - PAD - w);
        let panel = Rect::new(x, ROW_Y - 20.0 - h, w, h);
        ui.gfx.rect(Rect::new(panel.x, panel.y + 8.0, panel.w, panel.h), 24.0, Color(0.0, 0.0, 0.0, 0.35));
        ui.gfx.rect(panel.inset(-1.0), 25.0, Color(1.0, 1.0, 1.0, 0.10));
        ui.gfx.rect(panel, 24.0, Color(0.141, 0.141, 0.165, 0.96));
        ui.gfx.text(Font::SemiBold, 20.0, panel.x + 30.0, panel.y + 22.0, &menu.title.to_uppercase(), DIM, None);
        for (slot, index) in (first..first + visible).enumerate() {
            let r = Rect::new(panel.x + 12.0, panel.y + 60.0 + slot as f32 * row_h, panel.w - 24.0, row_h - 4.0);
            let focused = index == menu.focus;
            if focused {
                ui.gfx.rect(r, 14.0, TEXT);
            }
            let ink = if focused { BG } else { TEXT };
            ui.gfx.text(Font::Regular, 27.0, r.x + 18.0, r.y + 12.0, &menu.options[index].0, ink, Some(r.w - 70.0));
            if index == menu.selected {
                ui.gfx.icon(Icon::Check, r.x + r.w - 44.0, r.y + 16.0, 24.0, ink);
            }
        }
    }

    /// What's playing, in a sheet from the bottom.
    fn draw_info(&self, ui: &mut Ui) {
        ui.gfx.rect(Rect::new(0.0, 0.0, SCREEN_W, SCREEN_H), 0.0, Color(0.0, 0.0, 0.0, 0.45));
        let x = PAD * 2.0;
        let width = SCREEN_W - 4.0 * PAD;
        let named = self.card.episode.as_ref().is_some_and(|episode| !episode.name.is_empty());
        let lines = ui.gfx.wrap(Font::Regular, 26.0, &self.card.description, width, 4).len();
        let h = 52.0
            + if self.card.episode.is_some() { 44.0 } else { 0.0 }
            + 66.0
            + if named { 56.0 } else { 0.0 }
            + lines as f32 * 38.0
            + 64.0;
        let sheet = Rect::new(0.0, SCREEN_H - h, SCREEN_W, h + 40.0);
        ui.gfx.rect(sheet, 30.0, Color(0.11, 0.11, 0.13, 1.0));
        let mut y = sheet.y + 52.0;
        if let Some(episode) = &self.card.episode {
            let mut facts = Vec::new();
            if episode.season > 0 {
                facts.push(format!("Temporada {}", episode.season));
            }
            if !episode.number.is_empty() {
                facts.push(format!("Episodio {}", episode.number));
            }
            if episode.duration_secs > 0 {
                facts.push(format!("{} min", (episode.duration_secs + 30) / 60));
            }
            ui.gfx.text(Font::SemiBold, 22.0, x, y, &facts.join("  ·  ").to_uppercase(), DIM, Some(width));
            y += 44.0;
        }
        ui.gfx.text(Font::Bold, 46.0, x, y, &self.card.title, TEXT, Some(width));
        y += 66.0;
        if let Some(episode) = self.card.episode.as_ref().filter(|episode| !episode.name.is_empty()) {
            ui.gfx.text(Font::SemiBold, 30.0, x, y, &episode.name, TEXT, Some(width));
            y += 56.0;
        }
        if !self.card.description.is_empty() {
            ui.gfx.paragraph(Font::Regular, 26.0, x, y, &self.card.description, MUTED, width, 4, 38.0);
        }
    }

    fn draw_notice(&mut self, ui: &mut Ui) {
        let Some((text, at)) = &self.notice else { return };
        if at.elapsed() > Duration::from_millis(3000) {
            self.notice = None;
            return;
        }
        let w = ui.gfx.measure(Font::Regular, 24.0, text) + 64.0;
        let r = Rect::new((SCREEN_W - w) / 2.0, 48.0, w, 64.0);
        ui.gfx.rect(r, 32.0, SURFACE.alpha(0.96));
        ui.gfx.text(Font::Regular, 24.0, r.x + 32.0, r.y + 17.0, text, TEXT, None);
    }
}

impl Drop for PlayerPage {
    fn drop(&mut self) {
        // the playback stops with the last reference to it (this page's)
        self.playback = None;
        platform::want_video_window(false);
    }
}

impl Page for PlayerPage {
    fn shown(&mut self, env: &mut Env) {
        if self.requested.is_some() || self.playback.is_some() {
            return;
        }
        if env.state.account.languages.is_none() {
            self.send(env.worker, Command::Languages);
        }
        self.previews = env.state.settings.previews;
        let profile = env.state.account.profile();
        self.wanted.audio = env.state.account.audio();
        self.wanted.subtitles = profile.map(|profile| profile.subtitle_language.clone()).filter(|s| !s.is_empty());
        self.wanted.max_height = max_height(&env.state.settings.video_quality);
        if self.card.kind == Kind::Episode {
            self.send(env.worker, Command::EpisodeRating(self.card.id.clone()));
        }
        let id = self.card.id.clone();
        self.play(env.worker, id);
    }

    fn event(&mut self, event: &Event, env: &mut Env) {
        match event {
            Event::Playback(id, playback) if self.requested.as_ref() == Some(id) && self.playback.is_none() => {
                self.playback = Some(playback.clone());
                self.saved_at = Instant::now();
                self.activity = Instant::now();
                self.prefetch_next(env.worker);
                let version = playback.tracks.id.clone();
                if self.skips_for != version {
                    self.skips.clear();
                    self.skips_for = version.clone();
                    self.send(env.worker, Command::SkipEvents(version));
                }
            }
            Event::PlaybackFailed(id, message) if self.requested.as_ref() == Some(id) => {
                self.failed = Some(message.clone());
                platform::want_video_window(false);
            }
            Event::SkipEvents(id, events) if *id == self.skips_for => self.skips = events.clone(),
            Event::Bif { key, times } if self.thumbs_asked.as_ref() == Some(key) => {
                self.thumbs = Some((key.clone(), times.clone()));
            }
            Event::EpisodeRating(id, rating) if *id == self.card.id => self.rating = rating.clone(),
            Event::Adjacent { id, forward, card } if *id == self.card.id => {
                // the last episode can answer with itself
                let card = card.clone().filter(|card| card.id != self.card.id);
                if *forward {
                    self.next = Some(card.clone());
                    // looked up ahead, so it starts sooner when this one ends
                    if let Some(next) = &card {
                        self.send(env.worker, Command::Warm { id: next.id.clone(), audio: self.wanted.audio.clone() });
                    }
                }
                if self.adjacent == Some(*forward) {
                    self.adjacent = None;
                    self.go(env.worker, *forward, card);
                }
            }
            _ => {}
        }
    }

    fn key(&mut self, key: Key, env: &mut Env) -> Nav {
        self.activity = Instant::now();
        if self.info {
            if matches!(key, Key::Back | Key::Ok) {
                self.info = false;
            }
            return Nav::Stay;
        }
        if self.menu.is_some() {
            self.menu_key(key, env);
            return Nav::Stay;
        }
        // stopped after playing: OK (or Play) opens it again where it was
        if self.failed.is_some()
            && let Some(at) = self.stopped_at
            && matches!(key, Key::Ok | Key::Other(207) | Key::Other(164))
        {
            let id = self.playback.as_ref().map_or_else(|| self.card.id.clone(), |playback| playback.tracks.id.clone());
            self.stopped_at = None;
            self.wanted.from = at;
            self.play(env.worker, id);
            return Nav::Stay;
        }
        let Some(playback) = self.playback.clone().filter(|_| self.started && self.failed.is_none()) else {
            return if key == Key::Back { self.leave(env.worker) } else { Nav::Stay };
        };
        // the remote's media keys, where it has them (evdev codes)
        match key {
            Key::Other(164) => self.toggle(&playback),
            Key::Other(207) => playback.resume(),
            Key::Other(119) => playback.pause(),
            Key::Other(208) => self.jump(&playback, true),
            Key::Other(168) => self.jump(&playback, false),
            Key::Other(128) => return self.leave(env.worker),
            _ => {}
        }
        if let Key::Other(_) = key {
            return Nav::Stay;
        }

        if self.focus == Focus::Skip {
            match key {
                Key::Ok => return self.skip(env.worker, &playback),
                Key::Back => {
                    self.skip_dismissed = self.skip_active;
                    self.set_focus(Focus::Button(self.last_button));
                }
                Key::Up => {}
                _ => {
                    self.set_focus(Focus::Button(self.last_button));
                    self.show_controls();
                }
            }
            return Nav::Stay;
        }
        if !self.controls {
            match key {
                Key::Left => self.jump(&playback, false),
                Key::Right => self.jump(&playback, true),
                Key::Back => return self.leave(env.worker),
                _ => self.show_controls(),
            }
            return Nav::Stay;
        }
        match self.focus {
            Focus::Bar => match key {
                Key::Left => self.jump(&playback, false),
                Key::Right => self.jump(&playback, true),
                Key::Ok => match self.jump.take() {
                    Some(jump) => playback.seek(jump.to),
                    None => self.toggle(&playback),
                },
                Key::Up if self.skip_active.is_some() => self.set_focus(Focus::Skip),
                Key::Down => self.set_focus(Focus::Button(self.last_button)),
                Key::Back => self.controls = false,
                _ => {}
            },
            Focus::Button(control) => match key {
                Key::Left | Key::Right => {
                    let order = self.order();
                    let index = order.iter().position(|c| *c == control).unwrap_or(0);
                    let next = if key == Key::Left { index.saturating_sub(1) } else { (index + 1).min(order.len() - 1) };
                    self.set_focus(Focus::Button(order[next]));
                }
                Key::Up => self.set_focus(Focus::Bar),
                Key::Ok => self.activate(control, &playback, env),
                Key::Back => self.controls = false,
                _ => {}
            },
            Focus::Skip => {}
        }
        Nav::Stay
    }

    fn draw(&mut self, ui: &mut Ui, _state: &State, _focused: bool) {
        if let Some(message) = self.failed.clone() {
            self.draw_loading(ui, Some(&message), 1.0);
            return;
        }
        let Some(playback) = self.playback.clone() else {
            self.draw_loading(ui, None, 1.0);
            self.draw_notice(ui);
            return;
        };
        if let Some(jump) = self.jump
            && jump.at.elapsed() >= JUMP_SETTLE
        {
            playback.seek(jump.to);
            self.jump = None;
        }
        let status = playback.status();
        match &status.state {
            PlayState::Failed(message) => {
                eprintln!("crunchy: playback stopped: {message}");
                if self.started {
                    let at = self.jump.map_or(status.position, |jump| jump.to);
                    self.stopped_at = Some(at);
                    self.save(ui.worker, at);
                }
                self.failed = Some(message.clone());
                platform::want_video_window(false);
                return;
            }
            PlayState::Playing if !self.started => {
                self.started = true;
                self.started_at = Some(Instant::now());
                self.playing_since = Some(Instant::now());
                self.ever_played = true;
                eprintln!("crunchy: playing {:.1} s after it was asked for", self.opened_at.elapsed().as_secs_f32());
            }
            _ => {}
        }
        if !self.started {
            self.draw_loading(ui, None, 1.0);
            return;
        }
        let paused = status.state == PlayState::Paused;
        if status.state == PlayState::Playing && self.saved_at.elapsed() >= SAVE_EVERY {
            self.save(ui.worker, status.position);
        }
        // the end: the next episode a moment later, or back to where the player was opened
        if status.state == PlayState::Ended {
            if self.ended_at.is_none() {
                self.ended_at = Some(Instant::now());
                self.save(ui.worker, status.duration);
            }
            if self.ended_at.is_some_and(|at| at.elapsed() >= NEXT_AFTER) && self.adjacent.is_none() && !self.done {
                if self.card.kind == Kind::Episode && !self.demo {
                    self.ask_adjacent(ui.worker, true);
                } else {
                    self.done = true;
                }
            }
        }

        // a skip button, focused when it comes up
        let at = status.position.as_secs_f32();
        let active = self
            .skips
            .iter()
            .position(|event| {
                // the next episode is offered through the whole of the credits; a skip, its first seconds
                let lasts = if Self::ending(event) { event.end - event.start } else { SKIP_FOR.min(event.end - event.start) };
                at > 0.0 && event.start <= at && at <= event.start + lasts
            })
            .filter(|index| Some(*index) != self.skip_dismissed);
        if active != self.skip_active {
            // it takes the focus, unless a menu, the sheet or the bar is in use
            if active.is_some() && self.menu.is_none() && !self.info && self.jump.is_none() {
                self.controls = false;
                self.set_focus(Focus::Skip);
            } else if active.is_none() && self.focus == Focus::Skip {
                self.set_focus(Focus::Button(self.last_button));
            }
            self.skip_active = active;
        }

        if self.controls && !paused && self.menu.is_none() && self.jump.is_none() && self.activity.elapsed() >= HIDE_AFTER {
            self.controls = false;
        }
        // the information sheet covers them
        let target = if self.controls && !self.info { 1.0 } else { 0.0 };
        self.shown += (target - self.shown) * (1.0 - (-ui.dt * 9.0).exp());
        if (target - self.shown).abs() < 0.01 {
            self.shown = target;
        }

        // the seek thumbnails, asked for once the picture has played a moment
        if self.previews
            && let Some(url) = playback.bif.clone()
            && self.playing_since.is_some_and(|at| at.elapsed() >= THUMBS_AFTER)
            && self.thumbs_asked.as_ref() != Some(&playback.tracks.id)
        {
            let key = playback.tracks.id.clone();
            self.thumbs_asked = Some(key.clone());
            self.thumbs = None;
            self.send(ui.worker, Command::Bif { key, url });
        }
        let thumb = self.scrub_thumb(ui);

        if self.demo {
            draw_demo_picture(ui);
        }
        if self.shown > 0.0 {
            self.draw_controls(ui, status.position, status.duration, paused, self.shown, thumb);
        }
        // waiting for data (after a seek, or a slow network): the spinner, once it isn't just a blink
        if status.state == PlayState::Loading {
            let since = *self.loading_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= SPINNER_AFTER {
                let k = ((since.elapsed() - SPINNER_AFTER).as_secs_f32() / 0.25).min(1.0);
                spinner(ui, SCREEN_W / 2.0, SCREEN_H / 2.0, k);
            }
        } else {
            self.loading_since = None;
        }
        if !self.controls
            && let Some((forward, _, _)) = self.steps.filter(|_| self.jump.is_some())
        {
            self.draw_jump(ui, forward);
        }
        if let Some(event) = self.skip_active.and_then(|index| self.skips.get(index)).filter(|_| !self.info) {
            self.draw_skip(ui, event);
        }
        if let Some(menu) = &self.menu {
            self.draw_menu(ui, menu);
        }
        if self.info {
            self.draw_info(ui);
        }
        // the still fades away over the first frames instead of cutting to them
        if let Some(at) = self.started_at {
            let t = at.elapsed().as_secs_f32() / STILL_FADE.as_secs_f32();
            if t < 1.0 {
                let k = 1.0 - t;
                self.draw_loading(ui, None, k * k * (3.0 - 2.0 * k));
            } else {
                self.started_at = None;
            }
        }
        self.draw_notice(ui);
    }

    fn transparent(&self) -> bool {
        self.started && self.failed.is_none() && self.playback.is_some()
    }

    fn played(&self) -> bool {
        self.ever_played && !self.demo
    }

    fn finished(&self) -> bool {
        self.done
    }
}

/// A round button on the row, centred on `cx`: glass, or white with a dark icon when focused.
fn draw_button(ui: &mut Ui, cx: f32, icon: Icon, focused: bool, selected: bool, k: f32) {
    let cy = ROW_Y + BUTTON / 2.0;
    let d = if focused { BUTTON * 1.08 } else { BUTTON };
    let (fill, ink) = if focused {
        (TEXT.alpha(k), BG.alpha(k))
    } else if selected {
        (Color(1.0, 1.0, 1.0, 0.34 * k), TEXT.alpha(k))
    } else {
        (Color(1.0, 1.0, 1.0, 0.18 * k), TEXT.alpha(k))
    };
    ui.gfx.rect(Rect::new(cx - d / 2.0, cy - d / 2.0, d, d), d / 2.0, fill);
    ui.gfx.icon_centered(icon, cx, cy, d * 0.42, ink);
}

/// The fade under the controls: clear, to 42% black at 40% of its height, to `darkest` at the bottom.
/// A ring of dots, one bright and the ones behind it dimmer, going round (the TV's own "loading").
fn spinner(ui: &mut Ui, cx: f32, cy: f32, k: f32) {
    const DOTS: usize = 12;
    let head = (ui.t * 1.2).fract() * DOTS as f32;
    for index in 0..DOTS {
        let angle = index as f32 / DOTS as f32 * std::f32::consts::TAU;
        let behind = (head - index as f32).rem_euclid(DOTS as f32) / DOTS as f32;
        let alpha = 0.18 + 0.82 * (1.0 - behind).powi(2);
        let (x, y) = (cx + 30.0 * angle.sin(), cy - 30.0 * angle.cos());
        ui.gfx.rect(Rect::new(x - 4.5, y - 4.5, 9.0, 9.0), 4.5, Color(1.0, 1.0, 1.0, alpha * 0.9 * k));
    }
}

fn fade_bottom(ui: &mut Ui, darkest: f32, k: f32) {
    // the two stretches meet on a whole pixel: overlapping, they'd draw a darker line
    let split = FADE_TOP + ((SCREEN_H - FADE_TOP) * 0.4).round();
    let middle = Color(0.0, 0.0, 0.0, 0.42 * k);
    ui.gfx.gradient(Rect::new(0.0, FADE_TOP, SCREEN_W, split - FADE_TOP), Color(0.0, 0.0, 0.0, 0.0), middle, true);
    ui.gfx.gradient(Rect::new(0.0, split, SCREEN_W, SCREEN_H - split), middle, Color(0.0, 0.0, 0.0, darkest * k), true);
}

/// Something bright where the video would be, so the simulator shows the controls over a picture.
fn draw_demo_picture(ui: &mut Ui) {
    ui.gfx.gradient(Rect::new(0.0, 0.0, SCREEN_W, SCREEN_H), Color(0.36, 0.55, 0.62, 1.0), Color(0.62, 0.52, 0.38, 1.0), true);
    ui.gfx.rect(Rect::new(1180.0, 160.0, 420.0, 420.0), 210.0, Color(1.0, 0.93, 0.78, 0.55));
    ui.gfx.rect(Rect::new(0.0, 700.0, SCREEN_W, 380.0), 0.0, Color(0.20, 0.30, 0.22, 1.0));
}

/// The tallest picture a quality setting allows: Crunchyroll's streams top out at 1080p.
fn max_height(quality: &str) -> u32 {
    quality.trim_end_matches('p').parse().ok().filter(|height| *height < 1080).unwrap_or(1080)
}

/// A language's name ("Español (América Latina)") from Crunchyroll's lists, else from a few common ones, else its
/// code.
fn language_name(state: &State, code: &str, subtitles: bool) -> String {
    if let Some((audio, subs)) = &state.account.languages {
        let (first, second) = if subtitles { (subs, audio) } else { (audio, subs) };
        if let Some((_, name)) = first.iter().chain(second.iter()).find(|(c, _)| c == code) {
            return name.clone();
        }
    }
    let known = match code {
        "ja-JP" => "Japonés",
        "en-US" => "Inglés",
        "en-IN" => "Inglés (India)",
        "es-419" => "Español (América Latina)",
        "es-ES" => "Español (España)",
        "pt-BR" => "Portugués (Brasil)",
        "pt-PT" => "Portugués (Portugal)",
        "fr-FR" => "Francés",
        "de-DE" => "Alemán",
        "it-IT" => "Italiano",
        "ru-RU" => "Ruso",
        "ar-SA" | "ar-ME" => "Árabe",
        "hi-IN" => "Hindi",
        "zh-CN" => "Chino (Mandarín)",
        "ko-KR" => "Coreano",
        _ => return code.to_string(),
    };
    known.to_string()
}

/// 1:02:03 or 2:03.
fn clock(time: Duration) -> String {
    let seconds = time.as_secs();
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 { format!("{hours}:{minutes:02}:{seconds:02}") } else { format!("{minutes}:{seconds:02}") }
}

const SCREEN_W: f32 = crate::gfx::SCREEN_W;
const SCREEN_H: f32 = crate::gfx::SCREEN_H;
