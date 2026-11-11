//! Where frames are drawn and keys come from: the TV (Wayland + EGL, OpenGL ES 2) or, on a computer, a window
//! that simulates it (desktop OpenGL), both drawing through the same [`crate::gfx::Renderer`].

use std::path::PathBuf;
use std::sync::Mutex;

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

/// The window the TV's media pipeline shows video in: a part of our surface handed to the compositor, which puts the
/// decoded picture there, under what we draw (so the app draws transparent where it should show). The app asks for
/// it before playing and lets it go after; its id arrives a moment later and goes in the pipeline's load payload.
static VIDEO_WINDOW: Mutex<(bool, Option<String>)> = Mutex::new((false, None));

pub fn want_video_window(wanted: bool) {
    VIDEO_WINDOW.lock().unwrap().0 = wanted;
}

/// The video window's id once the compositor assigned one.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn video_window() -> Option<String> {
    let window = VIDEO_WINDOW.lock().unwrap();
    if window.0 { window.1.clone() } else { None }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn video_window_wanted() -> bool {
    VIDEO_WINDOW.lock().unwrap().0
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn set_video_window(id: Option<String>) {
    VIDEO_WINDOW.lock().unwrap().1 = id;
}

/// A writable folder for the session, caches and logs: the first of the candidates the app can write to.
pub fn data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("CRUNCHY_DATA_DIR") {
        return PathBuf::from(dir);
    }
    let candidates: Vec<PathBuf> = if cfg!(target_os = "linux") {
        vec![
            // survives reinstalls; may not be writable for a sandboxed app
            PathBuf::from("/media/developer/apps/usr/palm/data/com.ian.crunchy"),
            std::env::var("HOME").map(|home| PathBuf::from(home).join(".crunchy")).unwrap_or_default(),
            PathBuf::from("/tmp/com.ian.crunchy"),
        ]
    } else {
        vec![std::env::temp_dir().join("crunchy-sim")]
    };
    for dir in candidates.into_iter().filter(|dir| !dir.as_os_str().is_empty()) {
        let probe = dir.join(".write-test");
        if std::fs::create_dir_all(&dir).is_ok() && std::fs::write(&probe, b"").is_ok() {
            let _ = std::fs::remove_file(probe);
            return dir;
        }
    }
    std::env::temp_dir()
}

/// On the TV nothing reads stderr: send it to `crunchy.log` in the data folder (the previous run's kept as `.1`).
pub fn log_to_file(dir: &std::path::Path) {
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        unsafe extern "C" {
            fn dup2(old: i32, new: i32) -> i32;
        }
        let path = dir.join("crunchy.log");
        let _ = std::fs::rename(&path, dir.join("crunchy.log.1"));
        if let Ok(file) = std::fs::File::create(&path) {
            unsafe {
                dup2(file.as_raw_fd(), 2);
            }
            std::mem::forget(file);
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = dir;
}

#[cfg(target_os = "macos")]
pub fn run(app: impl App + 'static) -> anyhow::Result<()> {
    desktop::run(app)
}

#[cfg(target_os = "linux")]
pub fn run(app: impl App + 'static) -> anyhow::Result<()> {
    webos::run(app)
}
