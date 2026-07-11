# Engineering Dossier

The evidence-grade internals: security model, interrupt/idle policy, measured
worst-case latencies, the audited unsafe surface, and the Wasm module-slot design.
Every number is reproducible by a named command.

## Security model

NobroRTOS treats security the way it treats everything else: as **explicit,
machine-checkable contracts**, not vibes. This page states what exists, what each
piece actually guarantees, and where the boundaries are.

### What firmware gets

| Mechanism | What it guarantees | Where |
| --- | --- | --- |
| **Capability grants** | admission derives grants; protected runtime operations are crate-private and `ModuleCtx`/foreign-host dispatch checks authority while recording trace evidence | `nobro_kernel` |
| **Quota and energy accounting** | admission seeds bounded object quotas; protected dispatch charges objects and executor poll duration feeds per-module energy accounting | `nobro_kernel`, `nobro_power` |
| **Peripheral leases** | exclusive, owner-checked access to buses/timers/radios; wrong-owner release is an error, verified by a 30k-op property test | `nobro_hal::lease` |
| **Bounded everything** | fixed-capacity mailboxes, pools, bridges; no heap = no heap exploits, no fragmentation | whole tree (`no_std`, no alloc) |

### The update trust chain

```
measure = SHA-256(image)
sig     = Ed25519(signing_key, domain || version || geometry || vectors || measure)
verify  → VerifiedSignedImage / reject
```

- `nobro_secure` verifies against a pinned Ed25519 public key and produces a private-field
  `VerifiedSignedImage` token only after image geometry, vectors, measurement, and rollback
  policy pass.
- `PersistentBootController` commits stage, first trial, confirm, and revert transitions
  through monotonic storage; storage failure is fail-closed.
- Rust tests pin signing vectors and reject altered metadata, vectors, image bytes, keys,
  versions, and persistent-state failures. The older host OTA preflight demo remains a
  symmetric-authentication simulation and is not evidence for this asymmetric boundary.
- Supporting pieces include the non-exporting protected-key backend contract and provisioning
  policy, `RollbackGuard`, `TamperSeal`, and a hash-chained `AuditLog`.

**Honest boundary:** the platform bootloader still owns protected key storage, image
writing, reset-vector handoff, and enforcement before untrusted application code runs.
The repository provides and tests the verification/state-machine core, not a complete
ROM-to-application secure-boot product.

### Diagnostics without disclosure

`NOBRO_*` reports expose *state*, not secrets: fixed-layout counters, statuses, and
checksums. Keys never appear in reports; the evidence pack redacts machine-specific
paths and environments by construction.

### Sandboxing direction (exploration)

The C-ABI module boundary has a **Wasm-style isolation exploration** (fixed linear
memory, bounds-checked marshaling, per-poll fuel) exercised by a host-side spike.
Today's C/C++ modules are trusted code behind capability grants — isolation claims
wait until a real runtime is embedded and verified.

### Reporting a vulnerability

Open a GitHub issue with the `security` label, or contact the maintainer via the
repository profile for anything sensitive. Include reproduction steps and the commit
hash. There is no bug-bounty program; there *is* a maintainer who treats a failing
security gate as a stop-ship.

## Interrupt and idle policy

The questions every RTOS should answer in writing: how long are interrupts masked,
can they nest, where does deferred work go, and what does the CPU do when idle. Here
are NobroRTOS's answers, with measurements where a number exists.

### Masking: how long, and by whom

Kernel primitives that must be atomic (peripheral leases) use `critical_section`
(PRIMASK on Cortex-M). The **longest masked window a kernel operation produces was
measured at 152 cycles = 2.4 µs @ 64 MHz** (max over 1000 runs; see
the measured-latency section below, reproduce with
`nobro_hw_eval.py wcet`). Pure-data kernel ops (mailbox, quota, alarms, capability
checks) run unmasked — they take `&mut self` and belong to one context.

Rule for contributors: if your code masks interrupts, it must be a bounded, data-only
region — no bus waits, no loops without a proven iteration cap.

### Nesting

NVIC preemption is left in its reset configuration: **all interrupts share one
priority level, so ISRs do not nest** in the demos and eval apps. This is a policy,
not a limitation — a design that needs nesting sets NVIC priorities explicitly and
owns the stacking consequences. The kernel makes no assumption either way because
its shared state is either single-context (`&mut`) or masked (leases).

### Deferred work (the "bottom half")

The pattern is: **ISRs record, the poll loop acts.** An ISR's only sanctioned
interactions are (a) latching hardware state and (b) pushing a `Message` into a
`Mailbox` / setting `EventFlags` for the main loop to consume. The measured cost
makes this cheap: a mailbox push+pop pair is 8 cycles; an EventFlags set+wait pair
is 11. There are no tasklets because there is nothing for them to run on — the
deadline loop IS the executor, and `nobro_classic::select2` gives multi-source waits
a bounded shape.

### Idle

Some measurement apps use deterministic delays. The production executor requires a
`PowerPlatform`: it programs the next wake before idle/low-power entry, accounts measured
poll energy, and runs fallible peripheral suspend/resume hooks. Lower-level insertion
points remain available to classic loops:

- the `idle` callback of `nobro_classic::select2` — put `cortex_m::asm::wfe` there;
- the main loop between polls — `wfe` wakes on any event/interrupt;
- timed sleep via the RTC (`rtc_sleep_demo`, hardware-verified) for scheduled wakeups.

This is an executor-owned idle path, not a preemptive idle task. Ports must implement the
actual clock/RTC/sleep operations and are responsible for wake-source correctness.

## Measured kernel-op latencies

The external question every RTOS dodges: *what do core operations actually cost, worst
case?* NobroRTOS measures instead of estimating. `kernel_wcet_demo` times each
operation with the Cortex-M DWT cycle counter, takes the **max over 1000 iterations**,
and seals the numbers in `NOBRO_WCET_REPORT` — reproduce with:

```bash
python tools/nobro_hw_eval.py wcet --profile s140   # or nosd
```

### Results (nRF52840 @ 64 MHz, release build, max of 1000 runs)

| Operation (hot path) | Max cycles | Max time |
| --- | ---: | ---: |
| Mailbox push + pop (kernel IPC) | 8 | 125 ns |
| EventFlags set + wait_any | 11 | 172 ns |
| Capability authorize (every host-service call) | 14 | 219 ns |
| Quota reserve + release | 71 | 1.1 µs |
| Lease acquire + release | 127 | 2.0 µs |
| Alarm schedule + pop_due | 290 | 4.5 µs |
| Longest interrupt-masked window (CS probe + lease pair) | 152 | 2.4 µs |

The app enforces ceilings (ops < 640 cycles, CS probe < 1280) so a regression fails
the hardware gate, not a code review.

### How to read these honestly

- **Measured max ≠ formal WCET.** On a cache-less, single-issue Cortex-M4 the spread
  between typical and worst observed is tiny, but this is an empirical bound on this
  silicon, stated as such.
- **Interrupt masking:** the longest masked window a kernel op produces was 2.4 µs in
  this probe. Poll-driven demo apps mask nothing else; if you add ISRs, your masked
  windows are your own additions on top of this floor.
- **No context switch row?** NobroRTOS drives modules through `init/poll` from a
  deadline-scheduled loop rather than preemptive stack switching — the scheduler-slot
  jitter (2 µs max) is measured separately by `resource_sched_demo` (scene A).

## Unsafe inventory

Every `unsafe` surface in the portable tree, its invariant, and who upholds it. The
rule enforced by CI (`tools/lint_gate.sh`, clippy `-D warnings` incl.
`missing_safety_doc`): **an `unsafe fn` without a `# Safety` section does not merge.**

### Why unsafe exists here at all

NobroRTOS talks to memory-mapped peripherals. Register access is inherently outside
the borrow checker; the design keeps every such access behind three fences:

1. **Leases** — `ResourceLease` makes peripheral ownership explicit and owner-checked
   (property-verified over 30k random operations).
2. **`# Safety` contracts** — every unsafe fn states its invariant (lease held,
   call-once, pin wiring, DMA-buffer liveness). Callers cite the contract, not vibes.
3. **Bounded surface** — apps and modules never touch registers; only the HAL and the
   mountable backends (`nobro_usb`, radio drivers) contain unsafe, and module code
   reaches hardware exclusively through bounded host services.

### Surface map (categories, invariants, upholder)

| Surface | Files | Invariant | Upheld by |
| --- | --- | --- | --- |
| Timebase / deadline timers | `timer.rs`, `deadline_timer.rs` | lease held; init once before timestamped APIs | app boot sequence (`init_timebase`) |
| PWM (servo + bank) | `pwm.rs` | PWM0 lease; one owner (Servo XOR Bank); DMA sequence buffers are `static` and only written via `addr_of_mut!` | lease + `# Safety` contracts |
| PPI / GPIOTE / EGU wiring | `ppi.rs`, `radio_sim.rs` | channel leases; wired pins only; call-once | eval apps' bring-up |
| Bus bring-up (SPIM/TWIM) | `spim_hw.rs`, `twim_hw.rs` | bus lease; board-wired pins; TWIM runs 9-pulse recovery first | `board.rs` pin constants + leases |
| Register snapshots (self-test) | `platform/nrf52840/inspect.rs` | peripheral powered; volatile reads only | self-test scenes |
| USB / radio backends | `nobro_usb/*`, app radio drivers | peripheral exclusivity; errata sequences | mount()-once pattern |
| Report seals | demo apps' `static mut NOBRO_*` | single-writer main loop; host reads via probe after halt | app structure (no ISR writers) |

### Reviewer checklist for new unsafe

- Which lease covers this peripheral? If none exists, add the `Resource` first.
- Is the fn callable twice? If not, say "call once" in `# Safety` and make init
  idempotent where cheap.
- Does a DMA engine read a buffer you're writing? Name the buffer and its lifetime.
- Could an ISR alias this data? If yes, it needs a critical section, not a comment.

## Wasm module slot (exploration)

**Status: exploration (P3).** This documents the boundary shape for running a WebAssembly
module in a NobroRTOS module slot and points at a runnable host-side stub
(`tools/wasm_slot_spike.py`). It is deliberately not a shipped feature - the goal is to
prove the calling convention and the bounded-execution story fit the existing C ABI before
committing to a runtime.

### Why this is easy to reason about

NobroRTOS already has a stable, pointer-light module boundary: the C ABI in
[`bindings/c/include/nobro_app.h`](../bindings/c/include/nobro_app.h). A module implements
two callbacks and reaches the world only through a handful of host services:

```
guest exports (kernel calls):   nobro_app_init() -> i32,  nobro_app_poll() -> i32
host imports (guest calls):      nobro_now_us() -> u64
                                 nobro_i2c_write(addr, tx*, len) -> i32
                                 nobro_i2c_write_read(addr, tx*, tx_len, rx*, rx_len) -> i32
                                 nobro_publish_imu(who, addr, ax..gz, temp) -> ()
```

That is almost exactly a Wasm module's shape: **exports** the runtime calls, **imports** the
host provides. The only real work is bridging pointers.

### The one hard part: pointers become linear-memory offsets

C passes `const uint8_t *tx` / `uint8_t *rx` as raw addresses. A Wasm guest has no access to
host addresses - it can only name **`i32` offsets into its own linear memory**. So the slot
adapter re-types the two I2C services at the boundary:

| C ABI (native module) | Wasm slot ABI (guest) |
| --- | --- |
| `nobro_i2c_write(addr, const u8* tx, u32 len)` | `nobro_i2c_write(addr, i32 tx_off, u32 len)` |
| `nobro_i2c_write_read(addr, tx*, tx_len, rx*, rx_len)` | `..., i32 tx_off, tx_len, i32 rx_off, rx_len` |
| `nobro_now_us() -> u64` | unchanged (scalar) |
| `nobro_publish_imu(...scalars...)` | unchanged (all scalar) |

When the guest calls an import, the host **copies `tx_len` bytes out of** linear memory at
`tx_off`, does the real bus transaction, and **copies the reply into** linear memory at
`rx_off` (after bounds-checking both against the guest's memory size). Only scalar values and
copied byte ranges cross the boundary - the guest never sees a host pointer, and the host
never trusts a guest pointer.

### Keeping the slot bounded (the whole reason to bother)

A Wasm slot must not reintroduce the unbounded behavior NobroRTOS exists to prevent:

- **Fixed linear memory.** The guest gets a single, fixed-size linear memory allocated once
  at admission (declared in the manifest, like any module's RAM budget). No `memory.grow`.
- **Fuel per poll.** Each `nobro_app_poll` runs under a step/fuel budget. Exceeding it aborts
  the cycle with a transient error (the same contract as a C module returning `<0`), so one
  bad module cannot stall the schedule. A real runtime (wasmtime fuel, wasm3 step limit)
  enforces this; the spike models it with a per-cycle fuel counter.
- **No host-symbol access.** The guest's import table is exactly the four services above -
  nothing reaches kernel internals. This is the same guarantee the C ABI documents ("never
  touches the kernel internals"), enforced structurally by the sandbox instead of by
  convention.
- **Static admission.** One precompiled module per slot; no dynamic module loading in the
  spike. The Wasm blob is a build input, admitted like any other module.

### How a slot compares to the two boundaries we already ship

| | USB backends | C module | **Wasm slot (spiked)** |
| --- | --- | --- | --- |
| Selection | `backend-*` cargo feature | link-time symbols + admission | precompiled blob + admission |
| Isolation | Rust type system | `extern "C"` discipline | sandbox (memory + imports) |
| Pointers | native refs | native pointers | linear-memory offsets |
| Bound | compile-time | compile-time budget | fixed memory + fuel/poll |

A Wasm slot sits closest to the C module (link/admit a unit that implements `init`/`poll`),
but adds a hardware-enforced sandbox and a fuel bound in exchange for an indirection cost.

### Runnable stub

`tools/wasm_slot_spike.py` models this end to end **without a real Wasm runtime**: a fixed
linear memory (a `bytearray`), a host that exposes only the four imports over offsets and
copies bytes in/out with bounds checks, a fuel budget per poll, and a pure-Python "guest"
that mirrors `bindings/c/examples/imu_module.c` (WHO_AM_I read, 14-byte burst, publish) using
*only* the import facade and its own memory. It runs init + N polls against a simulated
MPU-class device and emits `_work/evidence/wasm_slot.json`:

```bash
python tools/wasm_slot_spike.py            # run the slot, emit evidence
python tools/wasm_slot_spike.py --selftest # assert the boundary holds (gated)
```

The stub proves the calling convention and the bounded/marshaled boundary; swapping the
pure-Python guest for a wasm3/wasmtime instance that imports the same four functions is the
remaining, mechanical step if this graduates from a spike.
