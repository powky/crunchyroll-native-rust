# Changelog

## 1.0.4 — 2026-11-16

### Fixed

- **The TV's decoders are given back when an episode ends.** The player stopped but stayed registered with the TV,
  holding a video and an audio decoder, so after one or two episodes the next one couldn't start until the app was
  restarted, and LG's screensaver came up over a playing episode. Now nothing is left once the player closes.
- **Seeking with held arrows no longer runs away.** A press jumps 5 seconds; held, an arrow jumps at most 4 times a
  second and further the longer it's held (10, 20, then 30 s; 60 and 90 on the bar with its thumbnails). Letting go
  starts over at 5 s.

### Changed

- The player's controls always come up on play/pause.
- OK with the controls hidden is play/pause.

## 1.0.3 — 2026-11-15

### Fixed

- **The next episode no longer gets stuck.** When the screensaver covered a paused video, webOS minimized the app
  and the TV refused it the video decoder ("Resource Allocation Error (601)"), so the next episode never started
  until the app was restarted. A start that fails is now tried again (twice, once the app is back in front), OK
  tries again after that, and a new video waits for the last one to give the decoder back.
- **OK on the seek bar no longer pauses.** It only confirms a move of the knob; play/pause is the button below it.
  Pausing there left the controls on screen for good.
- A pause the TV ignores is asked again, and the controls follow what the picture does.

### Changed

- **No spoilers while an episode loads:** a black screen with the series and episode number, no still and no
  episode name until the video plays.
- Up from the seek bar puts the controls away (it still goes to Skip intro / Next episode when they show).

### Added

- **Settings → Advanced → Detailed log:** timestamped lines for every request and how it ended, playback states,
  pauses, seeks and decoder hand-overs. `crunchy.log` keeps the two runs before it.

## 1.0.2 — 2026-11-11

- The interface follows the TV's language (English, Spanish, Portuguese, French, German, Italian), with a setting to
  pick another one.
- Skip intro / recap / credits comes up on its own as that part starts, for 5–30 seconds (a setting), then shows
  with the controls.
- Smaller player controls as an option; the times line up with the title.
- Framework7 Icons (MIT) replace Material Icons. The project is MIT-licensed and the IPK carries the license.

## 1.0.1 — 2026-11-11

- The episode behind the focused play button is looked up ahead: OK to first frame in 2.0 s.
- Less work at rest: 1.2 % of one CPU core while nothing moves.

## 1.0.0 — 2026-11-11

- First release: home, series and movie pages, Simulcast, Series, Movies, Music, My List and Search; the player
  with seek thumbnails, skip buttons, next episode and audio, subtitle and quality menus; device-code sign-in and
  profiles.
