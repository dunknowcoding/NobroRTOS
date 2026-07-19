# ESP32 RMT adapter

This adapter mounts one board-core RMT transmitter behind the allocation-free
`nobro-servo` pulse-engine contract. It owns symbol-count and deadline bounds,
lifecycle, backpressure, recovery, and admission price. The board core owns
the RMT channel, memory block, interrupt, and DMA implementation.

Target compilation is not physical waveform evidence.
