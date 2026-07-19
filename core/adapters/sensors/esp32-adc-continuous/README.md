# ESP32 continuous ADC adapter

This adapter mounts a board-core continuous ADC/DMA implementation behind the
allocation-free `nobro-sensor` contract. NobroRTOS owns configuration bounds,
lifecycle, deadline and partial-frame reporting, while the mounted transport
owns ADC conversion, DMA storage, interrupts, and vendor runtime resources.
The Rust adapter refuses to mount until every fixed and runtime price
dimension is known for the exact ADC configuration and admitted read rate. A
measured or declared zero is distinct from the default unknown state.
ADC-controller/channel ownership is priced separately from DMA, while
transient heap peak, stack high-water, CPU cycles, and latency stay bound to
the workload that produced them. Configuration or declared-rate mismatch
fails closed; the scheduler must enforce the admitted read rate at runtime.

The in-tree transport split has host tests and Arduino-ESP32 target builds.
The Arduino facade exposes the DMA-aligned conversions-per-channel count and
rejects a request the vendor core would silently widen, so averaging and
deadline semantics remain exact across cache-line sizes.
State-restoring classic ESP32, single-core ESP32-C3, and dual-core ESP32-P4
campaigns verify sampling rate, lifecycle recovery/release, and immediate
runtime reservation. C3 delivered 19,999 conversions/s; P4 delivered 19,795
with an exact aligned frame. The pinned board-core path allocates an aligned
DMA-capable buffer on every read; prior immediate heap snapshots do not prove
its transient peak or allocator-tail behavior. Their unreferenced inputs are
not calibration evidence. Conversion accuracy and complete price dimensions remain open, so no
exact board binding is promoted.
`quiesce` preserves configuration for recovery; `release` stops and
deinitializes the process-wide ADC engine and returns the adapter to `Down`.
