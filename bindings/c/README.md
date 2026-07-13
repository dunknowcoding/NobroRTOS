# NobroRTOS C Binding

This folder provides a dependency-light C ABI surface for host-readable
NobroRTOS reports. It is designed for firmware glue, host tools, and test
harnesses that need stable fixed-layout records without linking Rust code.

## Header

Add `bindings/c/include` to your include path and use:

```c
#include "nobro_rtos.h"
```

The header currently mirrors:

- `nobro_board_profile_report_t`
- `nobro_board_package_report_t`
- `nobro_manifest_report_t`
- `nobro_adapter_compat_report_t`
- `nobro_admission_report_t`
- `nobro_runtime_report_t`
- `nobro_health_report_t`
- `nobro_event_log_report_t`
- `nobro_module_runtime_report_t`
- `nobro_degrade_application_report_t`
- `nobro_ai_model_contract_t`
- `nobro_ai_route_policy_t`
- `nobro_ros_bridge_contract_t`
- ROS topic, service, action, and parameter contract records

Each report has inline checksum and status helpers:

```c
nobro_manifest_report_t report = {0};
report.magic = NOBRO_MANIFEST_REPORT_MAGIC;
report.version = NOBRO_REPORT_VERSION;
report.completed = 1;
report.valid = 1;
report.checksum = nobro_manifest_report_checksum(&report);

if (nobro_manifest_report_status(&report) != NOBRO_REPORT_STATUS_PASS) {
    return -1;
}
```

The structs use only `uint32_t` fields and include compile-time size checks for
C11 toolchains. Older C toolchains receive typedef-based static assertions.

AI and ROS bridge helpers stay allocation-free:

```c
uint32_t imu_hash = nobro_stable_hash32_cstr("/imu");
uint32_t stale_window = nobro_ai_effective_stale_after_us(policy, model);
nobro_ai_route_decision_t decision = nobro_ai_route_decide(
    policy,
    model,
    runtime_state,
    20000u
);
nobro_ai_invocation_limits_t limits = {
    32u,
    128u,
    8192u,
    20000u,
    50000u,
    0u,
    0u,
    0u,
    0u,
};
nobro_ai_invocation_preflight_t preflight = nobro_ai_invocation_preflight(
    policy,
    model,
    runtime_state,
    model.model_id,
    16u,
    limits
);
nobro_ros_topic_contract_t topic = {
    nobro_stable_hash32_cstr("/imu"),
    nobro_stable_hash32_cstr("sensor_msgs/Imu"),
    4u,
    64u,
};
nobro_ros_bridge_preflight_t topic_check =
    nobro_ros_topic_preflight(topic, 32u);
```

For AI routing, a zero policy stale window inherits the model contract's
`stale_after_us`; otherwise the helper uses the stricter window. AI preflight
checks buffer sizes, RAM demand, route fallback, stale snapshots, and endpoint
circuit state before a model or remote endpoint is contacted.
ROS preflight checks topic payloads, service/action response capacity, queue
depth, parameter value size, and timeout budget before a ROS agent or transport
is contacted.

## Authoring a module in C

`include/nobro_app.h` is a second, separate ABI for writing module **logic** in C
(not just inspecting reports). A C module implements two callbacks and reaches
hardware only through bounded host services - it never touches kernel internals:

```c
#include "nobro_app.h"

int32_t nobro_app_init(void) {                 /* kernel calls once after admission */
    uint8_t wake[2] = {0x6B, 0x01};
    return nobro_i2c_write(0x68, wake, 2);
}

int32_t nobro_app_poll(void) {                 /* kernel calls every cycle */
    uint8_t reg = 0x3B, raw[14];
    if (nobro_i2c_write_read(0x68, &reg, 1, raw, 14) < 0) return -1;
    /* ... parse + nobro_publish_imu(...) ... */
    return 0;
}
```

Callback dispatch is fail-closed. Admission denial prevents both callbacks;
negative `nobro_app_init` or `nobro_app_poll` results revoke the module's host
capabilities and prevent subsequent polls. The Tier-C packaging gate links both
negative callback objects with the shipped archive, while portable kernel tests
execute and verify the denial/failure state transitions.

The NobroRTOS app provides the `extern "C"` host services, admits the module through
`BootAssembly`, and drives the callbacks. Because the ABI is plain `extern "C"`, the
module object can come from any toolchain. `core/apps/interop/c_abi_demo` builds it two ways
from one source of truth:

- `--features rust-module` (default): links the reference module from
  `core/apps/interop/c_abi_module` (Rust `extern "C"`, byte-identical ABI) - no C compiler
  needed.
- `--features c-source`: `build.rs` compiles `examples/imu_module.c` with
  `arm-none-eabi-gcc` and links it.

Both paths are verified on hardware (nRF52840 + an IMU): the kernel admits the
module and it reads the IMU to a passing `NOBRO_IMU_HW_EVAL_REPORT`. See
`examples/imu_module.c` for the complete reference module.

## Scope

The C binding focuses on fixed contracts and report inspection. Module builders,
board package helpers, and adapter registration wrappers should stay layered on
top of this ABI instead of changing these layouts.
