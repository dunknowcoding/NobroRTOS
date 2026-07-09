# Cookbook: coming from Zephyr

You think in devicetree, Kconfig, `k_thread`, work queues, and `k_timer`. This
cookbook maps those primitives onto NobroRTOS and shows where the mental model
changes. It complements [PORTING_FROM.md](PORTING_FROM.md).

The one idea to internalize: **Zephyr describes hardware in DTS and configures
features in Kconfig; NobroRTOS describes hardware in `board.json` and configures
capabilities in the manifest.** You keep your driver logic; you re-express the
*system wiring* as data + contracts.

## 1. devicetree → board.json

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

## 2. `k_thread` → module

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

## 3. Primitive mapping

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

## 4. Driver porting

Zephyr drivers often assume the Zephyr `device` API and devicetree macros. The
porting move:

1. Extract the **register logic** (the part that reads/writes hardware).
2. Reach the bus through a **SAL adapter** (`BusSal` or `embedded-hal` bridge).
3. Wrap the logic in a **module** with budget + capabilities.
4. Replace DTS pin macros with `board.json` pin entries + HAL features.

If your driver is already `embedded-hal`, use `backend-eh` behind a UDI category
trait — see [UDI.md](UDI.md).

## 5. Recipe summary

1. Import or hand-write a **`board.json`** (DTS on-ramp or manual).
2. Map each `k_thread` to a **module** with criticality, memory, deadline.
3. Replace `k_msgq` with a **bounded mailbox**; replace mutexes with **capability ownership**.
4. Wrap hardware access in a **SAL adapter**.
5. Run admission and read **`NOBRO_*` reports** to confirm the contract.

See [porting-guide.md](porting-guide.md) for adding a board family and
[import_dts.py](../tools/import_dts.py) for the DTS on-ramp.
