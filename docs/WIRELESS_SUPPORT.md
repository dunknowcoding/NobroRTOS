# Wireless support

`nobro-wireless` is the system-wide wireless domain. It provides allocation-free
link contracts, deadlines, resource budgets, diagnostics, and mesh re-exports. It
does not replace device libraries: hardware implementations remain categorized
under `core/adapters/wireless/`, and Arduino libraries are ecosystem members.

| Member or backend | Verified scope | Current boundary |
|---|---|---|
| nRF proprietary radio adapter | Nobro HAL lease, 32-byte frames, deadline and window-budget enforcement | nRF HAL only |
| NiusWireless 0.1.0 RC522 | UNO R4 compile with the Nobro health adapter | Physical restoring HIL remains pending |
| NiusWireless 0.1.0 LoRa | ESP32-S3 compile with bounded send/receive adapter | Physical radio-pair HIL remains pending |
| NiusWireless HC06, HC12, NRF24L01, PN532 | Inventory audited | These modules are explicit upstream stubs; Nobro does not claim support |
| NiusZigbee 1.0.0 / CC2530 | Pinned API inventory and ArduinoNRF compile | Friendly Nobro Arduino facade and restoring HIL remain pending |

NiusWireless 0.1.0 currently fails to compile with ArduinoNRF because its RC522
and SX127x sources call an ambiguous `String(uint8_t, HEX)` constructor. Nobro's
adapter does not hide or patch that upstream portability limitation.

The authoritative machine-readable inventory and exact pins are in
`core/ecosystem/integration_matrix.json`; CI recompiles the representative UNO R4
and ESP32-S3 cases from clean pinned checkouts.
