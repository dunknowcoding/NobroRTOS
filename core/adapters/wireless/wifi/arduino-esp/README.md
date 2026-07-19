# Arduino-ESP32 WiFi adapter

This categorized adapter mirrors the stable identity and runtime credential
rules used by `NobroArduinoEspWiFi.h`. The Arduino facade owns the actual
ESP-IDF-backed station lifecycle; the Rust side keeps portable admission and
configuration code independent from Arduino headers.

The implementation is optional. Credentials stay in caller storage and are
never board metadata. Arduino-ESP32/ESP-IDF owns its tasks, event loop, heap,
radio, sockets, and controller resources, so target-build evidence alone does
not price or physically promote the backend.
