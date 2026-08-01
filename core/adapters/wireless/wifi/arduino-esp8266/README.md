# Arduino-ESP8266 Wi-Fi adapter

This adapter binds Nobro's wireless lifecycle to the Wi-Fi and lwIP stack in
Arduino-ESP8266 3.1.2. Association and scanning are exposed as pollable,
nonblocking operations; credentials remain caller-owned and are not persisted.

The vendor stack is process-wide and heap-managed. The adapter therefore does
not claim allocation-free radio operation, native USB, multicore execution, or
memory isolation. Exact promotion is scoped to the WeMos D1 Mini profile.
