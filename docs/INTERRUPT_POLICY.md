# Interrupt & Idle Policy

The questions every RTOS should answer in writing: how long are interrupts masked,
can they nest, where does deferred work go, and what does the CPU do when idle. Here
are NobroRTOS's answers, with measurements where a number exists.

## Masking: how long, and by whom

Kernel primitives that must be atomic (peripheral leases) use `critical_section`
(PRIMASK on Cortex-M). The **longest masked window a kernel operation produces was
measured at 152 cycles = 2.4 µs @ 64 MHz** (max over 1000 runs; see
[MEASURED_LATENCIES.md](MEASURED_LATENCIES.md), reproduce with
`nobro_hw_eval.py wcet`). Pure-data kernel ops (mailbox, quota, alarms, capability
checks) run unmasked — they take `&mut self` and belong to one context.

Rule for contributors: if your code masks interrupts, it must be a bounded, data-only
region — no bus waits, no loops without a proven iteration cap.

## Nesting

NVIC preemption is left in its reset configuration: **all interrupts share one
priority level, so ISRs do not nest** in the demos and eval apps. This is a policy,
not a limitation — a design that needs nesting sets NVIC priorities explicitly and
owns the stacking consequences. The kernel makes no assumption either way because
its shared state is either single-context (`&mut`) or masked (leases).

## Deferred work (the "bottom half")

The pattern is: **ISRs record, the poll loop acts.** An ISR's only sanctioned
interactions are (a) latching hardware state and (b) pushing a `Message` into a
`Mailbox` / setting `EventFlags` for the main loop to consume. The measured cost
makes this cheap: a mailbox push+pop pair is 8 cycles; an EventFlags set+wait pair
is 11. There are no tasklets because there is nothing for them to run on — the
deadline loop IS the executor, and `nobro_classic::select2` gives multi-source waits
a bounded shape.

## Idle

Demo apps idle with `cortex_m::asm::delay` for determinism during eval. For power,
the sanctioned idle insertion points are:

- the `idle` callback of `nobro_classic::select2` — put `cortex_m::asm::wfe` there;
- the main loop between polls — `wfe` wakes on any event/interrupt;
- timed sleep via the RTC (`rtc_sleep_demo`, hardware-verified) for scheduled wakeups.

A kernel-owned idle *task* does not exist by design (nothing preempts, so there is
nothing to yield from). If a future preemptive profile lands, the idle hook becomes
kernel API; until then, claiming one would be decoration.
