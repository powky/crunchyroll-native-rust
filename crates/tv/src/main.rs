//! Crunchy: a native Crunchyroll client for LG webOS TVs. The UI is drawn straight with OpenGL ES; there's no web
//! view anywhere.

mod app;
mod gfx;
mod player;
mod platform;

fn main() -> anyhow::Result<()> {
    platform::log_to_file(&platform::data_dir());
    eprintln!("crunchy {} starting, args {:?}", env!("CARGO_PKG_VERSION"), std::env::args().collect::<Vec<_>>());
    let result = platform::run(app::CrunchyApp::new());
    if let Err(error) = &result {
        eprintln!("crunchy: fatal: {error:#}");
    }
    result
}
