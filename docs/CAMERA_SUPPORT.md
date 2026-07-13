# Camera support

`nobro-camera` defines allocation-free frame leases, deadline and memory admission,
bounded stream windows, backpressure, recovery, and diagnostics. Sensor registers,
DMA, PSRAM, storage, and network transport remain in independently selectable
device libraries and adapters.

| Member | Public integration | Build target |
| --- | --- | --- |
| NiusCam 0.2.0, OV2640 | `NobroNiusCam.h` facade | AI-Thinker ESP32-CAM |
| NiusCam 0.2.0, OV3660 | `NobroNiusCam.h` facade | XIAO ESP32-S3 Sense |
| NiusCam 0.2.0, OV5640 | `NobroNiusCam.h` facade | ESP32-S3-CAM |

The facade translates NiusCam frames into Nobro frame leases without claiming
ownership of the underlying camera driver. The machine-readable member tree is in
`core/adapters/catalog.json`.
