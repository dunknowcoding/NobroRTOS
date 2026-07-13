# Limitations and support boundaries

This matrix is part of the product contract. “Builds” does not mean “deeply
supported,” declared timing is not a WCET proof, and a host simulation is not hardware
evidence. The machine-readable platform truth is
[`core/boards/platform_tiers.json`](../core/boards/platform_tiers.json).
Application and library integration status is separately gated by
[`core/ecosystem/integration_matrix.json`](../core/ecosystem/integration_matrix.json),
including rows that are still absent.

## Application model

| Boundary | Current behavior | Practical consequence |
| --- | --- | --- |
| Scheduling | Cooperative, fixed-priority execution with response-time admission, measured budgets, fuel-bounded async, and containment after overruns | A callback that never yields still needs the deadline/watchdog interrupt to reset or recover it; there is no preemptive time-slicing profile |
| Timing evidence | Admission uses declared or measured budgets and pessimistic interference | Blocking terms and unmeasured platform/compiler paths are not formal WCET proofs |
| Async | No allocation; up to 32 reactor tasks per core; fixed timer/channel capacities | Channels retain one parked sender and receiver waker, so use them as single-producer/single-consumer primitives or add arbitration |
| Composition | One graph derives manifest, startup, task metadata, labels, and unambiguous mailbox grants | Capability kinds remain a closed bit set, and stable numeric module codes remain on wire formats |
| Project workflow | `nobro project` creates/imports, explains, builds, simulates, and reports; `nobro firmware` generates a real nRF `no_std` crate and admission workload from one declaration | Production generation currently covers explicit nRF52840 S140/no-SoftDevice layouts. It does not yet bind application behavior to arbitrary providers, flash generated firmware, or infer measured WCET/interrupt/DMA ownership |
| Arduino authoring | Fixed-capacity `NobroApp` declares tasks/channels and previews admission with plain errors; all examples compile on AVR, RA4M1, ESP32-S3, and ArduinoNRF | The facade does not embed the Rust executor or prove timing; production execution and physical behavior still require generated/core firmware, providers, and HIL |

## Resources

Admission, identity, quotas, recovery, and evidence have real flash, RAM, stack, and CPU
costs. Small applications that do not need those controls may be smaller with a simpler
executor or a direct loop. Size and timing depend on the selected profile, toolchain,
target, and workload; measure the final application rather than treating a specimen as a
universal forecast. Static RAM is not total RAM, so deployment review must include stack
high-water and bounded dynamic arenas. Direct current or energy claims require calibrated
equipment; software coefficients are estimates only.

## Platform support

| Tier | Platforms | What is verified | Missing |
| --- | --- | --- | --- |
| Deep | nRF52840 | Portable core, granular providers, drivers, faults, reports, and state-restoring automated HIL | A scheduled lab runner is not attached to hosted CI |
| Provider | RP2350, ESP32-C3 | Real microsecond timebase implementation, target build, on-device provider check included in `all_pass`; local physical smoke recorded during Wave 50 | Deadline/PWM/bus/lease parity and integration into the reusable HIL collector |
| Conformance | ESP32-S3, RA4M1, SAMD21, AVR subset | Shared portable-core suite and target/package build where the toolchain is available | Portable HAL providers and deep fault/peripheral evidence |

A provider row is not interchangeable with the deep HAL. In particular, ESP32-C3 has
no PPI-equivalent event router and its PWM peripherals require a platform-specific
mapping. Physical smoke evidence does not promote either provider port to deep support.

## Isolation, boot, and recovery

- Rust module identity is dispatcher-owned, but modules still share one privileged
  address space. Per-module MPU switching and unprivileged execution are not present.
- The signed boot/update state machine is fail-closed and persistent, but a production
  board bootloader, slot writer, protected-key implementation, and factory provisioning
  transport remain platform integration work.
- Kernel-object cleanup is reconciled and leak-checked. The deep nRF HAL now invalidates
  generation-tagged sessions and stops peripheral DMA/interrupt routing before a lease
  is reassigned. Equivalent quiescence is not yet implemented on provider/conformance
  ports, and arbitrary module-owned static state still needs lifecycle-hook cleanup.
- Stack and MPU fault paths have deep-platform negative HIL. That evidence does not
  imply equivalent isolation on provider or conformance ports.

## Evidence interpretation

Hosted CI covers host tests, format/lint, dependency policy, Miri, sanitizer, coverage,
package builds, and cross-compilation. It cannot access the
lab. Hardware evidence is generated under ignored work roots and is never committed;
public claims report only sanitized verdicts. A quiet or absent endpoint is not converted
into a passing result.
