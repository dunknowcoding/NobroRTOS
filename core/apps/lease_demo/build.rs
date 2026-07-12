use std::{env, fs, path::PathBuf};
fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let linker = if env::var_os("CARGO_FEATURE_BOARD_NICENANO_S140").is_some() {
        "../../memory-s140.x"
    } else {
        "../../memory-nosd.x"
    };
    fs::copy(linker, out.join("memory.x")).expect("copy memory.x");
    println!("cargo:rerun-if-changed={linker}");
    println!("cargo:rustc-link-search={}", out.display());
}
