//! Embeds the SF Symbols the web app generates (assets/icons/sf-symbols.json, local only) when it's there; a build
//! without it still works, its icons just don't draw.

fn main() {
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/icons/sf-symbols.json");
    println!("cargo:rerun-if-changed={}", source.display());
    let out = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("sf-symbols.json");
    let json = std::fs::read_to_string(&source).unwrap_or_else(|_| "{\"glyphs\":{}}".to_string());
    std::fs::write(out, json).unwrap();
}
