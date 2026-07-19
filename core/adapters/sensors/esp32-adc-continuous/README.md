# ESP32 continuous ADC adapter

This adapter mounts a board-core continuous ADC/DMA implementation behind the
allocation-free `nobro-sensor` contract. NobroRTOS owns configuration bounds,
lifecycle, deadline and partial-frame reporting, while the mounted transport
owns ADC conversion, DMA storage, interrupts, and vendor runtime resources.

The in-tree transport split has host tests and an Arduino-ESP32 target build.
An exact board binding requires measured resource and physical evidence.
