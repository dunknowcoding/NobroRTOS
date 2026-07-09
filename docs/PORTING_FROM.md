# Porting to NobroRTOS (from Embassy, ROS 2, Zephyr, Arduino)

**Short answer:** there is no adapter-free, drop-in port - *by design*. NobroRTOS is
a contract + thin-adapter RTOS whose whole point is to bound memory, timing, and
resource ownership. Importing another stack's executor, DDS transport, devicetree
glue, or heap behavior unmodified would import exactly the unbounded behavior
NobroRTOS exists to prevent. What *does* transfer is the part with real value:
driver logic, algorithms, and message schemas. What must be re-expressed is the
*system wiring* (tasks, timing, recovery, transport config), and that re-expression
is usually small and mechanical.

## What transfers vs what you re-express

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

## Embassy / `embedded-hal` (highest ROI)

Both are Rust + `embedded-hal`, so this is the smoothest path. For a step-by-step recipe
with copy-pasteable code, see the **[Embassy cookbook](COOKBOOK_EMBASSY.md)**.

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

## micro-ROS / ROS 2

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

## Zephyr (most divergent)

Zephyr brings devicetree, Kconfig, and its own subsystems, so this is a porting
guide, not a compatibility runtime:

| Zephyr | NobroRTOS |
| --- | --- |
| devicetree node | board-package entry (pins, bus, capacity) |
| `K_THREAD` / work queue | module with budget + deadline |
| driver (`device` API) | SAL adapter (`BusSal`/`SensorSal`/`ActuatorSal`) |
| Kconfig feature flags | Cargo features + manifest capabilities |
| `k_timer` / `k_poll` | deadline alarms + bounded mailbox |

## Arduino / C

Arduino `setup()/loop()` maps to a module's `init()` + `poll()`. Today the
C/C++/Arduino surface only **decodes `NOBRO_*` reports** on the host; on-MCU module
authoring in C/C++ is the planned C-ABI work (see
the packaged-library tier on the roadmap). Until then, author
modules in Rust (or generate them from `nobro-contract.json`) and reuse Arduino
*library logic* through an adapter.

## Recommended porting recipe

1. Identify the **driver + algorithm** code (this transfers).
2. Express each task/thread as a **module**: budget, capabilities, criticality,
   deadline.
3. Wrap hardware access behind a **SAL adapter** (reuse `embedded-hal` drivers).
4. Replace executor/DDS/devicetree with the **board package + manifest + bridge**.
5. Run admission; read the `NOBRO_*` reports to confirm the contract.

## Cookbooks (step-by-step)

- **[Embassy cookbook](COOKBOOK_EMBASSY.md)** - tasks to modules, keeping `embedded-hal`
  drivers, and the bounded async executor as an escape hatch.
- **[FreeRTOS cookbook](COOKBOOK_FREERTOS.md)** - `xTaskCreate`/queues/semaphores/timers
  mapped to modules, mailboxes, capabilities, and deadline alarms.

See [system-architecture.md](system-architecture.md) for the layering and
[porting-guide.md](porting-guide.md) for adding a board.
