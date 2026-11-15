//! Video without a browser: Crunchyroll's encrypted DASH played the way the TV's own Chromium plays it. The fMP4
//! samples are demuxed here ([`mp4`]), rewritten into the elementary streams the decoder takes ([`es`]), handed to
//! the TV's Widevine library ([`cdm`]), which wraps each one for the secure video path instead of decrypting it,
//! and fed to LG's in-process media pipeline ([`starfish`]).

pub mod es;
pub mod mp4;

#[cfg(target_os = "linux")]
pub mod cdm;
#[cfg(target_os = "linux")]
mod dl;
#[cfg(target_os = "linux")]
pub mod engine;
#[cfg(target_os = "linux")]
pub mod starfish;

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

static DEBUG: AtomicBool = AtomicBool::new(false);
static START: OnceLock<Instant> = OnceLock::new();

/// Turns detailed logging on or off (the app's "Detailed log" setting, or `CRUNCHY_DEBUG`).
pub fn set_debug(on: bool) {
    START.get_or_init(Instant::now);
    DEBUG.store(on, Ordering::Relaxed);
}

/// Whether detailed logging is on.
pub fn debug() -> bool {
    DEBUG.load(Ordering::Relaxed)
}

/// Seconds since logging was set up, to stamp detailed log lines.
pub fn stamp() -> f64 {
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

/// A detailed log line ("player:" and the time in front), only while [`debug`] is on.
#[macro_export]
macro_rules! debug_log {
    ($($arg:tt)*) => {
        if $crate::debug() {
            eprintln!("[{:9.3}] {}", $crate::stamp(), format_args!($($arg)*));
        }
    };
}

/// A run of clear bytes followed by encrypted ones, as in an ISO CENC `senc` entry.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Subsample {
    pub clear: u32,
    pub cipher: u32,
}

/// Where playback stands.
#[derive(Clone, Debug, PartialEq)]
pub enum State {
    /// filling the pipeline before it plays (at the start and after a seek)
    Loading,
    Playing,
    Paused,
    Ended,
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct Status {
    pub state: State,
    pub position: std::time::Duration,
    pub duration: std::time::Duration,
}
