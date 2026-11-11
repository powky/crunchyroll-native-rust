//! The player: the video fills the screen under a transparent frame, with its title, a progress bar and the time
//! over it while the controls show (a few seconds after a key, and while paused). OK pauses and plays, left and
//! right jump 10 seconds (pressed again, further), Back stops.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crunchy_core::model::Card;
use crunchy_player::State as PlayState;

use crate::gfx::{Color, Font, Icon, Rect};
use crate::platform::{self, Key};
use crate::player::Playback;

use super::theme::*;
use super::ui::Ui;
use super::worker::{Command, Event};
use super::{Env, Nav, Page, State};

const CONTROLS_FOR: Duration = Duration::from_secs(5);
const JUMP: Duration = Duration::from_secs(10);
/// A jump is made once the arrows rest this long.
const JUMP_SETTLE: Duration = Duration::from_millis(700);
const BAR_Y: f32 = 968.0;

pub struct PlayerPage {
    card: Card,
    from: Duration,
    requested: bool,
    playback: Option<Arc<Playback>>,
    failed: Option<String>,
    /// the picture has shown once (until then the page draws its own background)
    started: bool,
    /// when the controls were last asked for
    controls_at: Instant,
    /// a jump being chosen: where to, and when an arrow was last pressed
    jump: Option<(Duration, Instant)>,
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
        PlayerPage {
            card,
            from,
            requested: false,
            playback: None,
            failed: None,
            started: false,
            controls_at: Instant::now(),
            jump: None,
        }
    }

    fn show_controls(&mut self) {
        self.controls_at = Instant::now();
    }

    fn jump(&mut self, forward: bool) {
        let Some(playback) = &self.playback else { return };
        let status = playback.status();
        let base = self.jump.map_or(status.position, |(to, _)| to);
        let to = if forward { (base + JUMP).min(status.duration) } else { base.saturating_sub(JUMP) };
        self.jump = Some((to, Instant::now()));
        self.show_controls();
    }

    fn title_lines(&self) -> (String, String) {
        let detail = match &self.card.episode {
            Some(episode) if !episode.number.is_empty() => {
                format!("T{} · E{} · {}", episode.season, episode.number, episode.name)
            }
            Some(episode) => episode.name.clone(),
            None => String::new(),
        };
        (self.card.title.clone(), detail)
    }

    fn draw_waiting(&self, ui: &mut Ui, message: &str, failed: bool) {
        let (title, detail) = self.title_lines();
        let cx = SCREEN_W / 2.0;
        ui.text_centered(Font::Bold, 44.0, cx, 420.0, &title, TEXT);
        ui.text_centered(Font::Regular, 26.0, cx, 484.0, &detail, MUTED);
        if failed {
            ui.text_centered(Font::SemiBold, 26.0, cx, 580.0, message, TEXT);
            ui.text_centered(Font::Regular, 22.0, cx, 626.0, "Atrás para volver", DIM);
        } else {
            draw_dots(ui, cx, 600.0);
        }
    }

    fn draw_controls(&self, ui: &mut Ui, position: Duration, duration: Duration, paused: bool, loading: bool) {
        shade(ui, 0.0, 220.0, 0.65, false);
        shade(ui, 700.0, SCREEN_H, 0.8, true);
        let (title, detail) = self.title_lines();
        ui.gfx.text(Font::Bold, 36.0, PAD, 56.0, &title, TEXT, Some(SCREEN_W - 2.0 * PAD));
        ui.gfx.text(Font::Regular, 24.0, PAD, 108.0, &detail, MUTED, Some(SCREEN_W - 2.0 * PAD));

        let shown = self.jump.map_or(position, |(to, _)| to);
        let width = SCREEN_W - 2.0 * PAD;
        let share = if duration.is_zero() { 0.0 } else { (shown.as_secs_f32() / duration.as_secs_f32()).clamp(0.0, 1.0) };
        ui.gfx.rect(Rect::new(PAD, BAR_Y, width, 8.0), 4.0, VEIL_STRONG);
        ui.gfx.rect(Rect::new(PAD, BAR_Y, (width * share).max(8.0), 8.0), 4.0, ACCENT);
        let knob_x = PAD + width * share;
        ui.gfx.rect(Rect::new(knob_x - 11.0, BAR_Y - 7.0, 22.0, 22.0), 11.0, TEXT);
        ui.gfx.text(Font::SemiBold, 24.0, PAD, BAR_Y + 26.0, &clock(shown), TEXT, None);
        let left = format!("-{}", clock(duration.saturating_sub(shown)));
        let left_w = ui.gfx.measure(Font::SemiBold, 24.0, &left);
        ui.gfx.text(Font::SemiBold, 24.0, PAD + width - left_w, BAR_Y + 26.0, &left, MUTED, None);

        // play/pause (or the wait after a jump) over the bar's start
        let y = BAR_Y - 86.0;
        if loading {
            draw_dots(ui, PAD + 30.0, y + 18.0);
        } else if paused {
            ui.gfx.icon(Icon::Play, PAD, y, 40.0, TEXT);
        } else {
            ui.gfx.rect(Rect::new(PAD + 6.0, y, 10.0, 40.0), 3.0, TEXT);
            ui.gfx.rect(Rect::new(PAD + 26.0, y, 10.0, 40.0), 3.0, TEXT);
        }
        if let Some((to, _)) = self.jump {
            let delta = to.as_secs() as i64 - position.as_secs() as i64;
            let label = if delta >= 0 { format!("+{} s", delta) } else { format!("−{} s", -delta) };
            let w = ui.gfx.measure(Font::SemiBold, 24.0, &label) + 32.0;
            let x = (knob_x - w / 2.0).clamp(PAD, SCREEN_W - PAD - w);
            ui.gfx.rect(Rect::new(x, BAR_Y - 64.0, w, 44.0), 22.0, Color(0.0, 0.0, 0.0, 0.55));
            ui.gfx.text(Font::SemiBold, 24.0, x + 16.0, BAR_Y - 56.0, &label, TEXT, None);
        }
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
        if self.requested {
            return;
        }
        self.requested = true;
        platform::want_video_window(true);
        let subtitles = env.state.account.profile().map(|profile| profile.subtitle_language.clone()).filter(|s| !s.is_empty());
        env.worker.send(Command::Play { id: self.card.id.clone(), subtitles, from: self.from });
    }

    fn event(&mut self, event: &Event, _env: &mut Env) {
        match event {
            Event::Playback(id, playback) if *id == self.card.id && self.playback.is_none() => {
                self.playback = Some(playback.clone());
                self.show_controls();
            }
            Event::PlaybackFailed(id, message) if *id == self.card.id => {
                self.failed = Some(message.clone());
                platform::want_video_window(false);
            }
            _ => {}
        }
    }

    fn key(&mut self, key: Key, _env: &mut Env) -> Nav {
        if key == Key::Back {
            return Nav::Back;
        }
        let Some(playback) = self.playback.clone() else { return Nav::Stay };
        match key {
            Key::Ok => {
                if let Some((to, _)) = self.jump.take() {
                    // OK during a jump makes it at once
                    playback.seek(to);
                } else if playback.status().state == PlayState::Paused {
                    playback.resume();
                } else {
                    playback.pause();
                }
                self.show_controls();
            }
            Key::Left => self.jump(false),
            Key::Right => self.jump(true),
            _ => self.show_controls(),
        }
        Nav::Stay
    }

    fn draw(&mut self, ui: &mut Ui, _state: &State, _focused: bool) {
        if let Some(message) = &self.failed {
            self.draw_waiting(ui, &format!("No se pudo reproducir: {message}"), true);
            return;
        }
        let Some(playback) = self.playback.clone() else {
            self.draw_waiting(ui, "", false);
            return;
        };
        if let Some((to, at)) = self.jump
            && at.elapsed() >= JUMP_SETTLE
        {
            playback.seek(to);
            self.jump = None;
        }
        let status = playback.status();
        match &status.state {
            PlayState::Failed(message) => {
                self.failed = Some(message.clone());
                return;
            }
            PlayState::Playing => self.started = true,
            _ => {}
        }
        if !self.started {
            self.draw_waiting(ui, "", false);
            return;
        }
        let paused = status.state == PlayState::Paused;
        let loading = status.state == PlayState::Loading;
        if paused || loading || self.jump.is_some() || self.controls_at.elapsed() < CONTROLS_FOR {
            self.draw_controls(ui, status.position, status.duration, paused, loading);
        }
    }

    fn transparent(&self) -> bool {
        self.started && self.failed.is_none()
    }

    fn finished(&self) -> bool {
        self.playback.as_ref().is_some_and(|playback| playback.status().state == PlayState::Ended)
    }
}

/// A vertical fade to black over `top..bottom`, darkest at the bottom (or the top).
fn shade(ui: &mut Ui, top: f32, bottom: f32, darkest: f32, down: bool) {
    const STEPS: usize = 24;
    let step = (bottom - top) / STEPS as f32;
    for index in 0..STEPS {
        let t = (index as f32 + 0.5) / STEPS as f32;
        let alpha = darkest * if down { t * t } else { (1.0 - t) * (1.0 - t) };
        ui.gfx.rect(Rect::new(0.0, top + step * index as f32, SCREEN_W, step + 1.0), 0.0, Color(0.0, 0.0, 0.0, alpha));
    }
}

/// Three dots pulsing one after another, centered on `cx`.
fn draw_dots(ui: &mut Ui, cx: f32, y: f32) {
    for index in 0..3 {
        let phase = (ui.t * 2.4 - index as f32 * 0.35).sin() * 0.5 + 0.5;
        let x = cx - 30.0 + index as f32 * 30.0;
        ui.gfx.rect(Rect::new(x - 7.0, y - 7.0, 14.0, 14.0), 7.0, TEXT.alpha(0.3 + 0.7 * phase));
    }
}

/// 1:02:03 or 2:03.
fn clock(time: Duration) -> String {
    let seconds = time.as_secs();
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 { format!("{hours}:{minutes:02}:{seconds:02}") } else { format!("{minutes}:{seconds:02}") }
}

const SCREEN_W: f32 = crate::gfx::SCREEN_W;
const SCREEN_H: f32 = crate::gfx::SCREEN_H;
