use std::{env, fs, path::PathBuf};

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let memory = if env::var("CARGO_FEATURE_BOARD_NICENANO_S140").is_ok() {
        PathBuf::from("../../../memory-s140.x")
    } else {
        PathBuf::from("../../../memory-nosd.x")
    };
    fs::copy(&memory, out.join("memory.x")).expect("copy memory.x");
    println!("cargo:rerun-if-changed=../../../memory-nosd.x");
    println!("cargo:rerun-if-changed=../../../memory-s140.x");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_BOARD_NICENANO_S140");
    println!("cargo:rustc-link-search={}", out.display());

    // backend-arduino: compile the Arduino-style C++ driver through the shim
    // (embedded-safe C++: no exceptions/RTTI/libstdc++), like c_abi_demo's cpp-source.
    #[cfg(feature = "backend-arduino")]
    {
        let cpp = "../../../../bindings/cpp/arduino_shim/examples/ArduinoStyleMPU9250.cpp";
        println!("cargo:rerun-if-changed={cpp}");
        println!("cargo:rerun-if-changed=../../../../bindings/cpp/arduino_shim/NobroArduinoShim.h");
        cc::Build::new()
            .cpp(true)
            .cpp_link_stdlib(None)
            .file(cpp)
            .flag_if_supported("-fno-exceptions")
            .flag_if_supported("-fno-rtti")
            .flag_if_supported("-fno-use-cxa-atexit")
            .compile("nobro_arduino_module");
    }
}
