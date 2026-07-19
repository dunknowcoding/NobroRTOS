# ESP32 LEDC adapter

This adapter mounts one board-core LEDC instance behind the allocation-free
`nobro-servo` PWM engine contract. Frequency, resolution, duty bounds,
lifecycle, recovery, and resource pricing stay explicit.

A state-restoring classic ESP32 campaign verifies physical frequency, duty,
lifecycle recovery, and immediate runtime reservation. Other ESP32 families
remain target-build evidence only, and exact board promotion still requires
complete price and coexistence measurements.
`quiesce` preserves configuration for recovery; `release` detaches LEDC,
forgets that configuration, and returns the adapter to `Down`.
