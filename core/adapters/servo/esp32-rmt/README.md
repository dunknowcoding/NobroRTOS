# ESP32 RMT adapter

This adapter mounts one board-core RMT transmitter behind the allocation-free
`nobro-servo` pulse-engine contract. It owns symbol-count and deadline bounds,
lifecycle, backpressure, recovery, and admission price. The board core owns
the RMT channel, memory block, interrupt, and DMA implementation.

State-restoring classic ESP32 and single-core ESP32-C3 campaigns verify
physical pulse timing, lifecycle recovery/release, and immediate runtime
reservation; C3 measured 499-500 us levels. ESP32-S3 and ESP32-P4 remain
target-build evidence only; full channel/interrupt/DMA/coexistence pricing is
required before an exact binding is promoted.
`quiesce` preserves configuration for recovery; `release` deinitializes RMT,
forgets that configuration, and returns the adapter to `Down`.
