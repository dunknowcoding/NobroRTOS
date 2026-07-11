use std::{env, fs, path::PathBuf};

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let memory_x = if env::var("CARGO_FEATURE_BOARD_NICENANO_S140").is_ok() {
        "../../memory-s140.x"
    } else {
        "../../memory-nosd.x"
    };
    fs::copy(memory_x, out.join("memory.x")).expect("copy memory.x");
    println!("cargo:rerun-if-changed=../../memory-nosd.x");
    println!("cargo:rerun-if-changed=../../memory-s140.x");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_BOARD_NICENANO_S140");
    println!("cargo:rustc-link-search={}", out.display());
}
