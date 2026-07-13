use std::{env, fs, path::PathBuf};

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let ram_run = env::var_os("NOBRO_RAM_RUN").is_some();
    let layout = if ram_run {
        "memory-ram.x"
    } else {
        "memory-flash.x"
    };
    fs::copy(layout, out.join("memory.x")).expect("copy memory layout");
    fs::write(out.join("defmt.x"), "").expect("write defmt.x stub");
    println!("cargo:rustc-link-search={}", out.display());

    let kernel = "vendor/FreeRTOS-Kernel";
    let mut c = cc::Build::new();
    if ram_run {
        c.define("NOBRO_RAM_RUN", None);
    }
    c.include("src")
        .include(format!("{kernel}/include"))
        .include(format!("{kernel}/portable/GCC/ARM_CM4F"))
        .file(format!("{kernel}/tasks.c"))
        .file(format!("{kernel}/queue.c"))
        .file(format!("{kernel}/list.c"))
        .file(format!("{kernel}/portable/GCC/ARM_CM4F/port.c"))
        .file("src/workload.c")
        .flag("-ffunction-sections")
        .flag("-fdata-sections")
        .flag("-fno-common")
        .warnings(false)
        .compile("freertos_complex");

    println!("cargo:rerun-if-changed=memory-flash.x");
    println!("cargo:rerun-if-changed=memory-ram.x");
    println!("cargo:rerun-if-env-changed=NOBRO_RAM_RUN");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=vendor/FreeRTOS-Kernel");
}
