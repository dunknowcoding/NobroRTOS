# Wireless support

`nobro-wireless` defines allocation-free-by-default link contracts, deadline and
resource accounting, diagnostics, mesh records, and RFID health records. Device-specific
implementations stay under `core/adapters/wireless/`; external Arduino libraries
remain independently selectable members exposed through small Nobro facades.

| Member or backend | Public integration | Current boundary |
| --- | --- | --- |
| WiFi stack contract | `WifiStack` / `MountedWifi` | Portable lifecycle plus configuration-priced UNO R4 WiFiS3 and Arduino-ESP32 C3 station bindings |
| Arduino-ESP32 WiFi 3.3.10 | `wireless/wifi/arduino-esp` / `NobroArduinoEspWiFi.h` | ESP32/C3/S3 target builds; C3 zero-disabled plus priced association/DNS/TCP/lifecycle evidence at four HTTP operations/s |
| Arduino WiFiS3 | `wireless/wifi/arduino-wifis3` / `NobroArduinoWiFiS3.h` | Exact UNO R4/WiFiS3 0.6.0 zero-disabled, association, DNS, TCP, lifecycle, and RA-side/controller-image price |
| BLE stack contract | `BleStack` / `MountedBle` / `BleEventQueue` | Portable lifecycle plus physically verified UNO R4 ArduinoBLE, ESP32-C3 NimBLE, and classic ESP32 Bluedroid bindings |
| Adaptive traffic policy | `FixedAdaptiveQueue` / `BorrowedAdaptiveQueue` / optional `HeapAdaptiveQueue` | Portable policy and host behavior; no board is promoted by the policy alone |
| ArduinoBLE 2.1.0 | `wireless/ble/arduino-ble` / `NobroArduinoBLE.h` | UNO R4 zero-disabled; physical GATT/disconnect/remount/recovery; subscribed link across WiFiS3 traffic; complete controller price open |
| Arduino-ESP32 BLE 3.3.10 | `wireless/ble/arduino-esp` / `NobroArduinoEspBLE.h` | ESP32/C3/S3 zero-disabled target builds; exact C3 NimBLE and classic ESP32 Bluedroid GATT/lifecycle/WiFi coexistence; C3 incremental price, classic whole-composition price, S3 physical price open |
| nRF proprietary radio | `core/adapters/wireless/radio-comms` | nRF HAL only |
| NiusWireless 0.1.0 RC522 | Arduino facade and UNO R4 build | Other targets depend on the upstream library |
| NiusWireless 0.1.0 LoRa | Bounded send/receive facade and ESP32-S3 build | Radio-pair behavior is application-specific |
| NiusWireless HC06, HC12, NRF24L01, PN532 | Upstream inventory only | Upstream modules are currently stubs |
| NiusZigbee 1.0.0 / CC2530 | ArduinoNRF library integration | Friendly Nobro facade is not yet complete |

WiFi and BLE control are separate traits beneath the common data plane.
Mounting is fallible and returns ownership on failure. WiFi credentials borrow
runtime caller storage and never enter board metadata. BLE callbacks move
through a caller-sized fixed queue. Backend id, MTU, queue capacity, and GATT
limits are stable per logical instance; a board or vendor stack is supported
only after its separate adapter and evidence gates pass.

## Adaptive traffic without a heavier core

Strict periodic work still uses the deterministic scheduler and immediate
`ManagedLink::send_at`. Variable networks can add `AdaptiveQueue` above the same
link. Each message carries an offered time, desired deadline, hard expiry,
priority, and batching choice. Retry/backoff, cancellation, queue saturation,
offered load, useful delivered throughput, deadline misses, wakeups, and latency
remain explicit. Link-down and local-window deferrals do not consume the backend
retry budget.

The default owns a fixed array; a caller may lend a pool instead. Heap storage is
available only with the Rust `alloc` feature or an explicit C/C++ storage choice,
reserves a bounded slot count once, and must be included in the provider price.
Enqueue and service never allocate in any mode. Completion callbacks only wake the
owning task; protocol work stays outside interrupt/vendor callback context.
The transport completion boundary must itself be bounded: return success only
when the operation counted by the workload is complete. A vendor API that merely
queues work must wake its owner from the ISR/DMA/vendor callback and finish in
task context. Hiding an unbounded DNS, connect, or request wait inside `service`
defeats expiry and is not a conforming adaptive backend.

```rust
let policy = nobro_wireless::AdaptivePolicy::low_energy(50_000);
let mut tx = nobro_wireless::FixedAdaptiveQueue::<8, 64>::fixed(policy)?;
tx.enqueue_best_effort_for(b"telemetry", now, 2_000_000)?;
```

```cpp
auto policy = nobro::WirelessPolicy::lowEnergy(8, 64, 50000);
nobro::AdaptiveWirelessQueue<8, 64> tx(policy);
auto ticket = tx.enqueueBestEffortFor(payload, payloadLength, nowUs, 2000000);
auto outcome = tx.service(nowUs, sendThroughSelectedAdapter, adapterContext);
```

The Arduino queue above is a header-only fixed-capacity implementation, not
just configuration metadata. `service` is the only place it invokes the chosen
transport callback. Rust additionally offers borrowed caller storage and the
explicit `alloc` feature; choosing either never changes the default build.
`reserved_storage_bytes` / `reservedBytes` expose the selected queue's concrete
storage price instead of treating capacity as free.

`RadioPowerHint::IdleUntil` allows normal idle/wake scheduling; it is not
permission to enter a deep mode that would break USB, radio, or another mounted
provider. Adaptive provider workloads retain the exact observation interval and
offered/observed counts. This represents sub-Hz useful work without rounding and
never relabels best-effort delivery as fixed-rate success.

The WiFiS3 facade uses the installed Arduino Renesas board driver rather than
reimplementing its coprocessor protocol. It retains no credentials and copies
scan results into caller-owned fixed records. WiFiS3 itself uses dynamic
strings and synchronous modem calls; Nobro can report elapsed deadline misses
after those calls return but cannot preempt them. TCP/UDP clients, endpoints,
and response buffers remain caller-owned. Three state-restoring cycles passed
75/75 HTTP transactions at one operation/s with zero deadline misses, zero
retained heap, a 1,068 B transient heap peak, a 1,024 B RA stack reservation
and observed high-water, 42,771,027 call-active cycles/s, and a conservative
350,477,834-cycle p99/maximum transaction latency at 48 MHz. The complete
measured RA workload image is 67,420 B flash / 7,824 B static RAM. The board
core owns SCI1, four UART interrupt slots, no DMA channel, and the ESP32-S3
controller; the exact official 0.6.0 controller application artifact is
1,180,064 B. Its exact ELF/map also establishes 64,628 B static RAM; pinned
source establishes at least 22,288 B across three persistent application/USB
task stacks. Retained/transient heap, complete task/stack reservations, CPU,
BLE coexistence, other firmware versions, and other workloads remain separate
evidence.

The ArduinoBLE facade follows the same board-driver-first rule. On the exact
UNO R4 WiFi profile it uses ArduinoBLE 2.1.0's official
`HCIVirtualTransportAT`, which in turn uses the WiFiS3 modem and HCI commands
from the installed Arduino Renesas board package. Nobro admits one mounted
global stack, one service, one characteristic, one logical connection, and
20-byte values. It adds the `HCIEND` teardown omitted by ArduinoBLE 2.1.0,
repairs only the observed cleared-service retain, and exposes provider
disconnect. The disabled composition is byte-identical to baseline, and both
BLE-only and WiFi-plus-BLE images target-build.

Three state-restoring physical cycles passed 15 writes, 21 reads, 18 required
notifications, provider disconnect, quiesce/remount, owned recovery, stable
RA-side heap, and 15 WiFiS3 DNS/TCP transactions while the link stayed
connected and subscribed. Each cycle then required a post-WiFi marker
notification and readback. WiFiS3 calls remain synchronous and non-preemptible,
and controller retained/transient heap, complete task/stack reservations, CPU,
plus the complete shared-controller price remain unmeasured; the binding is
implemented but deliberately unpriced.

The Arduino-ESP32 facade follows the same board-driver-first rule: it includes
the pinned core's official `WiFi.h` and uses the ESP-IDF driver bundled with
that package instead of maintaining a parallel driver. Blocking scan avoids
the core's asynchronous completion race and consumes one fixed record
workspace. `persistent(false)` plus bounded failed-association cleanup keeps
credentials out of persistent storage and clears the vendor RAM copy.
Repeated C3 association, DNS, TCP, leave, quiesce, and recovery passed in a
state-restoring isolated test. The exact no-debug C3 workload is priced for
four HTTP transactions/s: 650,013 B flash, 21,788 B static RAM, 60,348 B
active retained heap, 14,028 B transient heap, four worker tasks, and
6,756 B aggregate caller/worker stack high-water. The conservative measured
runtime price is 6,431,243 cycles/s with 11,243,200-cycle p99 and
16,704,480-cycle maximum transaction latency at 160 MHz. ESP-IDF still owns
the radio, event loop, TCP/IP objects, heap, and tasks. Other rates, boards,
socket workloads, and coexistence beyond the exact BLE composition below
remain separate evidence.

The Arduino-ESP32 BLE facade uses the `BLE` library already bundled with that
same pinned board package. It does not add or prefer an external NimBLE
library: the package selects Bluedroid for classic ESP32 and NimBLE for
ESP32-C3/S3. One facade bounds the consumer surface to one service, one
read/write/notify characteristic, a four-event fixed ring, and 20-byte values while
retaining the selected vendor host in each exact binding. ESP32, C3, and S3
baseline/disabled images are byte-identical, enabled target builds retain only
their expected host, and queue overflow is explicit.

The exact ESP32-C3 composition passed two independent eight-cycle preflights:
160 HTTP operations, 80 GATT writes, 16 reads, 88 notifications, bounded
disconnect/quiesce/recovery, a five-cycle post-warmup heap plateau, and exact
restoration of both participating flash images. Against the separately priced
four-HTTP-operations/s WiFi workload, the conservative BLE increment is
324,703 B flash, 21,276 B static RAM, 77,448 B retained heap, two workers,
3,716 B stack high-water, and 4,156,381 cycles/s. No additional transient-heap
peak was observed. Windows-central GATT write-to-notification latency is priced
at 26,824,448 cycles p99 and 35,823,264 cycles maximum. These values do not
transfer to classic ESP32, ESP32-S3, other rates, or other compositions.

The classic ESP32 Bluedroid composition also passed two independent
eight-cycle preflights with the same 160 HTTP operations, 80 GATT writes,
16 reads, 88 notifications, bounded five-cycle post-warmup heap plateau, and
exact restoration. Bluedroid advertising start/stop is asynchronous, so the
facade now waits for the installed package's GAP completion events and for
vendor connection bookkeeping to drain before exposing quiescence. Conservative
whole-composition maxima are 1,663,227 B flash, 79,072 B static RAM,
153,604 B retained heap, 43,124 B reserved worker stacks, eight workers,
18,656 B transient heap, 17,528 B stack high-water, and 34,200,363 cycles/s
at 240 MHz. GATT write-to-notification latency is 47,247,480 cycles p99 and
68,852,952 cycles maximum. This is not a BLE-only increment: a matching
separately priced classic-ESP32 WiFi baseline remains open.

The exact ESP32-S3 NimBLE composition passed two independent 24-cycle campaigns
with concurrent WiFi association, DNS, bounded nonblocking HTTP, GATT
read/write/notify, disconnect/reconnect, and byte-exact restoration of both
participating flash images. The slower campaign offered 480 messages and
delivered 430 during a 201,714,300 us observation; expiry, retry, deadline miss,
and backpressure are explicit outcomes rather than invented fixed throughput.
Conservative whole-composition maxima are 1,108,868 B flash, 64,612 B static
RAM, 135,944 B retained heap, 29,184 B reserved worker stacks, six workers,
11,376 B transient heap, 15,288 B stack high-water, and 186,157,652 cycles/s at
240 MHz. The larger HTTP/GATT latency is 881,192,160 cycles p99 and maximum.
This is not a BLE-only increment: a matching ESP32-S3 wifi0 baseline, audio and
camera coexistence, and other policies remain separately unpriced.

NiusWireless 0.1.0 currently has an ArduinoNRF portability conflict in its RC522
and SX127x `String(uint8_t, HEX)` calls. Nobro does not patch or hide that upstream
boundary. The machine-readable member tree is in `core/adapters/catalog.json`.
