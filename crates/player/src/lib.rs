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
