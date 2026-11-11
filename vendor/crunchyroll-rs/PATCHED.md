# crunchyroll-rs 0.19.1, patched

This is [crunchyroll-rs](https://github.com/crunchy-labs/crunchyroll-rs) 0.19.1 as published on crates.io
(MIT OR Apache-2.0, see `LICENSE-MIT` and `LICENSE-APACHE`), with one change:

- `src/media/stream.rs`: `Stream` keeps the play response's `bifs` field (`pub bifs: serde_json::Value`), the link
  to the stream's seek thumbnails. Upstream declares it only under its `__test_strict` feature, so normal builds
  drop it, and asking for the stream a second time just to read it would open a second stream session.

Everything else is unchanged. The workspace uses it through `[patch.crates-io]` in the top-level `Cargo.toml`.
