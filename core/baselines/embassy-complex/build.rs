use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rustc-check-cfg=cfg(nobro_ram_run)");
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let ram_run = env::var_os("NOBRO_RAM_RUN").is_some();
    let layout = if ram_run {
        println!("cargo:rustc-cfg=nobro_ram_run");
        "memory-ram.x"
    } else {
        "memory-flash.x"
    };
    fs::copy(layout, out.join("memory.x")).expect("copy memory layout");
    fs::write(out.join("defmt.x"), "").expect("write defmt.x stub");
    println!("cargo:rerun-if-changed=memory-flash.x");
    println!("cargo:rerun-if-changed=memory-ram.x");
    println!("cargo:rerun-if-env-changed=NOBRO_RAM_RUN");
    println!("cargo:rustc-link-search={}", out.display());
}
