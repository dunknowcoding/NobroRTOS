# ESP32 continuous ADC adapter

This adapter mounts a board-core continuous ADC/DMA implementation behind the
allocation-free `nobro-sensor` contract. NobroRTOS owns configuration bounds,
lifecycle, deadline and partial-frame reporting, while the mounted transport
owns ADC conversion, DMA storage, interrupts, and vendor runtime resources.

The in-tree transport split has host tests and Arduino-ESP32 target builds.
State-restoring classic ESP32 and single-core ESP32-C3 campaigns verify
sampling rate, lifecycle recovery/release, and immediate runtime reservation;
the C3 delivered 19,999 conversions/s. Its unreferenced input is not
calibration evidence. Conversion accuracy and complete price dimensions remain
open, so no exact board binding is promoted.
`quiesce` preserves configuration for recovery; `release` stops and
deinitializes the process-wide ADC engine and returns the adapter to `Down`.
