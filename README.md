<p align="center">
  <img src="docs/images/Nobro_full.png" alt="NobroRTOS" width="100%">
</p>

<h1 align="center">NobroRTOS Core</h1>

<p align="center"><strong>One tiny core. Serious timing. Your hardware.</strong></p>

<p align="center">
  <a href="https://github.com/dunknowcoding/NobroRTOS/actions/workflows/core.yml"><img alt="Core CI" src="https://github.com/dunknowcoding/NobroRTOS/actions/workflows/core.yml/badge.svg"></a>
  <a href="https://github.com/dunknowcoding/NobroRTOS/releases"><img alt="Release" src="https://img.shields.io/github/v/release/dunknowcoding/NobroRTOS"></a>
  <img alt="Arduino" src="https://img.shields.io/badge/Arduino-ready-00878F?logo=arduino&logoColor=white">
  <img alt="PlatformIO" src="https://img.shields.io/badge/PlatformIO-ready-F5822A?logo=platformio&logoColor=white">
  <img alt="Rust no_std" src="https://img.shields.io/badge/Rust-no__std-000000?logo=rust&logoColor=white">
  <a href="https://discord.gg/NrRrQKmT2"><img alt="Join Discord" src="https://img.shields.io/badge/Discord-join%20us-5865F2?logo=discord&logoColor=white"></a>
  <a href="https://www.youtube.com/@NiusRobotLab"><img alt="NiusRobotLab on YouTube" src="https://img.shields.io/badge/YouTube-NiusRobotLab-FF0000?logo=youtube&logoColor=white"></a>
  <a href="LICENSE"><img alt="GPL-3.0-only" src="https://img.shields.io/badge/license-GPL--3.0--only-blue"></a>
</p>

<p align="center">中文名：<strong>糯哥RTOS</strong> — 面向 AI、机器人、IoT 与智能控制的超轻量嵌入式实时操作系统。</p>

NobroRTOS Core gives small embedded products a clear answer to a hard
question: **what runs next, and can it finish on time?** It combines a compact
no-heap runtime with deadline-aware workload admission while leaving your
application in control of drivers, interrupts, timers, sleep, and the board
framework.

From an 8-bit controller to modern Arm and RISC-V boards, the idea stays simple:
declare bounded work, reject impossible schedules, and run admitted tasks
without an allocator or hidden task stacks.

## Why NobroRTOS Core

| | Advantage | What it means for your product |
| --- | --- | --- |
| ⚡ | **Tiny by design** | A measured MCS-51 fixture starts at 504 program bytes and 7 kernel-data bytes. |
| 🎯 | **Deadline-aware before launch** | Invalid timing, overload, duplicate identity, and conservative response-time failures are rejected before work begins. |
| 🧩 | **Fits your stack** | Use Arduino, PlatformIO, Rust `no_std`, or generate a compact C/assembly boundary from Python. |
| 🧠 | **A simple mental model** | Periodic and event tasks run to completion in explicit priority order. |
| 🔋 | **You own the idle path** | The core reports the next release; your board chooses `yield`, sleep, or a low-power instruction. |
| 🛡️ | **Bounded behavior** | Wrap-safe clocks, phase-preserving late release, fixed capacity, and limited dispatch keep behavior visible. |

## Pick your path

| You use | Start here | Best for |
| --- | --- | --- |
| Arduino IDE | [BlinkCore](examples/BlinkCore/BlinkCore.ino) | A first deadline-aware sketch in minutes |
| PlatformIO | [PlatformIO setup](docs/ARDUINO_PLATFORMIO.md#install) | Reproducible multi-board firmware |
| Python | [Contract generator](docs/PYTHON.md) | Compact C/assembly applications and image budget gates |
| Rust | [Rust guide](docs/RUST.md) | Dependency-free `no_std` firmware |

## Arduino in one screen

```cpp
#include <NobroRTOSCore.h>

using nobro::core::Scheduler;
using nobro::core::Task;

Scheduler<1> scheduler;

void sample(void *) {
    // Read a sensor, update control, then return.
}

void setup() {
    static const Task tasks[] = {
        Task::periodic(1, 0, 1000, 1000, 40, sample),
    };
    scheduler.begin(tasks, micros());
}

void loop() {
    scheduler.releaseDue(micros());
    scheduler.runReady();
    yield();
}
```

The same package compiles with representative AVR, SAMD21, Renesas RA4M1,
ESP32-S3, RP2040, and ESP8266 Arduino cores. Nothing in the library claims your
pins or peripherals.

## Rust in one screen

```rust
use nobro_rtos_core::{admit, AdmittedWorkload, CoreKernel, TaskContract};

const WORKLOAD: AdmittedWorkload<2> = match admit([
    TaskContract::periodic(1, 0, 1_000, 1_000, 80),
    TaskContract::event(2, 1, 40),
]) {
    Ok(value) => value,
    Err(_) => panic!("invalid workload"),
};

let mut kernel = CoreKernel::start(&WORKLOAD, timer_now_us());
kernel.release_due(timer_now_us());
while let Some(task) = kernel.take_next() {
    run_to_completion(task);
}
# fn timer_now_us() -> u32 { 0 }
# fn run_to_completion(_: usize) {}
```

The Rust crate is dependency-free, `#![no_std]`, allocation-free, and forbids
unsafe code.

## Python-powered tiny targets

```text
pip install nobro-rtos-core
nobro-core ports/byte/examples/useful/app.json --out generated
```

The generator validates the workload and emits C plus compiler-aware assembly
symbols for compact byte-addressed targets. Your firmware supplies startup,
time, task bodies, idle, and watchdog behavior.

## Proof, not slogans

Every release runs behavior tests, strict Rust linting, four embedded Rust
target builds, portable C/C++ tests, exact SDCC MCS-51 size limits, Arduino
compilation, PlatformIO compilation, package assembly, and a fail-closed source
boundary check.

| Public fixture | SDCC program | Kernel data |
| --- | ---: | ---: |
| One periodic task | 504 bytes | 7 bytes |
| Two tasks + mailbox + idle/watchdog hooks | 725 bytes | 12 bytes |

The [resource report](docs/CORE_BENCHMARKS.md) records the exact scope. These
figures describe the named NobroRTOS Core fixtures; they are not a blanket
superiority claim against a differently configured RTOS.

<p align="center">
  <img src="docs/images/Nobro_simple.png" alt="NobroRTOS Core" width="70%">
</p>

## Explore

- [Arduino and PlatformIO guide](docs/ARDUINO_PLATFORMIO.md)
- [Python generator guide](docs/PYTHON.md)
- [Rust user guide](docs/RUST.md)
- [8-bit C/assembly guide](ports/byte/README.md)
- [Measured resource envelope](docs/CORE_BENCHMARKS.md)

## Go further with NobroRTOS

Need capabilities beyond the public Core? We also provide customized NobroRTOS
cores with advanced features for product-specific timing, safety, connectivity,
AI, robotics, and hardware requirements. These advanced cores have passed a
simultaneous real-hardware stress campaign across 18 development boards.

<p align="center">
  <img src="docs/images/real_tests.jpg" alt="NobroRTOS advanced-core stress campaign across 18 development boards" width="92%"><br>
  <em>One simultaneous, multi-board NobroRTOS hardware stress campaign.</em>
</p>

Join the community on [Discord](https://discord.gg/NrRrQKmT2) to discuss your
project, request a customized solution, or build with other embedded creators.
For demos, tutorials, and new projects, visit
[NiusRobotLab on YouTube](https://www.youtube.com/@NiusRobotLab).

<p align="center">
  <a href="https://discord.gg/NrRrQKmT2"><img src="docs/images/discord_niusrobotlab.jpg" alt="Scan to join the NobroRTOS Discord" width="220"></a><br>
  <strong>Scan or click to join the NobroRTOS Discord</strong>
</p>

## License and commercial use

NobroRTOS Core is open source under **GPL-3.0-only**. Commercial use is allowed
under the license; redistribution and derivative-work obligations still apply.
Review [LICENSE](LICENSE) for the controlling terms.
