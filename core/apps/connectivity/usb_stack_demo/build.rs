use std::{env, fs, path::PathBuf};

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let memory = if env::var("CARGO_FEATURE_BACKEND_RA_USBFS").is_ok() {
        PathBuf::from("../../../ports/ra4m1/memory.x")
    } else {
        PathBuf::from("../../../memory-s140.x")
    };
    let dest = out.join("memory.x");
    fs::copy(&memory, &dest).expect("copy memory.x");
    println!("cargo:rerun-if-changed={}", memory.display());
    println!("cargo:rerun-if-changed=../../../memory-s140.x");
    println!("cargo:rerun-if-changed=../../../ports/ra4m1/memory.x");
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_BACKEND_RA_USBFS");
}
