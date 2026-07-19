# Arduino-ESP32 WiFi adapter

This categorized adapter mirrors the stable identity and runtime credential
rules used by `NobroArduinoEspWiFi.h`. The Arduino facade owns the actual
ESP-IDF-backed station lifecycle; the Rust side keeps portable admission and
configuration code independent from Arduino headers.

The implementation is optional. Credentials stay in caller storage and are
never board metadata. The exact C3 composition passed repeated association,
DNS, TCP, leave, quiesce, and recovery with byte-exact firmware restoration.
Arduino-ESP32/ESP-IDF owns its tasks, event loop, heap, radio, sockets, and
controller resources. One exact no-debug C3 binding is completely priced for
four HTTP transactions/s; the price does not transfer to other boards, rates,
socket workloads, or WiFi/BLE compositions.
