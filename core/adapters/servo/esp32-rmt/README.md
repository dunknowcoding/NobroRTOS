# ESP32 RMT adapter

This adapter mounts one board-core RMT transmitter behind the allocation-free
`nobro-servo` pulse-engine contract. It owns symbol-count and deadline bounds,
lifecycle, backpressure, recovery, and admission price. The board core owns
the RMT channel, memory block, interrupt, and DMA implementation.
The Rust adapter refuses to mount until all shared price dimensions are known.
A zero must be measured or explicitly declared, and RMT peripheral-channel
ownership is priced separately from DMA-channel ownership.

State-restoring classic ESP32, single-core ESP32-C3, and dual-core ESP32-P4
campaigns verify physical pulse timing, lifecycle recovery/release, and
immediate runtime reservation; C3 and P4 each measured 499-500 us levels.
ESP32-S3 remains target-build evidence only; full channel/interrupt/DMA/
coexistence pricing is required before an exact binding is promoted.
`quiesce` preserves configuration for recovery; `release` deinitializes RMT,
forgets that configuration, and returns the adapter to `Down`.
