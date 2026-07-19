# ESP32-S3 ES8311 audio adapter

This allocation-free adapter mounts the WeAct ES8311 + NS4150B path behind
`nobro-audio`.

Nobro owns format validation, frame bounds, lifecycle, backpressure, and
admission accounting. The mounted Arduino-ESP32/NiusAudio transport owns codec
register control plus I2S/DMA. Vendor resources remain explicit in
`AudioResourcePrice`; the adapter does not relabel them as portable Rust costs.

The public crate has host conformance tests. Target compilation and physical
promotion are separate evidence gates.
