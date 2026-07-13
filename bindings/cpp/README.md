# NobroRTOS C++ Binding

This folder provides header-only C++ convenience wrappers over the C ABI. The
wrappers are allocation-free, exception-free, and intended for control
libraries, sensor libraries, host tests, and SDK-style integrations.

## Header

Add both include folders to your include path:

- `bindings/c/include`
- `bindings/cpp/include`

Then use:

```cpp
#include "nobro_rtos.hpp"
```

Example:

```cpp
nobro_manifest_report_t report{};
report.magic = NOBRO_MANIFEST_REPORT_MAGIC;
report.version = NOBRO_REPORT_VERSION;
report.completed = 1;
report.valid = 1;
report.checksum = nobro_manifest_report_checksum(&report);

nobro::rtos::ManifestReportView view(report);
if (!nobro::rtos::passing(view.status())) {
    return -1;
}
```

## Authoring a module in C++

`include/nobro_app.hpp` is a separate facade for writing module **logic** in C++
over the C ABI. A module is a plain struct with two static methods, registered in
one line; it is deliberately bare-metal-safe (no global constructors, vtables,
exceptions, RTTI, or heap):

```cpp
#include "nobro_app.hpp"

struct ImuModule {
    static int32_t init() {
        const uint8_t wake[2] = {0x6B, 0x01};
        return nobro::I2c::write(0x68, wake, 2);
    }
    static int32_t poll() {
        uint8_t reg = 0x3B, raw[14];
        if (nobro::I2c::write_read(0x68, &reg, 1, raw, 14) < 0) return -1;
        /* ... parse + nobro::publish_imu(...) ... */
        return 0;
    }
};
NOBRO_REGISTER_MODULE(ImuModule)
```

The NobroRTOS app (`core/apps/interop/c_abi_demo`) admits the module and drives the
callbacks. Build with `--features cpp-source`: `build.rs` compiles the `.cpp` with
`arm-none-eabi-g++` (`-fno-exceptions -fno-rtti`, no libstdc++) and links it - the
same `extern "C"` `nobro_app_*` symbols as the C and Rust providers. Verified on
hardware (nRF52840 + an IMU): the kernel admits the C++ module and it reads the IMU
to a passing `NOBRO_IMU_HW_EVAL_REPORT`. See `examples/imu_module.cpp`.

## Scope

The C++ layer currently focuses on type-safe report views plus small helpers for
stable hashes, AI route decisions, AI invocation preflight, AI model reports,
ROS bridge summaries, ROS bridge preflight, and runtime diagnostics such as
admission, runtime, health, event-log, module-runtime, and degraded-mode
reports. Builders for module manifests, board packages, and adapter descriptors
should remain thin wrappers over fixed-size C or Rust-compatible records.
