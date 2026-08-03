use std::{env, fs, path::PathBuf};

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let memory = if env::var("CARGO_FEATURE_BOARD_PROMICRO_S140").is_ok() {
        "../../../memory-s140.x"
    } else {
        "../../../memory-nosd.x"
    };
    fs::copy(memory, out.join("memory.x")).expect("copy memory.x");
    fs::copy("isolation.x", out.join("isolation.x")).expect("copy isolation.x");
    println!("cargo:rerun-if-changed=../../../memory-nosd.x");
    println!("cargo:rerun-if-changed=../../../memory-s140.x");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_BOARD_PROMICRO_S140");
    println!("cargo:rerun-if-changed=isolation.x");
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rustc-link-arg=-Tisolation.x");
}
