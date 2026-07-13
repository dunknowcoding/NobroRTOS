# Limitations and support boundaries

A successful build does not imply full peripheral support, declared timing is not a
formal WCET proof, and host simulation is not device execution. The machine-readable
platform matrix is `core/boards/platform_tiers.json`; adapter/library membership is
listed separately in `core/adapters/catalog.json`.

## Application model

| Boundary | Current behavior | Practical consequence |
| --- | --- | --- |
| Scheduling | Cooperative fixed-priority execution with response-time admission, fuel-bounded async, and overrun containment | A callback that never yields still needs a deadline/watchdog path to recover it; there is no general preemptive time-slicing profile |
| Timing | Admission uses declared or measured budgets and pessimistic interference | Blocking terms and unmeasured compiler/platform paths are not formal WCET bounds |
| Async | No allocation; fixed task, timer, waiter, and channel capacities | Capacity is explicit and exhaustion is reported instead of allocating dynamically |
| Composition | One graph derives manifest, startup, task metadata, labels, and mailbox grants | Capability kinds remain a closed bit set and stable numeric module codes remain on wire formats |
| Project workflow | `nobro project` creates, explains, builds, simulates, and reports; `nobro firmware` emits nRF firmware from one declaration | Firmware generation currently covers explicit nRF52840 layouts and does not infer WCET or interrupt/DMA ownership |
| Arduino authoring | `NobroApp` declares fixed-capacity tasks/channels and previews admission | The facade does not embed the Rust executor or prove device timing |

## Resources

Admission, identity, quotas, recovery, and diagnostics consume flash, RAM, stack, and
CPU. Small applications that do not need those controls may be smaller with a direct
loop. Size and timing depend on the selected profile, compiler, target, and workload;
measure the final application. Static RAM is not total RAM, so deployment review must
also include stacks and bounded arenas. Current or energy estimates require calibrated
hardware and are not inferred from software coefficients alone.

## Platform support

| Tier | Platforms | Included | Missing |
| --- | --- | --- | --- |
| Deep | nRF52840 | Portable core plus board-specific time, deadline, event, PWM, I2C, SPI, and lease providers | USB parity and broader board families |
| Provider | RP2350, ESP32-C3, ESP32-S3, RA4M1 | Selected typed providers; ESP32-S3 and RA4M1 include time, deadline, I2C, SPI, PWM, and USB paths (RA4M1 peripheral paths use the Arduino Renesas core) | Full lease/event/fault parity and unimplemented peripherals |
| Core | SAMD21, AVR subset | Target startup and status integration | Portable peripheral providers |

A provider row is not interchangeable with deep support. In particular, event routing
and PWM construction differ between MCU families, and a generic bus adapter still needs
a concrete board application to exercise the selected pins and peripheral instance.

## Isolation, boot, and recovery

- Rust module identity is dispatcher-owned, but modules share one privileged address
  space. Per-module MPU switching and unprivileged execution are not present.
- The signed boot/update state machine is fail-closed and persistent, but production
  bootloader slots, protected keys, and provisioning transports are board integrations.
- Kernel-object cleanup is reconciled and leak-checked. The nRF HAL invalidates
  generation-tagged sessions and quiesces peripheral activity before lease reassignment;
  equivalent behavior is incomplete on other ports.
- Application-owned static state still needs explicit lifecycle cleanup.

## Validation boundary

Hosted checks cover portable tests, formatting, dependency policy, package builds, and
cross-compilation. Device behavior remains target- and application-specific; a quiet or
absent device is never interpreted as success.
