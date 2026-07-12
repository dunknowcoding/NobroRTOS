use std::{env, fs, path::PathBuf};
fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::copy("memory.x", out.join("memory.x")).expect("copy memory.x");
    // The parent tree links -Tdefmt.x; baselines carry no defmt, so satisfy
    // the flag with an empty stub instead of pulling the logging stack in.
    fs::write(out.join("defmt.x"), "").expect("write defmt.x stub");
    println!("cargo:rerun-if-changed=memory.x");
    println!("cargo:rustc-link-search={}", out.display());
}
