# Camera support

`nobro-camera` is the system-wide camera domain. It standardizes allocation-free
frame leases, deadline and memory admission, bounded stream windows, backpressure,
recovery, and diagnostics. Sensor register, DMA, PSRAM, storage, and network details
remain in independently selectable device libraries and adapters.

| Member | Clean compile proof | Nobro physical status |
|---|---|---|
| NiusCam 0.2.0, OV2640 / AI-Thinker ESP32-CAM | `esp32:esp32:esp32cam` | Pending restoring HIL |
| NiusCam 0.2.0, OV3660 / XIAO ESP32-S3 Sense | `esp32:esp32:XIAO_ESP32S3:PSRAM=opi` | 5/5 JPEG adapter run; complete pre-test flash restored and verified |
| NiusCam 0.2.0, OV5640 / ESP32-S3-CAM | `esp32:esp32:esp32s3` | Pending restoring HIL |

The pinned upstream library contains its own physical evidence. NobroRTOS does not
silently inherit that evidence. Its OV3660 adapter run captured five valid JPEGs
(14,112 bytes total) in 913,677 microseconds with no drops, then restored the complete
8 MiB pre-test flash image with the flash tool's full-image verification. OV2640 and
OV5640 remain compile-only in NobroRTOS until equivalent restoring runs are available.

The machine-readable pin and support state live in
`core/ecosystem/integration_matrix.json`. CI checks out that exact commit and builds
all three profiles without modifying the external library.
