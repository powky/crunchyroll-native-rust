//! Crunchyroll for the TV app, on top of [`crunchyroll_rs`]: a session that signs in with a code shown on the
//! screen and stays signed in across launches, and the catalog shaped the way the screens show it.

pub mod account;
pub mod api;
pub mod auth;
pub mod catalog;
pub mod content;
pub mod images;
pub mod model;
pub mod play;
pub mod platform;
pub mod session;
pub mod store;

pub use crunchyroll_rs;
