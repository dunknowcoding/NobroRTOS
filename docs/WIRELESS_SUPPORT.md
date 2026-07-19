# Wireless support

`nobro-wireless` defines allocation-free link contracts, deadline and resource
accounting, diagnostics, mesh records, and RFID health records. Device-specific
implementations stay under `core/adapters/wireless/`; external Arduino libraries
remain independently selectable members exposed through small Nobro facades.

| Member or backend | Public integration | Current boundary |
| --- | --- | --- |
| WiFi stack contract | `WifiStack` / `MountedWifi` | Portable lifecycle, a compile-only UNO R4 bridge, and one configuration-priced Arduino-ESP32 C3 station binding |
| Arduino-ESP32 WiFi 3.3.10 | `wireless/wifi/arduino-esp` / `NobroArduinoEspWiFi.h` | ESP32/C3/S3 target builds; C3 zero-disabled plus priced association/DNS/TCP/lifecycle evidence at four HTTP operations/s |
| Arduino WiFiS3 | `wireless/wifi/arduino-wifis3` / `NobroArduinoWiFiS3.h` | UNO R4 target build and zero-disabled proof; association/socket/resource behavior unpromoted |
| BLE stack contract | `BleStack` / `MountedBle` / `BleEventQueue` | Portable lifecycle and bounded GATT events only; no board backend promoted |
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

The WiFiS3 facade uses the installed Arduino Renesas board driver rather than
reimplementing its coprocessor protocol. It retains no credentials and copies
scan results into caller-owned fixed records. WiFiS3 itself uses dynamic
strings and synchronous modem calls; Nobro can report elapsed deadline misses
after those calls return but cannot preempt them. TCP/UDP clients, endpoints,
vendor heap, controller firmware, and shared-radio resources remain outside
the compile-only claim.

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
socket workloads, and WiFi/BLE coexistence remain separate evidence.

NiusWireless 0.1.0 currently has an ArduinoNRF portability conflict in its RC522
and SX127x `String(uint8_t, HEX)` calls. Nobro does not patch or hide that upstream
boundary. The machine-readable member tree is in `core/adapters/catalog.json`.
