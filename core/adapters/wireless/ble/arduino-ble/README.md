# ArduinoBLE adapter

This adapter mounts the portable `BleStack` contract over an explicitly
selected ArduinoBLE transport. It admits one service, one characteristic, one
connection, and 20-byte GATT values. The portable Rust layer owns lifecycle,
deadline checks, and diagnostics; ArduinoBLE owns its global HCI/GATT state,
controller transport, and dynamic allocation.

The UNO R4 WiFi binding uses ArduinoBLE's official
`HCIVirtualTransportAT` path over the WiFiS3 modem supplied by the installed
Arduino Renesas board package. Target compilation proves the exact source
composition. It does not prove physical GATT behavior, WiFi/BLE concurrency,
recovery, or a resource price; those require separate physical evidence.
