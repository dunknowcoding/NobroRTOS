use std::{env, fs, path::PathBuf};

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::copy("memory.x", out.join("memory.x")).expect("copy memory.x");
    fs::write(out.join("defmt.x"), "").expect("write defmt.x stub");
    println!("cargo:rustc-link-search={}", out.display());

    let kernel = "vendor/FreeRTOS-Kernel";
    cc::Build::new()
        .include("src")
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

    println!("cargo:rerun-if-changed=memory.x");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=vendor/FreeRTOS-Kernel");
}
