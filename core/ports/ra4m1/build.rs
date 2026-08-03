use std::{env, fs, path::PathBuf};

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::copy("memory.x", out.join("memory.x")).expect("copy memory.x");
    fs::copy("defmt.x", out.join("defmt.x")).expect("copy defmt.x");
    fs::copy("device.x", out.join("device.x")).expect("copy device.x");
    if env::var("CARGO_FEATURE_ISOLATION_SELFTEST").is_ok() {
        fs::copy("isolation.x", out.join("isolation.x")).expect("copy isolation.x");
        println!("cargo:rerun-if-changed=isolation.x");
        println!("cargo:rustc-link-arg=-Tisolation.x");
    }
    println!("cargo:rerun-if-changed=memory.x");
    println!("cargo:rerun-if-changed=device.x");
    println!("cargo:rustc-link-search={}", out.display());
}
