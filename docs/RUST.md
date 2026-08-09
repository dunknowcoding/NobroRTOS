# Rust user guide

`nobro-rtos-core` is a dependency-free `no_std` crate for static periodic and
event dispatch. It allocates no memory, owns no task stacks, and contains no
unsafe code. Your board binding supplies time, interrupts, idle behavior, and
task execution.

## Add the crate

Pin the release tag:

```toml
[dependencies]
nobro-rtos-core = { git = "https://github.com/dunknowcoding/NobroRTOS", tag = "v1.0.0" }
```

## Admit a workload

```rust
use nobro_rtos_core::{admit, AdmittedWorkload, TaskContract};

const WORKLOAD: AdmittedWorkload<3> = match admit([
    TaskContract::periodic(1, 0, 1_000, 1_000, 50),
    TaskContract::periodic(2, 2, 10_000, 5_000, 200).phase(500),
    TaskContract::event(3, 4, 80),
]) {
    Ok(workload) => workload,
    Err(_) => panic!("workload admission failed"),
};
```

Lower numeric priorities run first. Periodic contracts include period, finish
deadline, and WCET in microseconds. Event work has no fabricated arrival rate;
the application bounds its source and accounts for interference.

## Dispatch

```rust
use nobro_rtos_core::CoreKernel;

let mut kernel = CoreKernel::start(&WORKLOAD, board_now_us());
loop {
    kernel.release_due(board_now_us());
    while let Some(index) = kernel.take_next() {
        TASKS[index]();
    }
    board_idle_until(kernel.next_release_us(board_now_us()));
}
# fn board_now_us() -> u32 { 0 }
# fn board_idle_until(_: Option<u32>) {}
# static TASKS: [fn(); 3] = [|| {}, || {}, || {}];
```

An interrupt can publish a board-owned atomic flag. The single scheduler owner
then calls `mark_ready(index)`. The core deliberately does not invent a
portable critical section or low-power policy.

## Target checks

The release continuously checks Cortex-M0+, Cortex-M4F, Cortex-M33F, and
RV32IMAC builds using Rust 1.85. Other `no_std` targets can use the crate when
their board binding supplies the same 32-bit wrap-safe clock contract.
