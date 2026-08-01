# Kernel-lite Candidate Witnesses

This directory answers one narrow question: can an exact compiler represent the
bounded admission, mailbox, wrapping-deadline, and telemetry-checksum model on a
candidate architecture? It does not implement or claim a board HAL, interrupt
ownership, clock accuracy, isolation, preemption, power behavior, or physical
recovery.

`nobro_kernel_lite.c` is allocation-free and makes the target's addressable storage
unit explicit. This matters on TI C28x, where `CHAR_BIT` is 16 rather than 8. The
STM32C011 files add a minimal generic vector/link image for the exact C0 memory
shape. The FPGA files simulate a four-task bounded round-robin primitive; they are
not an MCU port or a synthesized board bitstream.

The candidate gate separates artifact types:

| Candidate | Artifact |
| --- | --- |
| classic 8051 and STM8S103 | SDCC firmware image |
| STM32C011F6 | generic Cortex-M0+ ELF image |
| CH32V003F4P6 | exact RV32EC ISA object; no vendor startup/HAL claim |
| MSP430G2553 | GCC firmware image |
| PIC16F18855, PIC24FJ64GA002, PIC32MX270F256B | exact XC compiler/DFP image |
| TMS320F280049C | C28x object with 16-bit storage-unit contract |
| FPGA | RTL simulation plus a separate RV32IMC soft-core ABI object |

Run metadata-only validation everywhere:

```text
python tools/checks/platforms/check_kernel_lite_candidates.py
```

`--open-source` requires SDCC, Arm GCC, Icarus Verilog, and VVP on `PATH` (or
their `NOBRO_*` executable variables). `--local-full` additionally requires exact
RISC-V, MSP430, Microchip XC/DFP, TI C2000, and optional IAR 8051 paths through
the named environment variables. Toolchains and generated artifacts stay outside
the repository.
