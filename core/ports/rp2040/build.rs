use std::{env, fs, path::PathBuf};

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    fs::copy("memory.x", out.join("memory.x")).expect("copy memory.x");
    println!("cargo:rerun-if-changed=memory.x");
    println!("cargo:rustc-link-search={}", out.display());
    // Build scripts run for the selected manifest even when Cargo is invoked
    // from the workspace root. A child `.cargo/config.toml` is not discovered
    // from that working directory, so the flash layout must travel with the
    // package rather than depend on the caller's shell location.
    println!("cargo:rustc-link-arg=--nmagic");
    println!("cargo:rustc-link-arg=-Tlink.x");
}
