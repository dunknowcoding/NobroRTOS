# HAL/SAL Contract

NobroRTOS describes one exact native composition with the same versioned
capability vocabulary in Rust, the platform matrix, and the machine-readable
HAL registry. A capability is never inferred from a board family name, a
successful build, or a nearby implementation.

The canonical registry is
[`core/boards/hal_contract_v2.json`](../core/boards/hal_contract_v2.json).
The compiled contract is `nobro_hal::HardwareCapabilityDeclaration`.

## Capability states and profiles

Every capability is in exactly one state for each native composition:

- `required`: selected by the profile but not implemented yet;
- `supported`: implemented and named by an exact compiled witness;
- `hardware-inapplicable`: the exact hardware cannot provide it;
- `unimplemented`: applicable in principle but not implemented.

A `deep` profile is reserved for an exact composition whose `unimplemented`
set is empty: every capability exposed by that board is supported, while
features absent from the hardware are `hardware-inapplicable`. An attachable
external module such as a servo is not a board parity requirement unless the
exact composition includes it. A `constrained` profile is an intentionally
partial composition and keeps every applicable gap visible. Both are
versioned. A platform claim must equal the registry's supported set, and the
supported set must equal its compiled witness set.

The vocabulary separates timebase, deadline, event, DMA completion, GPIO, IRQ,
UART, byte I/O, ADC, PWM, servo, pulse, I2C, SPI, USB, watchdog, RTC, flash,
reset, power, cache, multicore, and lease capabilities. There is no generic
`bus` capability: declare the exact transport. Scheduling remains a
composition of timebase, deadline, event, interrupt, and ownership facilities,
not a substitute capability.

## Lifecycle boundary

`nobro_device::ProviderLifecycle` provides the common owned and fallible
lifecycle state machine. Its generation-tagged sessions reject stale access.
Operations carry absolute deadlines and monotonic partial progress. Completion,
cancellation, faults, resets, release, and recovery produce typed receipts.
Quiesce and release fail while an operation remains active, so cleanup cannot
silently discard in-flight ownership.

Board-varying adapters classify these lifecycle dimensions independently in
`core/boards/feature_providers.json`. A `limited` entry means that adapter has
not yet adopted the v2 receipt even if its existing call is bounded.

## Migration from the former names

| Former declaration | Contract v2 |
| --- | --- |
| `clock` | `timebase` |
| `event_capture` | `event` |
| `servo_pwm` | `servo` for angle/pulse semantics, otherwise `pwm` |
| `bus` | exact `i2c`, `spi`, `uart`, or `byte_io` capability |
| scheduling capability | declare the granular facilities it composes |
| resource ownership | `lease` plus the exact peripheral capability |

When adding a native composition:

1. select or add a versioned profile; use `deep` only when no applicable
   capability remains `unimplemented`;
2. classify all capabilities in the HAL registry;
3. implement `HalCompatibility` and its compile-time validity assertions;
4. make the platform claims equal the supported set;
5. bind each claim to exact host or target-build evidence;
6. run `check_hal_contract.py --selftest` and
   `check_platform_tiers.py --selftest`.

Host tests validate contract behavior and target builds validate compilation.
Neither is physical evidence. Physical promotion remains scoped to the exact
board, firmware, peripheral, wiring, and observed behavior.
