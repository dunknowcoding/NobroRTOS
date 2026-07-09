# Cookbook: coming from Embassy

You already write `no_std` Rust with `embedded-hal` drivers and `async fn` tasks. This
cookbook is the mechanical recipe for re-expressing an Embassy app as a NobroRTOS system.
It complements the high-level [PORTING_FROM.md](PORTING_FROM.md) with copy-pasteable steps.

The one idea to internalize: **Embassy gives you an unbounded async runtime; NobroRTOS gives
you bounded modules that declare their budget and are admission-checked.** Your driver and
algorithm code transfers unchanged - the *system wiring* is what you rewrite, and it stays
small.

## 1. A task becomes a module

An Embassy task is an `async fn` spawned onto an executor. A NobroRTOS module is a struct
with `init()` + `poll(now_us)` and a declared contract.

```rust
// Embassy
#[embassy_executor::task]
async fn imu_task(mut i2c: I2c<'static>) {
    loop {
        let s = read_imu(&mut i2c).await;
        TELEMETRY.send(s).await;
        Timer::after_millis(20).await;
    }
}
```

```rust
// NobroRTOS: the loop body is poll(); the 20 ms cadence is a deadline, not a sleep.
struct ImuModule<B> { bus: B }

impl<B: BusSal> ImuModule<B> {
    fn poll(&mut self, _now_us: u64) -> Result<Option<Sample>, KernelError> {
        let s = self.read_imu()?;          // your embedded-hal driver, unchanged
        Ok(Some(s))                        // publish into a bounded pool - no .await
    }
}

// Declared once, checked at admission:
ModuleSpec::new(ModuleId::Sensor, Criticality::Driver)
    .requires(CapabilitySet::empty().with(Capability::Bus0).with(Capability::SamplePool))
    .memory(MemoryBudget::new(8 * 1024, 1024, 1))
    .deadline(DeadlineContract::new(20_000, 10)); // 20 ms period, 10 us jitter budget
```

Mapping table:

| Embassy | NobroRTOS |
| --- | --- |
| `#[task] async fn` | a module (`init` + `poll`) |
| `Timer::after(..).await` | a period in `DeadlineContract`, or an `Alarm` you check in `poll` |
| `Channel` / `Signal` | `Mailbox` (bounded) or a `SamplePool` handle |
| `select!` over events | drain a bounded mailbox / check alarms in `poll` |
| `Spawner` / executor owns the system | the kernel scheduler owns the system |
| heap / `Box`ed futures on hot paths | static pools, fixed capacity |

## 2. Keep your `embedded-hal` drivers

Your drivers stay as-is. A bus adapter exposes NobroRTOS's `BusSal` as the `embedded-hal`
I2C/SPI traits a driver expects (see `core/adapters/embedded-hal-i2c` and
`core/adapters/embedded-hal-spi`), so the driver crate you already depend on runs unchanged
inside the module. This is the highest-ROI part of the port: no driver rewrite.

## 3. When you genuinely want `async fn`: the bounded executor

If a piece of logic is naturally async (a multi-step handshake, a state machine that
`await`s), you do not have to unroll it by hand. `nobro_kernel::async_exec::BoundedExecutor`
is a fixed-capacity, no-alloc cooperative runtime you host *inside one module*:

```rust
use nobro_kernel::async_exec::BoundedExecutor;
use core::pin::pin;

let mut exec = BoundedExecutor::<4>::new();      // at most 4 concurrent tasks, ever
let fut = pin!(handshake(&mut bus));             // your async fn, stack-pinned
exec.spawn(fut).expect("within capacity");       // returns Err(Full) instead of allocating

// drive it from poll(), bounded so a stuck future can never hang the system:
match exec.run_to_idle(64) {
    Ok(_)  => { /* all handshakes done this tick */ }
    Err(_) => { /* Stalled: budget exceeded, take a recovery action */ }
}
```

The difference from Embassy: capacity is a `const N` you pick up front, `spawn` fails loudly
at the limit, and `run_to_idle` is itself bounded (it returns `Stalled` rather than looping
forever). You get the async ergonomics without importing unbounded behavior. Use it as an
escape hatch, not as the whole system - hard-real-time work still belongs on the deadline
scheduler.

## 4. Message types: generate them from ROS `.msg` (optional)

If your Embassy app talks to a ROS 2 world, don't hand-pack bytes. Generate a bounded,
fixed-size Rust struct (and the bridge contract fragment) from the `.msg`:

```bash
python tools/ros_msg_gen.py sensor_msgs/Imu.msg --type sensor_msgs/Imu --gen-rust imu_msg.rs
```

The emitted module is self-contained `no_std` with little-endian `encode`/`decode` and a
stable type hash - the same hash the device and host agree on. See
[the ROS section of PORTING_FROM.md](PORTING_FROM.md#micro-ros--ros-2).

## 5. Recipe summary

1. List your tasks; each becomes a **module** with budget + capabilities + (if RT) deadline.
2. Keep your **`embedded-hal` drivers**; reach them through a bus adapter.
3. Replace `Timer::after().await` cadence with a **deadline period** or an **alarm**.
4. Replace channels/signals with a **bounded `Mailbox`** or **sample pool**.
5. Only where async truly helps, host a **`BoundedExecutor`** inside one module.
6. Run admission and read the `NOBRO_*` reports to confirm the contract holds.
