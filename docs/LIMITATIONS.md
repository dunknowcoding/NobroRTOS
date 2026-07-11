# Limitations and support boundaries

This matrix is part of the product contract. “Builds” does not mean “deeply
supported,” declared timing is not a WCET proof, and a host simulation is not hardware
evidence. The machine-readable platform truth is
[`core/boards/platform_tiers.json`](../core/boards/platform_tiers.json).

## Application model

| Boundary | Current behavior | Practical consequence |
| --- | --- | --- |
| Scheduling | Cooperative, fixed-priority execution with response-time admission, measured budgets, fuel-bounded async, and containment after overruns | A callback that never yields still needs the deadline/watchdog interrupt to reset or recover it; there is no preemptive time-slicing profile |
| Timing evidence | Admission uses declared or measured budgets and pessimistic interference | Blocking terms and unmeasured platform/compiler paths are not formal WCET proofs |
| Async | No allocation; up to 32 reactor tasks per core; fixed timer/channel capacities | Channels retain one parked sender and receiver waker, so use them as single-producer/single-consumer primitives or add arbitration |
| Composition | One graph derives manifest, startup, task metadata, labels, and unambiguous mailbox grants | Capability kinds remain a closed bit set, and stable numeric module codes remain on wire formats |
| Project workflow | `nobro project` creates/imports, explains, builds, simulates, reports, and delegates HIL | Its generated Cargo program is a host graph. Hardware mode evaluates a selected repository firmware app; imports do not infer production budgets or interrupt/DMA ownership |

## Resources

The pinned minimal-workload baseline currently measures NobroRTOS at 16,080 bytes of
flash and 16 bytes of static RAM, versus 3,708/4,644 for Embassy and 1,324/16 for the
bare-metal baseline. The graph API reduces contract boilerplate but adds about 2.9 KiB
of flash in that baseline. These numbers are regression-gated, not universal forecasts;
re-run `python tools/measure_baselines.py` for the pinned targets and settings.

NobroRTOS deliberately spends more flash on admission, identity, quotas, recovery, and
evidence. Small applications for which those controls are unnecessary will usually be
smaller and simpler in Embassy or bare metal. Cross-platform CPU and energy comparisons
are not yet published because equivalent provider-level hardware measurements are not
available.

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
- Kernel-object cleanup is reconciled and leak-checked. DMA state, interrupt handlers,
  driver-owned statics, and hardware leases still require their platform adapters to
  quiesce and release them correctly.
- Stack and MPU fault paths have deep-platform negative HIL. That evidence does not
  imply equivalent isolation on provider or conformance ports.

## Evidence interpretation

Hosted CI covers host tests, format/lint, dependency policy, Miri, persistent fuzz
smoke, sanitizer, coverage, package builds, and cross-compilation. It cannot access the
lab. Hardware evidence is generated under ignored work roots and is never committed;
public claims report only sanitized verdicts. A quiet or absent endpoint is not converted
into a passing result.
