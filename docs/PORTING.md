# Porting

Two directions: bringing your codebase *to* NobroRTOS (from FreeRTOS, Embassy,
Zephyr, Arduino, bare metal) and bringing NobroRTOS to new silicon.

## Choosing your route

**Short answer:** there is no adapter-free, drop-in port - *by design*. NobroRTOS is
a contract + thin-adapter RTOS whose whole point is to bound memory, timing, and
resource ownership. Importing another stack's executor, DDS transport, devicetree
glue, or heap behavior unmodified would import exactly the unbounded behavior
NobroRTOS exists to prevent. What *does* transfer is the part with real value:
driver logic, algorithms, and message schemas. What must be re-expressed is the
*system wiring* (tasks, timing, recovery, transport config), and that re-expression
is usually small and mechanical.

### What transfers vs what you re-express

| From the source app | Transfers as-is? | In NobroRTOS |
| --- | --- | --- |
| `embedded-hal` device drivers (sensors, displays, radios) | **Yes** | run under a `BusSal` adapter unchanged |
| Pure algorithm / DSP / control-law code (`no_std`, no heap) | **Yes** | call it from a module's `poll()` |
| Message/struct schemas (ROS msg, packet layouts) | **Mostly** | map to fixed-capacity records / bounded queues |
| Async tasks / threads | Re-expressed | become **modules** with budgets + (if RT) deadlines |
| Executor / scheduler (Embassy, Zephyr, rclc) | **No** | the NobroRTOS kernel schedules; don't bring one |
| Heap / dynamic allocation on hot paths | **No** | static pools, fixed capacity |
| DDS / micro-ROS agent transport | Re-expressed | a **bounded bridge adapter** over `StreamSal`/`RadioSal` |
| devicetree / Kconfig / board DSL | Re-expressed | the **board package** (layout, pins, capacity as data) |
| Recovery / watchdog policy | Re-expressed | health counters + module-scoped recovery actions |

Every ported unit still declares its memory budget, capabilities, criticality, and
deadline, and still passes admission. The UX makes that *easier to write*, never
optional.

### Embassy / `embedded-hal` (highest ROI)

Both are Rust + `embedded-hal`, so this is the smoothest path. For a step-by-step recipe
with copy-pasteable code, see the Embassy cookbook in this guide.

- **Drivers:** an `embedded-hal` I2C/SPI **bus adapter** exposes NobroRTOS's `BusSal`
  as the `embedded-hal` traits a driver expects, so the large universe of
  `embedded-hal` drivers runs unchanged.
- **Tasks to modules:** each Embassy `#[embassy_executor::task]` becomes a NobroRTOS
  module. An `async fn` that `await`s a timer becomes a `poll()` that checks a
  deadline alarm and returns; cooperative `select!` over events becomes draining a
  bounded mailbox. If a driver is fundamentally async, an Embassy executor may be
  hosted *inside one bounded module* rather than owning the system.

```text
// Embassy                              // NobroRTOS
#[task]                                 ModuleSpec::new(Sensor, Driver)
async fn imu(mut i2c: I2c) {              .requires(Bus0 | SamplePool | Timebase)
  loop {                                  .memory(MemoryBudget::new(..))
    let s = read(&mut i2c).await;       fn poll(&mut self) -> Result<Option<Sample>> {
    TELEM.send(s).await;                  let s = self.read_burst()?;       // embedded-hal driver
  }                                       Ok(Some(pool.publish(s)?))        // bounded pool, no .await
}                                       }
```

### micro-ROS / ROS 2

Bring the **interfaces and algorithms**, not the middleware. A bounded **bridge
adapter** over `StreamSal`/`RadioSal` maps ROS concepts to bounded NobroRTOS
contracts (this is the design's stated direction):

| ROS concept | NobroRTOS bridge |
| --- | --- |
| topic (pub/sub) | bounded queue (fixed depth, fixed payload) |
| service | fixed request/response record |
| action | bounded goal / feedback / result state |
| parameter | fixed-capacity key-value |

An `rclc` node's callback logic ports into a module; the node's transport/agent
config does not (a bridge generated from the NobroRTOS manifest carries it, so the
system truth is not duplicated). Topic payloads and queue depths are declared and
admission-checked before any agent is contacted.

### Zephyr (most divergent)

Zephyr brings devicetree, Kconfig, and its own subsystems, so this is a porting
guide, not a compatibility runtime. For a step-by-step recipe, see the
Zephyr cookbook later in this guide.

| Zephyr | NobroRTOS |
| --- | --- |
| devicetree node | board-package entry (pins, bus, capacity) |
| `K_THREAD` / work queue | module with budget + deadline |
| driver (`device` API) | SAL adapter (`BusSal`/`SensorSal`/`ActuatorSal`) |
| Kconfig feature flags | Cargo features + manifest capabilities |
| `k_timer` / `k_poll` | deadline alarms + bounded mailbox |

### Arduino / C

Arduino `setup()/loop()` maps to a module's `init()` + `poll()`. Today the
C/C++/Arduino surface only **decodes `NOBRO_*` reports** on the host; on-MCU module
authoring in C/C++ is the planned C-ABI work (see
the packaged-library tier on the roadmap). Until then, author
modules in Rust (or generate them from `nobro-contract.json`) and reuse Arduino
*library logic* through an adapter.

### Recommended porting recipe

1. Identify the **driver + algorithm** code (this transfers).
2. Express each task/thread as a **module**: budget, capabilities, criticality,
   deadline.
3. Wrap hardware access behind a **SAL adapter** (reuse `embedded-hal` drivers).
4. Replace executor/DDS/devicetree with the **board package + manifest + bridge**.
5. Run admission; read the `NOBRO_*` reports to confirm the contract.

### Cookbooks (step-by-step)

- **Embassy cookbook below** - tasks to modules, keeping `embedded-hal`
  drivers, and the bounded async executor as an escape hatch.
- **FreeRTOS cookbook below** - `xTaskCreate`/queues/semaphores/timers
  mapped to modules, mailboxes, capabilities, and deadline alarms.
- **Zephyr cookbook below** - devicetree/Kconfig/`k_thread` mapped to
  `board.json`, manifest capabilities, and SAL adapters; DTS import on-ramp.
- **UDI rule in [ARCHITECTURE.md](ARCHITECTURE.md)** - one category, one trait, N mountable backends.

See [ARCHITECTURE.md](ARCHITECTURE.md) for layering and the board-port workflow later in
this guide.

## From FreeRTOS

You think in tasks, priorities, queues, semaphores, and software timers. This cookbook maps
those primitives onto NobroRTOS and shows where the mental model changes. It complements the
high-level migration overview above.

The one idea to internalize: **FreeRTOS schedules priorities you assign; NobroRTOS schedules
a criticality + deadline contract it can check.** Instead of tuning priorities until timing
"looks right," you *declare* each module's period, jitter budget, memory, and capabilities,
and admission rejects a system that cannot meet them.

### 1. `xTaskCreate` -> a module

A FreeRTOS task is a function with its own stack and a fixed priority. A NobroRTOS module is
a struct with `init()` + `poll(now_us)` and a declared contract; the kernel owns the stack
budget and the schedule.

```c
/* FreeRTOS */
void imu_task(void *arg) {
    for (;;) {
        imu_sample_t s = imu_read();
        xQueueSend(telemetry_q, &s, 0);
        vTaskDelay(pdMS_TO_TICKS(20));
    }
}
xTaskCreate(imu_task, "imu", 512, NULL, 3 /*prio*/, NULL);
```

```rust
// NobroRTOS: the delay is a period; the priority is a criticality; the queue is a mailbox.
ModuleSpec::new(ModuleId::Sensor, Criticality::Driver)
    .requires(CapabilitySet::empty().with(Capability::Bus0).with(Capability::SamplePool))
    .memory(MemoryBudget::new(8 * 1024, 1024, 1)) // flash, ram, pool slots - checked
    .deadline(DeadlineContract::new(20_000, 10));  // 20 ms period, 10 us jitter budget
```

### 2. Primitive mapping

| FreeRTOS | NobroRTOS | Note |
| --- | --- | --- |
| `xTaskCreate` | `ModuleSpec` + module `poll()` | stack sizing becomes a declared RAM budget |
| task priority (number) | `Criticality` (BestEffort < User < Driver < System < HardRealtime) | 5 ordered classes, not 32 raw levels |
| `vTaskDelay` / `vTaskDelayUntil` | `DeadlineContract` period, or an `Alarm` you check in `poll` | cadence is declared, not slept |
| `xQueueSend/Receive` | `Mailbox` (bounded) | fixed depth + fixed message size |
| `xSemaphoreTake` (mutex) | a **capability** owned by one module | ownership is exclusive by contract, not by lock discipline |
| counting semaphore | quota / pool slots | admission-checked capacity |
| `xTimerCreate` (software timer) | `Alarm` in the `AlarmQueue` | one-shot / periodic deadline events |
| task watchdog | `Watchdog` + `HealthMonitor` counters | misses drive module-scoped recovery |
| priority inheritance | not needed | shared resources are owned, not locked across tasks |
| `xEventGroupSetBits/WaitBits` | `nobro_classic::EventFlags` | 32 flags; `wait_any`/`wait_all` polls, `clear_on_exit` semantics preserved |
| queue sets / block-on-many | `nobro_classic::select2` | bounded multi-event wait with an idle hook (insert `wfe` there); never unbounded |
| heap (`pvPortMalloc`) | static pools, fixed capacity | no hot-path allocation |

### 3. Priorities become criticality + deadline

FreeRTOS gives you a raw priority number and leaves correctness to you. NobroRTOS asks two
questions instead:

- **How critical is it?** `Criticality::HardRealtime` modules *must* carry a
  `DeadlineContract` (period + jitter budget) or admission rejects them. Lower classes
  (`System`, `Driver`, `User`, `BestEffort`) degrade first under pressure - see
  `DegradePlanner`, which drops best-effort work before critical work.
- **Can the system meet it?** Admission sums every module's memory and checks the deadline
  set is schedulable *before* the system runs. A build that can't meet its contract fails
  loudly, not intermittently in the field.

So the porting move is: for each task, pick a criticality class, and for anything real-time,
write down the period and jitter you were implicitly assuming with `vTaskDelayUntil`.

### 4. Queues and semaphores

- **Queue -> `Mailbox`.** Declare depth and message size; sends past capacity fail (or drop
  by policy) instead of blocking unboundedly.
- **Mutex -> capability ownership.** Rather than a runtime lock every task must remember to
  take, a resource (a bus, the sample pool, a radio) is *owned* by exactly one module in the
  manifest. Two modules that both claim `Capability::Bus0` fail admission - the conflict is
  caught at build time, not as a priority-inversion bug at 3am.
- **Counting semaphore -> quota / pool slots.** Capacity is declared and enforced by the
  `QuotaLedger` / `SamplePool`.

### 5. If you have a large legacy C task loop

`nobro_classic` (`core/crates/nobro_classic`) is the compatibility surface for a
`setup()/loop()`-style body. Port the task *logic* into a module's `poll()`, reach hardware
through a SAL adapter (reusing your existing driver code where possible), and let the kernel
provide the schedule and the watchdog. On-MCU authoring in C/C++ rides the C ABI
(`bindings/c/include/nobro_app.h`): your module implements `nobro_app_init` / `nobro_app_poll`
and calls back into host-provided services.

### 6. Recipe summary

1. For each `xTaskCreate`, write a **`ModuleSpec`**: criticality, memory, capabilities, and
   a **deadline** for anything real-time.
2. Turn `vTaskDelay` cadence into a **declared period** or an **alarm**.
3. Turn queues into **bounded mailboxes**; turn mutexes into **capability ownership**.
4. Replace heap with **static pools**.
5. Run admission - it schedules and budget-checks the whole system up front - then read the
   `NOBRO_*` reports to confirm timing and health.

## From Embassy

You already write `no_std` Rust with `embedded-hal` drivers and `async fn` tasks. This
cookbook is the mechanical recipe for re-expressing an Embassy app as a NobroRTOS system.
It complements the high-level migration overview with copy-pasteable steps.

The one idea to internalize: **Embassy gives you an unbounded async runtime; NobroRTOS gives
you bounded modules that declare their budget and are admission-checked.** Your driver and
algorithm code transfers unchanged - the *system wiring* is what you rewrite, and it stays
small.

### 1. A task becomes a module

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

### 2. Keep your `embedded-hal` drivers

Your drivers stay as-is. A bus adapter exposes NobroRTOS's `BusSal` as the `embedded-hal`
I2C/SPI traits a driver expects (see `core/adapters/embedded-hal-i2c` and
`core/adapters/embedded-hal-spi`), so the driver crate you already depend on runs unchanged
inside the module. This is the highest-ROI part of the port: no driver rewrite.

### 3. When you genuinely want `async fn`: the bounded executor

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

### 4. Message types: generate them from ROS `.msg` (optional)

If your Embassy app talks to a ROS 2 world, don't hand-pack bytes. Generate a bounded,
fixed-size Rust struct (and the bridge contract fragment) from the `.msg`:

```bash
python tools/ros_msg_gen.py sensor_msgs/Imu.msg --type sensor_msgs/Imu --gen-rust imu_msg.rs
```

The emitted module is self-contained `no_std` with little-endian `encode`/`decode` and a
stable type hash - the same hash the device and host agree on. See
the ROS migration section in this guide.

### 5. Recipe summary

1. List your tasks; each becomes a **module** with budget + capabilities + (if RT) deadline.
2. Keep your **`embedded-hal` drivers**; reach them through a bus adapter.
3. Replace `Timer::after().await` cadence with a **deadline period** or an **alarm**.
4. Replace channels/signals with a **bounded `Mailbox`** or **sample pool**.
5. Only where async truly helps, host a **`BoundedExecutor`** inside one module.
6. Run admission and read the `NOBRO_*` reports to confirm the contract holds.

## From Zephyr

You think in devicetree, Kconfig, `k_thread`, work queues, and `k_timer`. This
cookbook maps those primitives onto NobroRTOS and shows where the mental model
changes. It complements the migration overview above.

The one idea to internalize: **Zephyr describes hardware in DTS and configures
features in Kconfig; NobroRTOS describes hardware in `board.json` and configures
capabilities in the manifest.** You keep your driver logic; you re-express the
*system wiring* as data + contracts.

### 1. devicetree → board.json

Zephyr's devicetree is the source of truth for pins, memory, and peripherals.
NobroRTOS uses data-first `core/boards/*/board.json` profiles validated by
`tools/check_board_profiles.py`.

Best-effort import from a `.dts` file:

```bash
python tools/import_dts.py board.dts --out core/boards/mine/board.json
```

The importer maps the clean subset (compatible, model, code/app flash partition,
SRAM region, LED gpio) and lists everything DTS cannot carry under `_review`:

- `feature` (Cargo feature name)
- `capacity.*` (NobroRTOS software budgets)
- `pins.servo_pwm` / `pins.mvk_trigger`

**Always review** the emitted `board.json` before trusting it. This is an on-ramp,
not a full DTS compiler.

| Zephyr | NobroRTOS |
| --- | --- |
| devicetree node | `board.json` entry |
| `chosen` / partitions | `boot.layout`, `app_flash_start`, `ram_start` |
| `gpios = <&gpio0 N>` | `pins.led` (and HAL pin tables) |
| Kconfig `CONFIG_*` | Cargo features + manifest capabilities |

### 2. `k_thread` → module

A Zephyr thread is a function with its own stack and a priority. A NobroRTOS
module is a struct with `init()` + `poll(now_us)` and a declared contract.

```c
/* Zephyr */
K_THREAD_DEFINE(imu_tid, 1024, imu_thread, NULL, NULL, NULL, 5, 0, 0);
void imu_thread(void *a, void *b, void *c) {
    while (1) {
        read_imu(&sample);
        k_msgq_put(&telemetry_q, &sample, K_NO_WAIT);
        k_sleep(K_MSEC(20));
    }
}
```

```rust
// NobroRTOS
ModuleSpec::new(ModuleId::Sensor, Criticality::Driver)
    .requires(CapabilitySet::empty().with(Capability::Bus0).with(Capability::SamplePool))
    .memory(MemoryBudget::new(8 * 1024, 1024, 1))
    .deadline(DeadlineContract::new(20_000, 10));
```

### 3. Primitive mapping

| Zephyr | NobroRTOS | Note |
| --- | --- | --- |
| `k_thread` / `k_work` | module with budget + deadline | stack sizing → RAM budget |
| thread priority | `Criticality` + `DeadlineContract` | 5 ordered classes, not 32 levels |
| `k_msgq` | `Mailbox` (bounded) | fixed depth + message size |
| `k_sem` / `k_mutex` | capability ownership | exclusive by contract, not lock discipline |
| `k_timer` / `k_poll` | `Alarm` + deadline scheduler | cadence is declared |
| Zephyr driver (`device` API) | SAL adapter (`BusSal`/`SensorSal`) | reuse logic behind trait |
| `CONFIG_*` flags | Cargo features + manifest capabilities | admission-checked |
| `LOG_*` | `EventLog` + `NOBRO_*` reports | host-decodable |

### 4. Driver porting

Zephyr drivers often assume the Zephyr `device` API and devicetree macros. The
porting move:

1. Extract the **register logic** (the part that reads/writes hardware).
2. Reach the bus through a **SAL adapter** (`BusSal` or `embedded-hal` bridge).
3. Wrap the logic in a **module** with budget + capabilities.
4. Replace DTS pin macros with `board.json` pin entries + HAL features.

If your driver is already `embedded-hal`, use `backend-eh` behind a UDI category
trait — see [ARCHITECTURE.md](ARCHITECTURE.md).

### 5. Recipe summary

1. Import or hand-write a **`board.json`** (DTS on-ramp or manual).
2. Map each `k_thread` to a **module** with criticality, memory, deadline.
3. Replace `k_msgq` with a **bounded mailbox**; replace mutexes with **capability ownership**.
4. Wrap hardware access in a **SAL adapter**.
5. Run admission and read **`NOBRO_*` reports** to confirm the contract.

See the board-port workflow in this guide for adding a board family and
[import_dts.py](../tools/import_dts.py) for the DTS on-ramp.

## Porting NobroRTOS to a new board or MCU

This guide describes how to add a board family or platform without weakening
the core architecture.

### Porting Checklist

1. Add or extend a HAL platform module.
2. Define a board descriptor.
3. Add memory layout files.
4. Add one Cargo feature per boot layout.
5. Export a board profile report.
6. Add host-side checks for feature selection and report constants.
7. Add stub adapters before real device integration when possible.

### Platform Port

A platform port implements the HAL traits needed by apps and adapters:

- clock and monotonic time
- deadline timer
- event capture
- resource leases
- bus access
- PWM or actuator backend
- board inspection snapshots

Use platform-specific names only inside the platform module. App and adapter
APIs should use portable HAL terms.

Each platform also implements `HalCompatibility` and declares a
`HardwareCapabilitySet`. This metadata lets host-side tests and app assembly
check whether a backend advertises required services such as timebase, resource
leases, deadline timer, event capture, PWM, bus access, and self-test snapshots
before board-specific validation begins.

### Board Descriptor

Board profiles live in `core/boards/*/board.json` and are validated by
`tools/check_board_profiles.py`. For a Zephyr-style on-ramp from DeviceTree:

```bash
python tools/import_dts.py board.dts --out core/boards/mine/board.json
```

Review the `_review` list in the output — DTS carries hardware layout, not
NobroRTOS software budgets or Cargo feature names. See the Zephyr cookbook above.

A board descriptor should include:

- board name and stable hash
- app flash start
- flash and RAM budgets
- sample-pool slots
- max module count
- critical pins
- servo defaults
- bootloader compatibility notes

The HAL also exposes a `BoardPackage` contract. A package combines the board
descriptor with boot layout, app flash range, RAM range, capacity budgets, and
critical pins. New board ports should make this package valid before app
assembly depends on the board.

Example shape:

```rust
pub const BOOT_PROFILE: BootProfile = BootProfile::new(
    BootLayout::NoSoftDevice,
    0x1000,
    1020 * 1024,
    0x2000_0000,
    256 * 1024,
);

pub const ACTIVE_BOARD_PACKAGE: BoardPackage = BoardPackage::new(
    Board::PLATFORM_ID,
    Board::BOARD_ID,
    BOOT_PROFILE,
    Board::CAPACITY,
    BoardPins::new(LED_PIN, SERVO_PWM_PIN, MVK_TRIGGER_PIN),
);
```

### Feature Rules

Each firmware build should select exactly one platform feature and one board
feature. Adapter crates should disable default HAL features and forward the
selected board feature explicitly.

Good:

```toml
nobro-hal = { path = "../../crates/nobro_hal", default-features = false }
board-promicro-nosd = ["nobro-hal/board-promicro-nosd"]
```

Avoid hidden default board selection in adapter crates.

### Board Fixtures

Each supported board feature should be mirrored in `BOARD_PROFILE_FIXTURES` and
`BOARD_PACKAGE_FIXTURES`. Profile fixtures let host review tools inspect board
identity, capacity, critical pins, and servo defaults. Package fixtures add boot
layout, memory ranges, and package validation without rebuilding the HAL for
every board feature.

Apps that enable `nobro-kernel/hal-profile` can derive `SystemProfile` from the
active `BoardPackage`. This is the preferred path for admission checks because
the manifest budget then follows the selected board feature.

### Acceptance Gates

Before a board port becomes a recommended target, it should provide:

- host build coverage for its feature set
- board profile report generation
- valid `BoardPackage` contract
- manifest and admission report compatibility
- linker layout review
- declared `HardwareCapabilitySet` for required HAL services
- resource lease coverage for shared peripherals
- at least one app composition that exercises timers, bus, and reports

### Naming

Public product documentation uses NobroRTOS. Existing Rust crate names retain
the `nobro-*` prefix until a coordinated crate migration is performed.
