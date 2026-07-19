# ESP32-S3 ES8311 audio adapter

This allocation-free adapter mounts the WeAct ES8311 + NS4150B path behind
`nobro-audio`.

Nobro owns format validation, frame bounds, lifecycle, backpressure, and
admission accounting. The mounted Arduino-ESP32/NiusAudio transport owns codec
register control plus I2S/DMA. Vendor resources remain explicit in
`AudioResourcePrice`; the adapter does not relabel them as portable Rust costs.
The embedded provider price distinguishes unknown fields from evidenced zeroes
and includes I2S peripheral channels separately from DMA channels. Fixed
ownership is separate from transient heap, stack high-water, CPU, and latency
evidence for the exact codec/frame/transfer-rate workload. Incomplete,
zero-ownership, or workload-mismatched prices fail configuration instead of
mounting as apparent zero-cost providers. The scheduler must enforce the
admitted transfer rate at runtime.

The exact Arduino composition is priced only for 16 kHz mono signed-16
full-duplex audio, a 192-byte maximum frame, two queue slots, and 100
capture/playback transfers per second. Its same-target build delta, retained
heap, caller-stack high-water, active cycles, latency, full-duplex DMA/IRQ
ownership, physical playback/capture, and repeated recovery are recorded in
the board-feature registry. Other formats, rates, codecs, and queue shapes
remain unpriced rather than inheriting this binding.
