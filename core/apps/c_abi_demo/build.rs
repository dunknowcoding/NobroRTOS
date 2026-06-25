use std::{env, fs, path::PathBuf};

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let memory = if env::var("CARGO_FEATURE_BOARD_NICENANO_S140").is_ok() {
        PathBuf::from("../../memory-s140.x")
    } else {
        PathBuf::from("../../memory-nosd.x")
    };
    fs::copy(&memory, out.join("memory.x")).expect("copy memory.x");
    println!("cargo:rerun-if-changed={}", memory.display());
    println!("cargo:rerun-if-changed=../../memory-nosd.x");
    println!("cargo:rerun-if-changed=../../memory-s140.x");
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_BOARD_NICENANO_S140");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_BOARD_PROMICRO_NOSD");

    // With feature "c-source", compile the reference C module (needs a C cross
    // compiler, e.g. arm-none-eabi-gcc). It provides the same extern "C"
    // nobro_app_init / nobro_app_poll symbols as the Rust module crate - link one.
    #[cfg(feature = "c-source")]
    {
        // Point NOBRO_C_MODULE at your own .c to author a module without editing this
        // crate; defaults to the reference IMU module.
        println!("cargo:rerun-if-env-changed=NOBRO_C_MODULE");
        let c = env::var("NOBRO_C_MODULE")
            .unwrap_or_else(|_| "../../../bindings/c/examples/imu_module.c".into());
        println!("cargo:rerun-if-changed={c}");
        cc::Build::new()
            .file(&c)
            .include("../../../bindings/c/include")
            .compile("nobro_c_module");
    }

    // With feature "cpp-source", compile the reference C++ module (arm-none-eabi-g++).
    // Embedded-safe C++: no exceptions / RTTI / global constructors. Same extern "C"
    // nobro_app_* symbols as the other providers - link exactly one.
    #[cfg(feature = "cpp-source")]
    {
        // Point NOBRO_CPP_MODULE at your own .cpp (e.g. an Arduino-style setup/loop
        // module); defaults to the reference IMU module.
        println!("cargo:rerun-if-env-changed=NOBRO_CPP_MODULE");
        let cpp = env::var("NOBRO_CPP_MODULE")
            .unwrap_or_else(|_| "../../../bindings/cpp/examples/imu_module.cpp".into());
        println!("cargo:rerun-if-changed={cpp}");
        cc::Build::new()
            .cpp(true)
            .cpp_link_stdlib(None) // bare-metal: no libstdc++ (no exceptions/RTTI/std)
            .file(cpp)
            .include("../../../bindings/cpp/include")
            .flag_if_supported("-fno-exceptions")
            .flag_if_supported("-fno-rtti")
            .flag_if_supported("-fno-use-cxa-atexit")
            .compile("nobro_cpp_module");
    }
}
