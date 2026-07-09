# Spike: a Wasm module slot behind the C ABI

**Status: exploration (P3).** This documents the boundary shape for running a WebAssembly
module in a NobroRTOS module slot and points at a runnable host-side stub
(`tools/wasm_slot_spike.py`). It is deliberately not a shipped feature - the goal is to
prove the calling convention and the bounded-execution story fit the existing C ABI before
committing to a runtime.

## Why this is easy to reason about

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

## The one hard part: pointers become linear-memory offsets

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

## Keeping the slot bounded (the whole reason to bother)

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

## How a slot compares to the two boundaries we already ship

| | USB backends | C module | **Wasm slot (spiked)** |
| --- | --- | --- | --- |
| Selection | `backend-*` cargo feature | link-time symbols + admission | precompiled blob + admission |
| Isolation | Rust type system | `extern "C"` discipline | sandbox (memory + imports) |
| Pointers | native refs | native pointers | linear-memory offsets |
| Bound | compile-time | compile-time budget | fixed memory + fuel/poll |

A Wasm slot sits closest to the C module (link/admit a unit that implements `init`/`poll`),
but adds a hardware-enforced sandbox and a fuel bound in exchange for an indirection cost.

## Runnable stub

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
