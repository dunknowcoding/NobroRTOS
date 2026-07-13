use std::{env, fs, path::PathBuf};

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let memory = if env::var("CARGO_FEATURE_BOARD_NICENANO_S140").is_ok() {
        PathBuf::from("../../../memory-s140.x")
    } else {
        PathBuf::from("../../../memory-s140.x")
    };
    let dest = out.join("memory.x");
    fs::copy(&memory, &dest).expect("copy memory.x");
    println!("cargo:rerun-if-changed={}", memory.display());
    println!("cargo:rerun-if-changed=../../../memory-s140.x");
    println!("cargo:rerun-if-changed=../../../memory-s140.x");
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_BOARD_NICENANO_S140");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_BOARD_PROMICRO_NOSD");
}
