//! Crunchy: a native Crunchyroll client for LG webOS TVs. The UI is drawn straight with OpenGL ES; there's no web
//! view anywhere.

mod app;
mod gfx;
mod platform;

fn main() -> anyhow::Result<()> {
    platform::run(app::CrunchyApp::new())
}
