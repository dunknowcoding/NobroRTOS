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
deadline semantics remain exact across cache-line sizes. It offers a compact
`Esp32ContinuousAdc` convenience transport and an opt-in
`Esp32PersistentContinuousAdc<Pins, Conversions>` transport whose aligned
object storage is read directly by ESP-IDF.

State-restoring classic ESP32, single-core ESP32-C3, and dual-core ESP32-P4
campaigns verify sampling rate, lifecycle recovery/release, and immediate
runtime reservation. C3 delivered 19,999 conversions/s; P4 delivered 19,795
with an exact aligned frame. Three-run C3/P4 comparisons found 80/144 bytes
of ADC-specific transient allocator peak in the convenience path and zero
above the common 36-byte instrumentation floor in the persistent path.
Worst active cycles/read fell from 5,338 to 3,268 on C3 and from 11,220 to
6,756 on P4 with unchanged p99 latency. The persistent path increased observed
task-stack high-water by 40/192 bytes. In an equivalent S3 application build,
it used 20,520 B flash / 456 B static RAM versus 21,108 B / 368 B for the
compact path. Their unreferenced inputs are not calibration evidence.
Conversion accuracy and complete fixed price dimensions remain open, so no
exact board binding is promoted.
`quiesce` preserves configuration for recovery; `release` stops and
deinitializes the process-wide ADC engine and returns the adapter to `Down`.
