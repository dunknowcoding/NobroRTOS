# NobroRTOS vs FreeRTOS

FreeRTOS is the most-deployed embedded kernel and excellent at what it does: tiny,
portable, simple. NobroRTOS targets the exact places FreeRTOS makes you do the hard,
error-prone work yourself. Every "how" below is a shipped mechanism, not a promise.

| FreeRTOS pain point (widely reported) | NobroRTOS answer | Mechanism |
| --- | --- | --- |
| **Manual stack sizing** per task; guess wrong → runtime stack overflow | RAM is **bounded at build time**; the kernel refuses to admit a module that exceeds its budget | `SystemBudget` + `QuotaLedger` admission (cross-module budget enforced), `stack_guard` canary verified on HW |
| **Heap fragmentation** — free RAM becomes unusable over time | **No heap at all** — every structure is fixed-capacity `no_std` | whole tree is no-alloc; `#![forbid(unsafe_code)]` in the safe layers |
| **No memory protection / task isolation** — a bug anywhere corrupts anything | **MPU regions** + **capability grants** so a module only touches what it's granted | MPU read-only region + MemManage recovery (verified on HW), `CapabilityGrantTable` |
| **Priority inversion** — manual mutex/inheritance config, missed deadlines | Peripherals are **leased**, not locked — the kernel arbitrates ownership, so there is no inversion to configure | `ResourceLease` (acquire/conflict-reject/wrong-owner), verified on HW (radio/PWM/SPI) |
| **Data races** on shared peripherals | The **lease is exclusive**; a second owner is rejected at compile-of-intent + runtime | lease conflict tests pass on HW |
| **Manual driver rewrite** for each new chip | A new servo/motor/sensor **brand is data**, not a driver rewrite | `nobro-device` catalog + `SensorRegistry` (WHO_AM_I identity), verified on HW (SG90 → PWM) |
| **Minimal abstraction** collapses at scale; ad-hoc refactors are painful | A **structured control plane** from day one | manifest / admission / quota / capability / watchdog / health / recovery — property-tested |
| **C only**, safety is the developer's job | Rust core: memory/type safety at **compile time**; the safe crates forbid `unsafe` | portable core builds for 6 MCU families; C/C++ ABI for those who want it |
| **Hard to verify** | Every kernel claim is **property-tested** (200 seeds × 300 ops) or read back from a `NOBRO_*` report on real silicon | `property_tests`, on-hardware self-certifying reports |

## Migrate without a rewrite: `nobro-classic`

Keep your FreeRTOS mental model. `nobro-classic` gives the familiar primitives —
`Queue<T, N>`, `Semaphore` (binary/counting), `Mutex`, `SoftwareTimer` — but fixed
capacity, **no heap**, `#![forbid(unsafe_code)]`, sized at compile time:

```rust
use nobro_classic::{Queue, Semaphore, SoftwareTimer};
let mut q: Queue<Event, 8> = Queue::new();      // xQueueCreate(8, …), no heap
q.send(ev);                                     // xQueueSend
let mut sem = Semaphore::counting(4, 0);        // xSemaphoreCreateCounting
let mut t = SoftwareTimer::new(1_000, true);    // xTimerCreate(auto-reload)
```

The API maps 1:1 to FreeRTOS calls (see the crate's doc table), so a port is a rename,
not a redesign — and you immediately gain bounded RAM, no fragmentation, and (for
peripherals) lease arbitration instead of manual mutexes.

## Where FreeRTOS still wins today

Honesty matters: FreeRTOS ports **more architectures** (40+ vs our 6 families today),
has a **massive ecosystem** and commercial support, and true **preemptive** multitasking
(NobroRTOS favors bounded run-to-completion + a deadline scheduler). NobroRTOS's bet is
that safety, bounded guarantees, and data-first extensibility matter more as products
grow — exactly where FreeRTOS users report the most pain.
