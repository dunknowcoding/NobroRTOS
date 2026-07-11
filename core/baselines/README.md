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
