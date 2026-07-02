use std::{env, fs, path::PathBuf};
fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::copy("../../memory-nosd.x", out.join("memory.x")).expect("copy memory.x");
    println!("cargo:rerun-if-changed=../../memory-nosd.x");
    println!("cargo:rustc-link-search={}", out.display());
}
