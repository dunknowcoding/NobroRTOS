use std::{env, fs, path::PathBuf};

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::copy("memory.x", out.join("memory.x")).expect("copy memory.x");
    fs::copy("defmt.x", out.join("defmt.x")).expect("copy defmt.x");
    println!("cargo:rerun-if-changed=memory.x");
    println!("cargo:rustc-link-search={}", out.display());
}
