# Arduino-ESP32 BLE adapter

This adapter describes the bounded Nobro BLE peripheral facade backed by the
`BLE` library shipped in Arduino-ESP32 3.3.10. The board package selects the
vendor host: classic ESP32 uses Bluedroid, while ESP32-C3 and ESP32-S3 use
NimBLE. That distinction remains visible in each exact board binding.

The facade admits one logical stack instance, one service, one
read/write/notify characteristic, one pending caller-visible event, and
20-byte GATT values. The ESP-IDF controller, host tasks, callbacks, and heap
remain vendor-managed and require exact physical pricing before promotion.
