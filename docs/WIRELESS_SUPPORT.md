# Wireless support

`nobro-wireless` defines allocation-free link contracts, deadline and resource
accounting, diagnostics, mesh records, and RFID health records. Device-specific
implementations stay under `core/adapters/wireless/`; external Arduino libraries
remain independently selectable members exposed through small Nobro facades.

| Member or backend | Public integration | Current boundary |
| --- | --- | --- |
| WiFi stack contract | `WifiStack` / `MountedWifi` | Portable lifecycle plus one compile-only UNO R4 bridge; no physical backend promoted |
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

NiusWireless 0.1.0 currently has an ArduinoNRF portability conflict in its RC522
and SX127x `String(uint8_t, HEX)` calls. Nobro does not patch or hide that upstream
boundary. The machine-readable member tree is in `core/adapters/catalog.json`.
