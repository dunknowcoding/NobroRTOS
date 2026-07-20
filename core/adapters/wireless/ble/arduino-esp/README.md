# Arduino-ESP32 BLE adapter

This adapter describes the bounded Nobro BLE peripheral facade backed by the
`BLE` library shipped in Arduino-ESP32 3.3.10. The board package selects the
vendor host: classic ESP32 uses Bluedroid, while ESP32-C3 and ESP32-S3 use
NimBLE. That distinction remains visible in each exact board binding.

The facade admits one logical stack instance, one service, one
read/write/notify characteristic, a four-event fixed callback ring, and
20-byte GATT values. The ESP-IDF controller, host tasks, callbacks, and heap
remain vendor-managed. Exact classic ESP32 and ESP32-C3 bindings have physical
GATT/lifecycle/WiFi-coexistence evidence; C3 has an incremental BLE price,
while classic ESP32 has a whole-composition price because its standalone
WiFi baseline is not separately priced. ESP32-S3 remains target-build-only.
