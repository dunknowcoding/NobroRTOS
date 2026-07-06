use std::{env, fs, path::PathBuf};

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    // No-SoftDevice profile, app @ 0x1000.
    let memory = PathBuf::from("../../memory-nosd.x");
    let dest = out.join("memory.x");
    fs::copy(&memory, &dest).expect("copy memory.x");
    println!("cargo:rerun-if-changed=../../memory-nosd.x");
    println!("cargo:rustc-link-search={}", out.display());
}
