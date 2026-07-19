# ESP32 RMT adapter

This adapter mounts one board-core RMT transmitter behind the allocation-free
`nobro-servo` pulse-engine contract. It owns symbol-count and deadline bounds,
lifecycle, backpressure, recovery, and admission price. The board core owns
the RMT channel, memory block, interrupt, and DMA implementation.

A state-restoring classic ESP32 campaign verifies physical pulse timing,
lifecycle recovery, and immediate runtime reservation. Other ESP32 families
remain target-build evidence only; full channel/interrupt/DMA/coexistence
pricing is required before an exact binding is promoted.
`quiesce` preserves configuration for recovery; `release` deinitializes RMT,
forgets that configuration, and returns the adapter to `Down`.
