# Core resource measurements

These measurements cover the byte-machine Core dispatcher and the two public
fixtures in `ports/byte/examples`. They are linked-image measurements, not
estimates, and do not include a board startup, interrupt vector, hardware
driver, or application code beyond the named fixture.

| Toolchain | Fixture | Program bytes | Kernel data bytes | Total RAM bytes |
| --- | ---: | ---: | ---: | ---: |
| SDCC 4.5.0 #15242, MCS-51 | minimal | 504 | 7 | not reported by this gate |
| SDCC 4.5.0 #15242, MCS-51 | useful | 725 | 12 | not reported by this gate |
| IAR Embedded Workbench for 8051 10.20.1.5333 | minimal | 312 | 7 | 57 |
| IAR Embedded Workbench for 8051 10.20.1.5333 | useful | 526 | 12 | 67 |

The minimal fixture has one periodic task and no mailbox or hooks. The useful
fixture has two periodic tasks, a two-byte mailbox, idle hook, and watchdog
hook. The configuration reserves at least 1 KiB of a 2 KiB program device for
startup, drivers, and the real application.

The public SDCC gate regenerates both fixtures, runs a functional host harness,
links both MCS-51 images, parses the linker memory report, and rejects a minimal
image above 512 program/16 kernel-data bytes or a useful image above 1024
program/32 kernel-data bytes.

No claim against another RTOS follows from these numbers. A fair comparison
requires the same MCU, compiler options, timer and interrupt binding, workload,
deadline semantics, and included functionality. Peer numbers will only be
published with that complete protocol.
