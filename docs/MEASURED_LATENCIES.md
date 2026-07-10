# Measured Kernel-Op Latencies

The external question every RTOS dodges: *what do core operations actually cost, worst
case?* NobroRTOS measures instead of estimating. `kernel_wcet_demo` times each
operation with the Cortex-M DWT cycle counter, takes the **max over 1000 iterations**,
and seals the numbers in `NOBRO_WCET_REPORT` — reproduce with:

```bash
python tools/nobro_hw_eval.py wcet --profile s140   # or nosd
```

## Results (nRF52840 @ 64 MHz, release build, max of 1000 runs)

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

## How to read these honestly

- **Measured max ≠ formal WCET.** On a cache-less, single-issue Cortex-M4 the spread
  between typical and worst observed is tiny, but this is an empirical bound on this
  silicon, stated as such.
- **Interrupt masking:** the longest masked window a kernel op produces was 2.4 µs in
  this probe. Poll-driven demo apps mask nothing else; if you add ISRs, your masked
  windows are your own additions on top of this floor.
- **No context switch row?** NobroRTOS drives modules through `init/poll` from a
  deadline-scheduled loop rather than preemptive stack switching — the scheduler-slot
  jitter (2 µs max) is measured separately by `resource_sched_demo` (scene A).
