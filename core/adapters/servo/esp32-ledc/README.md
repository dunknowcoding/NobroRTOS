# ESP32 LEDC adapter

This adapter mounts one board-core LEDC instance behind the allocation-free
`nobro-servo` PWM engine contract. Frequency, resolution, duty bounds,
lifecycle, recovery, and resource pricing stay explicit.
The Rust adapter refuses to mount an incomplete or workload-mismatched price:
default zero-valued storage means unknown, while an evidenced zero must be
declared explicitly. Fixed ownership is separate from CPU/latency evidence for
the exact PWM configuration and admitted duty-update rate. The pinned board
core requires one LEDC channel and disables LEDC interrupts and DMA for this
path; those ownership constraints are checked separately. The scheduler must
enforce the admitted update rate at runtime.

State-restoring classic ESP32, single-core ESP32-C3, and dual-core ESP32-P4
campaigns verify physical frequency, duty, lifecycle recovery/release, and
immediate runtime reservation; C3 and P4 each measured 1,002 Hz at
249 permille. ESP32-S3 remains target-build evidence only, and exact board
promotion still requires complete price and coexistence measurements.
`quiesce` preserves configuration for recovery; `release` detaches LEDC,
forgets that configuration, and returns the adapter to `Down`.
