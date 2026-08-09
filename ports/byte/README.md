# Core byte-machine backend

This backend carries the smallest NobroRTOS contract to byte-addressed 8-bit
targets without linking the Rust kernel or a managed runtime. It is a static,
run-to-completion dispatcher with one shared application stack, generated task
metadata, optional bounded mailbox storage, and optional idle/watchdog hooks.

Generation rejects utilization above one and applies a conservative
fixed-order, non-preemptive response-time check to every declared dispatch-start
deadline. The resulting bounds rely on truthful WCET budgets; a board must add
measured task and interrupt overhead before treating them as timing evidence.

The scheduler samples an atomic 8-bit tick supplied in `nobro_core_abi.now`.
Periods and dispatch-start deadlines are limited to 1..127 ticks, so wrapping
comparisons remain unambiguous. Choose the tick quantum to fit the application's
time horizon. Work budgets are admission inputs and compile away; they are not
unmeasured runtime WCET claims.

Generate a configuration:

```text
python tools/nobro_core.py ports/byte/examples/useful/app.json --out generated
```

Compile `src/nobro_core.c` with the generated `nobro_core_config.c` and
`nobro_core_app.c`. Application code implements one `<task>_step()` function per
declaration. Each step must return; there are no task stacks and no allocator.
When enabled, implement `nobro_app_idle()` and `nobro_app_watchdog()` as well.

## Resource ladder

Each contract names a 2, 4, or 8 KiB program capacity and reserves at least half
for application code. The generated receipt separately publishes the runtime
baseline budget that preserves this headroom and the maximum final linked-image
size after the application uses it. Selecting a larger capacity does not
silently enable a richer runtime: the current ladder keeps the measured Core byte
profile and reports an empty extension set. Deeper HAL or managed features must
first obtain their own exact target/map evidence; otherwise they remain absent
and consume no memory.

## C and assembly boundary

Every ABI call is `void name(void)`. Inputs and outputs use the six-byte
`nobro_core_abi` block, which avoids compiler-specific parameter registers. The
generator emits symbol/offset include files for SDCC, Keil C51, and IAR 8051.
Keil assembly calls the imported lower-case `nobro_core_*` symbols directly;
SDCC and IAR include aliases matching their tested assembler syntax. The
application and interrupt owner must use register bank 0 or preserve the active
bank according to its compiler's rules.

| Entry | Operation |
| --- | --- |
| `nobro_core_reset` | Rebase releases at `now` and clear queue/fault state |
| `nobro_core_dispatch` | Run every due task once in stable ID order and publish `sleep_ticks` |
| `nobro_core_post` | Enqueue `message`, or reject and latch `MAILBOX_FULL` |
| `nobro_core_take` | Dequeue into `message`, reporting `OK` or `EMPTY` |
| `nobro_core_recover` | Deterministically return to reset state |

All shared kernel fields are single bytes. On a byte-addressed target, an ISR
may publish `now` atomically; the application must not call dispatcher or queue
entry points concurrently from multiple contexts. A port that cannot guarantee
that ownership must wrap calls in its interrupt critical section.

This is an architecture backend, not a blanket board claim. A board still needs
an exact clock/timer, idle, watchdog, startup, linker, programming, and recovery
binding before its hardware support can be promoted.
