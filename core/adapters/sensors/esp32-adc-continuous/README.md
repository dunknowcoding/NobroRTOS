# ESP32 continuous ADC adapter

This adapter mounts a board-core continuous ADC/DMA implementation behind the
allocation-free `nobro-sensor` contract. NobroRTOS owns configuration bounds,
lifecycle, deadline and partial-frame reporting, while the mounted transport
owns ADC conversion, DMA storage, interrupts, and vendor runtime resources.

The in-tree transport split has host tests and Arduino-ESP32 target builds.
A state-restoring classic ESP32 campaign verifies sampling rate, lifecycle
recovery, and immediate runtime reservation. Conversion accuracy and complete
price dimensions remain open, so no exact board binding is promoted.
