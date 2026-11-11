//! Embeds the icon set: Apple's SF Symbols as the web app generates them (assets/icons/sf-symbols.json, local only:
//! Apple's license keeps them out of anything distributed) when that file is there, else Google's Material Icons
//! Round (assets/icons/open-icons.json, Apache-2.0). `CRUNCHY_ICONS=open` takes the open set even where the SF
//! Symbols are: releases are built that way.

use std::path::Path;

fn main() {
    let icons = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/icons");
    let (sf, open) = (icons.join("sf-symbols.json"), icons.join("open-icons.json"));
    println!("cargo:rerun-if-changed={}", sf.display());
    println!("cargo:rerun-if-changed={}", open.display());
    println!("cargo:rerun-if-env-changed=CRUNCHY_ICONS");
    let want_open = std::env::var("CRUNCHY_ICONS").is_ok_and(|value| value == "open");
    let source = if want_open || !sf.exists() { open } else { sf };
    let json = std::fs::read_to_string(&source).unwrap_or_else(|_| "{\"glyphs\":{}}".to_string());
    let out = Path::new(&std::env::var("OUT_DIR").unwrap()).join("icons.json");
    std::fs::write(out, json).unwrap();
}
