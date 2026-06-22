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

## Scope

The C++ layer currently focuses on type-safe report views plus small helpers for
stable hashes, AI route decisions, AI invocation preflight, AI model reports,
ROS bridge summaries, and runtime diagnostics such as admission, runtime,
health, event-log, module-runtime, and degraded-mode reports. Builders for
module manifests, board packages, and adapter descriptors should remain thin
wrappers over fixed-size C or Rust-compatible records.
