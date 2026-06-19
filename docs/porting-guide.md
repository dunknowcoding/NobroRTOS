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

Example shape:

```rust
pub const BOARD: BoardDesc = BoardDesc {
    name: "promicro-nrf52840",
    app_flash_start: 0x1000,
    capacity: BoardCapacity {
        flash_budget_bytes: 80 * 1024,
        ram_budget_bytes: 32 * 1024,
        sample_pool_slots: 8,
        max_modules: 16,
    },
    pins: BoardPins {
        servo_pin: 24,
        led_pin: 15,
        mvk_trigger_pin: 17,
    },
};
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

## Acceptance Gates

Before a board port becomes a recommended target, it should provide:

- host build coverage for its feature set
- board profile report generation
- manifest and admission report compatibility
- linker layout review
- resource lease coverage for shared peripherals
- at least one app composition that exercises timers, bus, and reports

## Naming

Public product documentation uses NobroRTOS. Existing Rust crate names retain
the `airon-*` prefix until a coordinated crate migration is performed.
