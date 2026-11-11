//! Where frames are drawn and keys come from: the TV (Wayland + EGL, OpenGL ES 2) or, on a computer, a window
//! that simulates it (desktop OpenGL), both drawing through the same [`crate::gfx::Renderer`].

use std::path::PathBuf;

use crate::gfx::Renderer;

#[cfg(target_os = "macos")]
pub mod desktop;
#[cfg(target_os = "linux")]
pub mod webos;

/// Remote keys the app acts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    Ok,
    Back,
    /// anything else, by its platform code (logged, to map later)
    Other(u32),
}

/// What the platform drives: one call per frame with the keys pressed since the last one.
pub trait App {
    /// Draws a frame. Returns `false` to quit.
    fn frame(&mut self, gfx: &mut Renderer, keys: &[Key]) -> bool;
}

/// A writable folder for the session, caches and logs.
pub fn data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("CRUNCHY_DATA_DIR") {
        return PathBuf::from(dir);
    }
    #[cfg(target_os = "linux")]
    {
        // the app's own folder on the TV's developer partition survives reinstalls of the same version
        PathBuf::from("/media/developer/apps/usr/palm/data/com.ian.crunchy")
    }
    #[cfg(not(target_os = "linux"))]
    {
        std::env::temp_dir().join("crunchy-sim")
    }
}

#[cfg(target_os = "macos")]
pub fn run(app: impl App + 'static) -> anyhow::Result<()> {
    desktop::run(app)
}

#[cfg(target_os = "linux")]
pub fn run(app: impl App + 'static) -> anyhow::Result<()> {
    webos::run(app)
}
