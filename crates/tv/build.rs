//! Embeds the icon set: Framework7 Icons (assets/icons/open-icons.json, MIT), or with `CRUNCHY_ICONS=sf` Apple's SF
//! Symbols as the web app generates them (assets/icons/sf-symbols.json, local only: Apple's license keeps them out
//! of anything distributed) when that file is there.

use std::path::Path;

fn main() {
    let icons = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/icons");
    let (sf, open) = (icons.join("sf-symbols.json"), icons.join("open-icons.json"));
    println!("cargo:rerun-if-changed={}", sf.display());
    println!("cargo:rerun-if-changed={}", open.display());
    println!("cargo:rerun-if-env-changed=CRUNCHY_ICONS");
    let want_sf = std::env::var("CRUNCHY_ICONS").is_ok_and(|value| value == "sf");
    let source = if want_sf && sf.exists() { sf } else { open };
    let json = std::fs::read_to_string(&source).unwrap_or_else(|_| "{\"glyphs\":{}}".to_string());
    let out = Path::new(&std::env::var("OUT_DIR").unwrap()).join("icons.json");
    std::fs::write(out, json).unwrap();
}
