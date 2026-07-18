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
(not just inspecting reports). The declarative Tier-C facade keeps the same task
mental model as the Rust graph: name the task, give it a rate, and provide its step.
`NOBRO_APP` generates the legacy init/poll callbacks:

```c
#include "nobro_app.h"

static int32_t imu_step(void) {
    /* one bounded sensor transaction */
    return 0;
}

static int32_t control_step(void) {
    return 0;
}

static int32_t configure(void) {
    nobro_task_options_t control = NOBRO_TASK_OPTIONS_INIT;
    control.role = NOBRO_TASK_CONTROL;
    control.budget_us = 2000;

    int32_t result = nobro_task("imu", HZ(100), imu_step);
    if (result < 0) return result;
    result = nobro_task_with("control", HZ(50), control_step, &control);
    if (result < 0) return result;
    result = nobro_wire("imu", "control", 8);
    if (result < 0) return result;
    return nobro_run();
}

NOBRO_APP(configure)
```

Registration is allocation-free and bounded to eight tasks plus eight wires in
the current Tier-C runtime. Names are static lowercase labels. A task defaults to
the periodic-driver role; `nobro_task_options_t` selects control/service policy
and timing overrides without expanding the main call. `nobro_run()` admits the
declaration through the same Rust `AppGraph` implementation. Late polling runs a
task once, preserves its phase, and increments `nobro_skipped_releases()` instead
of replaying a burst. A negative task result returns `NOBRO_ERR_STEP`;
`nobro_last_step_error()` retains the task's original code.
The current nRF Tier-C composition checks releases from the microsecond clock
with a tight busy-poll loop. It does not yet provide a compare/WFE sleep path, so
the target-link result is not an idle-residence or electrical-power claim.

`nobro_wire()` declares and validates the graph/mailbox relationship and retains
its bounded capacity metadata. It does not itself send or store payloads; use a
separate bounded transport API for data.

The original two-callback ABI remains supported by
`examples/imu_module.c`. Callback dispatch is fail-closed. Admission denial
prevents both callbacks; a negative init or poll result revokes the module's host
capabilities and prevents subsequent polls.

The NobroRTOS app provides the `extern "C"` host services, admits the module through
`BootAssembly`, and drives the callbacks. Because the ABI is plain `extern "C"`, the
module object can come from any toolchain. `core/apps/interop/c_abi_demo` builds it two ways
from one source of truth:

- `--features rust-module` (default): links the reference module from
  `core/apps/interop/c_abi_module` (Rust `extern "C"`, byte-identical ABI) - no C compiler
  needed.
- `--features c-source`: `build.rs` compiles `examples/imu_module.c` with
  `arm-none-eabi-gcc` and links it.

Both legacy callback paths are verified on hardware (nRF52840 + an IMU): the kernel admits the
module and it reads the IMU to a passing `NOBRO_IMU_HEALTH_REPORT`. See
`examples/imu_module.c` for the complete reference module.
The declarative specimen is covered by portable behavior tests and a strict C11
Arm target-link/symbol gate; that is not a physical timing claim.

## Scope

The C binding focuses on fixed contracts, report inspection, and the bounded
Tier-C task facade. Board package helpers and adapter registration wrappers
should stay layered on top of this ABI instead of changing report layouts.
