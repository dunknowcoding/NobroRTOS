# NobroRTOS Porting Guide

This guide describes how to add a board family or platform without weakening
the core architecture.

## Porting Checklist

1. Add or extend a HAL platform module.
2. Define a board descriptor.
3. Add memory layout files.
4. Add one Cargo feature per boot layout.
5. Export a board profile report.
6. Add host-side checks for feature selection and report constants.
7. Add stub adapters before real device integration when possible.

## Platform Port

A platform port implements the HAL traits needed by apps and adapters:

- clock and monotonic time
- deadline timer
- event capture
- resource leases
- bus access
- PWM or actuator backend
- board inspection snapshots

Use platform-specific names only inside the platform module. App and adapter
APIs should use portable HAL terms.

## Board Descriptor

A board descriptor should include:

- board name and stable hash
- app flash start
- flash and RAM budgets
- sample-pool slots
- max module count
- critical pins
- servo defaults
- bootloader compatibility notes

The HAL also exposes a `BoardPackage` contract. A package combines the board
descriptor with boot layout, app flash range, RAM range, capacity budgets, and
critical pins. New board ports should make this package valid before app
assembly depends on the board.

Example shape:

```rust
pub const BOOT_PROFILE: BootProfile = BootProfile::new(
    BootLayout::NoSoftDevice,
    0x1000,
    1020 * 1024,
    0x2000_0000,
    256 * 1024,
);

pub const ACTIVE_BOARD_PACKAGE: BoardPackage = BoardPackage::new(
    Board::PLATFORM_ID,
    Board::BOARD_ID,
    BOOT_PROFILE,
    Board::CAPACITY,
    BoardPins::new(LED_PIN, SERVO_PWM_PIN, MVK_TRIGGER_PIN),
);
```

## Feature Rules

Each firmware build should select exactly one platform feature and one board
feature. Adapter crates should disable default HAL features and forward the
selected board feature explicitly.

Good:

```toml
airon-hal = { path = "../../crates/airon-hal", default-features = false }
board-promicro-nosd = ["airon-hal/board-promicro-nosd"]
```

Avoid hidden default board selection in adapter crates.

## Board Fixtures

Each supported board feature should be mirrored in `BOARD_PROFILE_FIXTURES` and
`BOARD_PACKAGE_FIXTURES`. Profile fixtures let host review tools inspect board
identity, capacity, critical pins, and servo defaults. Package fixtures add boot
layout, memory ranges, and package validation without rebuilding the HAL for
every board feature.

Apps that enable `airon-kernel/hal-profile` can derive `SystemProfile` from the
active `BoardPackage`. This is the preferred path for admission checks because
the manifest budget then follows the selected board feature.

## Acceptance Gates

Before a board port becomes a recommended target, it should provide:

- host build coverage for its feature set
- board profile report generation
- valid `BoardPackage` contract
- manifest and admission report compatibility
- linker layout review
- resource lease coverage for shared peripherals
- at least one app composition that exercises timers, bus, and reports

## Naming

Public product documentation uses NobroRTOS. Existing Rust crate names retain
the `airon-*` prefix until a coordinated crate migration is performed.
