# ESP32 LEDC adapter

This adapter mounts one board-core LEDC instance behind the allocation-free
`nobro-servo` PWM engine contract. Frequency, resolution, duty bounds,
lifecycle, recovery, and resource pricing stay explicit.

State-restoring classic ESP32 and single-core ESP32-C3 campaigns verify
physical frequency, duty, lifecycle recovery/release, and immediate runtime
reservation; C3 measured 1,002 Hz at 249 permille. ESP32-S3 and ESP32-P4
remain target-build evidence only, and exact board promotion still requires
complete price and coexistence measurements.
`quiesce` preserves configuration for recovery; `release` detaches LEDC,
forgets that configuration, and returns the adapter to `Down`.
