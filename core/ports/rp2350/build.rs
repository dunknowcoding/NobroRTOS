use std::{env, fs, path::PathBuf};
fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::copy("memory.x", out.join("memory.x")).expect("copy memory.x");
    if env::var("CARGO_FEATURE_ISOLATION_SELFTEST").is_ok() {
        fs::copy("isolation.x", out.join("isolation.x")).expect("copy isolation.x");
        println!("cargo:rerun-if-changed=isolation.x");
        println!("cargo:rustc-link-arg=-Tisolation.x");
    }
    println!("cargo:rerun-if-changed=memory.x");
    println!("cargo:rustc-link-search={}", out.display());
    // Keep the exact XIP layout independent of Cargo's invocation directory.
    println!("cargo:rustc-link-arg=--nmagic");
    println!("cargo:rustc-link-arg=-Tlink.x");
}
