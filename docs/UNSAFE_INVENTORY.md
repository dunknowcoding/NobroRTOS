# Unsafe Inventory

Every `unsafe` surface in the portable tree, its invariant, and who upholds it. The
rule enforced by CI (`tools/lint_gate.sh`, clippy `-D warnings` incl.
`missing_safety_doc`): **an `unsafe fn` without a `# Safety` section does not merge.**

## Why unsafe exists here at all

NobroRTOS talks to memory-mapped peripherals. Register access is inherently outside
the borrow checker; the design keeps every such access behind three fences:

1. **Leases** — `ResourceLease` makes peripheral ownership explicit and owner-checked
   (property-verified over 30k random operations).
2. **`# Safety` contracts** — every unsafe fn states its invariant (lease held,
   call-once, pin wiring, DMA-buffer liveness). Callers cite the contract, not vibes.
3. **Bounded surface** — apps and modules never touch registers; only the HAL and the
   mountable backends (`nobro_usb`, radio drivers) contain unsafe, and module code
   reaches hardware exclusively through bounded host services.

## Surface map (categories, invariants, upholder)

| Surface | Files | Invariant | Upheld by |
| --- | --- | --- | --- |
| Timebase / deadline timers | `timer.rs`, `deadline_timer.rs` | lease held; init once before timestamped APIs | app boot sequence (`init_timebase`) |
| PWM (servo + bank) | `pwm.rs` | PWM0 lease; one owner (Servo XOR Bank); DMA sequence buffers are `static` and only written via `addr_of_mut!` | lease + `# Safety` contracts |
| PPI / GPIOTE / EGU wiring | `ppi.rs`, `radio_sim.rs` | channel leases; wired pins only; call-once | eval apps' bring-up |
| Bus bring-up (SPIM/TWIM) | `spim_hw.rs`, `twim_hw.rs` | bus lease; board-wired pins; TWIM runs 9-pulse recovery first | `board.rs` pin constants + leases |
| Register snapshots (self-test) | `platform/nrf52840/inspect.rs` | peripheral powered; volatile reads only | self-test scenes |
| USB / radio backends | `nobro_usb/*`, app radio drivers | peripheral exclusivity; errata sequences | mount()-once pattern |
| Report seals | demo apps' `static mut NOBRO_*` | single-writer main loop; host reads via probe after halt | app structure (no ISR writers) |

## Reviewer checklist for new unsafe

- Which lease covers this peripheral? If none exists, add the `Resource` first.
- Is the fn callable twice? If not, say "call once" in `# Safety` and make init
  idempotent where cheap.
- Does a DMA engine read a buffer you're writing? Name the buffer and its lifetime.
- Could an ISR alias this data? If yes, it needs a critical section, not a comment.
