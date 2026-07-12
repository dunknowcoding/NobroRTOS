# Measured resource baselines (RES-01)

One workload, three implementations, identical build settings — so "heavier /
lighter" claims about NobroRTOS are numbers, not adjectives.

## The workload (identical in all three)

On an nRF52840, with TIMER0 free-running at 1 MHz as the time source:

1. **control** — 50 Hz periodic task: toggles P0.15 via raw GPIO registers and
   counts ticks. (Raw registers everywhere, so GPIO driver differences don't
   pollute the comparison.)
2. **sensor** — 10 Hz periodic task: produces a synthetic sample
   (`count * 3 + 7`) into the framework's message channel.
3. **consumer** — drains the channel and folds samples into an exponential
   moving average.

Every implementation exports the same observable state:
`#[no_mangle] static BASELINE_REPORT: [u32; 4]` =
`[control_ticks, samples_produced, filtered_value, queue_drops]`.

## The implementations

| Directory | What it is |
| --- | --- |
| `baremetal-min/` | the floor: `cortex-m-rt` + a hand-rolled poll loop, no framework |
| `nobro-min/` | NobroRTOS: manifest → admission → `KernelExecutor` + `ModuleCtx` mailbox |
| `embassy-min/` | Embassy: `embassy-executor` tasks + `embassy-time` timers + `embassy-sync` channel |

Each directory is a **standalone workspace** (excluded from the main tree) so
dependency graphs stay honest and pinned per implementation.

## Build settings (identical, pinned)

`release` profile with `opt-level = "z"`, `lto = "fat"`, `codegen-units = 1`,
`panic = "abort"` semantics via `panic-halt`, no defmt/RTT logging in any of
the three. Target `thumbv7em-none-eabihf`, flash at origin 0 (these binaries
are size specimens, never flashed by the hardware tools).

## Measuring

```bash
python tools/measure_baselines.py            # builds + sizes all three
python tools/measure_baselines.py --breakdown  # + marginal cost per Nobro service
```

Output: `_work/evidence/baselines.json` (flash text+rodata+data, static RAM
data+bss, worst-case stack for the Nobro app via `static_budget`, source line
counts) and a plain table. Thresholds for `nobro-min` live in
`tools/baseline_budgets.json`; CI fails when a regression exceeds them.

## Honesty rules

- The three programs do the same observable work, but they are *not* the same
  program: Embassy wakes from interrupts (tickless), the bare-metal loop and
  the Nobro executor poll. CPU/energy comparisons need the HIL rig, not this
  suite; this suite pins **flash / static RAM / stack / code size** only.
- Numbers are reported per-toolchain-version and never rounded toward a
  favored conclusion. If NobroRTOS is bigger, the number says so.
- No result from this suite may appear in public docs without its method
  sentence and toolchain pin alongside.

## The complex workload (Wave 59)

The simple workload above is 2 tasks. To answer "NobroRTOS can't handle
multiple complex tasks / Embassy is more flexible", the **complex** workload is
a five-stage pipeline with fan-out and backpressure:

```
fusion(100Hz) -> control(50Hz) -> radio(20Hz)
                              \-> storage(10Hz)   ;  diagnostics(5Hz)
```

| Directory | Impl | Notes |
| --- | --- | --- |
| `baremetal-complex/` | the floor | five stages, hand-rolled deadline schedule + hand-rolled channels/backpressure |
| `nobro-graph-complex/` | NobroRTOS graph API | five `TaskDecl`s + three `.channel()`s; manifest/admission/quotas/capabilities/startup/executor all derived |

### Measured (2026-07-12, `thumbv7em-none-eabihf`, opt-level=z, LTO=fat)

| Impl | flash (B) | static RAM (B) | source lines |
| --- | --- | --- | --- |
| baremetal-min (2 tasks) | 1324 | 16 | 75 |
| nobro-graph-min (2 tasks) | 19124 | 16 | 114 |
| baremetal-complex (5 tasks) | 1436 | 16 | 109 |
| nobro-graph-complex (5 tasks) | 20332 | 16 | 148 |

**The point, honestly both ways:** NobroRTOS carries a fixed framework cost
(~18 KB flash) that a bare-metal loop does not — for a 2-task blinker, that is
a heavy tank, and we say so. But its *marginal* cost as complexity grows is
tiny: going from 2 tasks to a 5-stage pipeline with fan-out and backpressure
adds only **+1208 B flash and +34 source lines** (the whole extra contract is
three tasks + three channels), and **static RAM stays 16 B** — the graph API
absorbs "multiple complex tasks" without the linear hand-written growth or heap
the critique assumes. Bare metal grows +112 B / +34 lines but every stage,
deadline, and backpressure flag is hand-maintained with no admission or
isolation. This is a footprint/authoring comparison only; it makes no claim
about runtime behavior or correctness.

**Embassy-complex and FreeRTOS-complex** equivalents are part of the full
comparative campaign (Wave 61): Embassy builds only with registry access
(measured offline-skipped here like `embassy-min`), and a FreeRTOS C specimen
needs its own port. Those rows are declared, not silently omitted.
