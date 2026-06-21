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
```

For AI routing, a zero policy stale window inherits the model contract's
`stale_after_us`; otherwise the helper uses the stricter window.

## Scope

The C binding focuses on fixed contracts and report inspection. Module builders,
board package helpers, and adapter registration wrappers should stay layered on
top of this ABI instead of changing these layouts.
