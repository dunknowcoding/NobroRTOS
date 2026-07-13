use std::{env, fs, path::PathBuf};

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let memory_x = PathBuf::from("../../../memory-nosd.x");
    fs::copy(&memory_x, out.join("memory.x")).expect("copy memory.x");
    println!("cargo:rerun-if-changed=../../../memory-nosd.x");
    println!("cargo:rustc-link-search={}", out.display());
}
