# Measured resource baselines (RES-01)

Equivalent workloads, multiple implementations, identical build settings — so
"heavier / lighter" claims about NobroRTOS are numbers, not adjectives.

## The simple workload (equivalent observable behavior)

On an nRF52840, with TIMER0 free-running at 1 MHz as the time source:

1. **control** — 50 Hz periodic task: toggles P0.15 via raw GPIO registers and
   counts ticks. (Raw registers everywhere, so GPIO driver differences don't
   pollute the comparison.)
2. **sensor** — 10 Hz periodic task: produces a synthetic sample
   (`count * 3 + 7`) into the framework's message channel.
3. **consumer stage** — drains the channel and folds samples into an exponential
   moving average. Embassy expresses this as a separately spawned async task;
   the bare-metal and NobroRTOS specimens drain it in their control/poll path.

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
python tools/internal/comparison/measure_baselines.py
python tools/internal/comparison/measure_baselines.py --breakdown
```

Output: `_work/evidence/baselines.json` (flash text+rodata+data, static RAM
data+bss, worst-case stack for the Nobro app via `static_budget`, source line
counts) and a plain table. Thresholds for `nobro-min` live in
`tools/internal/comparison/baseline_budgets.json`; CI fails when a regression exceeds them.

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

The simple NobroRTOS graph above has 2 admitted periodic tasks. To test the narrower claim that NobroRTOS cannot handle
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
| `embassy-complex/` | Embassy 0.7 | five async tasks, tickless `Ticker`s, and three one-slot `embassy-sync` channels |
| `freertos-complex/` | FreeRTOS Kernel 11.3.0 | five statically allocated tasks, three static queues, delay-until scheduling, no heap |

### Measured (2026-07-12, `thumbv7em-none-eabihf`, opt-level=z, LTO=fat)

| Impl | flash (B) | static RAM (B) | source lines |
| --- | --- | --- | --- |
| baremetal-min (2 tasks) | 1324 | 16 | 75 |
| nobro-graph-min (2 tasks) | 19124 | 16 | 114 |
| baremetal-complex (5 tasks) | 1436 | 16 | 109 |
| nobro-graph-complex (5 tasks) | 19352 | 16 | 142 |
| embassy-complex (5 tasks) | 4328 | 4724 | 113 |
| embassy-complex-tuned (5 tasks, 1 KiB arena) | 4320 | 1652 | 113 |
| freertos-complex (5 tasks) | 6724 | 3828 | 183 |

**The point, honestly both ways:** NobroRTOS carries a fixed framework cost
(~18 KB flash) that a bare-metal loop does not — for a 2-task blinker, that is
a heavy tank, and we say so. But its *marginal* cost as complexity grows is
bounded: going from 2 tasks to a 5-stage pipeline with fan-out and one-slot
backpressure adds **+228 B flash and +28 source lines** (three tasks, three
channels, and explicit one-slot edge state), while ELF static RAM stays 16 B.
Bare metal grows +112 B / +34 lines but every stage,
deadline, and backpressure flag is hand-maintained with no admission or
isolation. This is a footprint/authoring comparison only; it makes no claim
about runtime behavior or correctness.

The Embassy and FreeRTOS rows are now real builds, not declarations. The FreeRTOS
specimen vendors the exact official V11.3.0 kernel subset and records its commit and
MIT license; only the benchmark's own `src/` C/Rust/configuration lines count as user
source. Static RAM includes each framework's statically reserved task/channel storage.
Instrumentation-only lines bracketed by `BENCH_INSTRUMENTATION` are excluded from
authoring counts. The multi-device physical workflow remains Wave 68.

### Runtime/resource campaign (Wave 61)

The five RAM-linked ELFs were loaded through J-Link without writing application flash,
run for the same 5-second host interval, and the protected target was returned to UF2
DFU. All five met release counts; observable spreads were control=2, fusion=3,
radio=1, drops=4. Task-work cycles are DWT-instrumented at each framework's task/poll
boundary; main-stack peak uses a debugger-loaded canary.

| Impl | task-work cycles | work/elapsed | main-stack peak | idle/sleep residence | mean/max jitter |
| --- | ---: | ---: | ---: | ---: | ---: |
| bare metal | 15,326 | 0.0047% | 96 B | 0% (busy poll) | 0.48/1 us |
| NobroRTOS | 1,512,069 | 0.4677% | 9,492 B | 98.3592% (deadline WFI) | 79.33/1,783 us |
| Embassy | 1,323,799 | 0.4094% | 216 B | 99.4177% | 79.11/247 us |
| Embassy, tuned 1 KiB arena | 1,322,724 | 0.4090% | 216 B | 99.4034% | 78.65/248 us |
| FreeRTOS | 1,084,829 | 0.3356% | 168 B | 99.4066% | 4.37/26 us |

FreeRTOS additionally measured a 156 B peak among its five task stacks, from 2,944 B
reserved for task+idle stacks (already represented in its static RAM). The result does
**not** show NobroRTOS using fewer resources: it used about 15% more task-work cycles
than Embassy, 40% more than FreeRTOS, much more peak main stack, less residence, and
worse jitter. The reusable nRF deadline-WFI provider nevertheless replaces the old
no-op hook and keeps the normal Nobro specimen to 19,352 B / 142 lines. The software
residency estimate uses explicit arbitrary coefficients (non-resident=1.0,
idle/sleep-resident=0.1), so it is not measured current or joules. Direct electrical
energy remains equipment-gated. Instrumentation increases RAM/stack slightly; the
footprint table reports uninstrumented production-shaped builds.

## Profile isolation, symbol-attributed (Wave 60)

Footprint is attributed to crates at the symbol level (`llvm-nm --size-sort`),
and profiles are **dependency sets enforced by that attribution**, not feature
flags that could drift. The `minimal` profile forbids the service crates
(`nobro_secure`, `nobro_crypto`, `nobro_net`, `nobro_storage`,
`nobro_database`, `nobro_ai`, `nobro_ml`, `nobro_nn`) from appearing in flash
at all — selecting a service = adding its crate; not selecting it = it does not
exist in the binary.

The check now covers the COMPLEX build too. `nobro-graph-complex` (the 5-stage
fan-out+backpressure pipeline) is `minimal_profile_clean: true`: its 19,352 B
attribute to `nobro_kernel` (7,314 B) + application/provider/misc code (9,486 B) +
compiler-builtins/core only — **zero** accidental service-crate linkage. Adding
tasks does not drag in unselected services, so the "minimal profile" claim
survives complexity. `managed` (+secure/storage/database) and `assured`
(+net/fleet/ai) are the larger dependency sets, each provable by the same
symbol-level attribution rather than by trusting a feature flag.
