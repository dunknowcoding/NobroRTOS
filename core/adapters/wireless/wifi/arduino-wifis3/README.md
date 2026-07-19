# Arduino WiFiS3 adapter

This adapter keeps the UNO R4 WiFi association lifecycle behind the portable
`nobro-wireless` WiFi contract. The Arduino Renesas core and its ESP32-S3
coprocessor firmware still own the UART protocol, IP stack, sockets, heap, and
controller resources.

The adapter owns no allocator and retains no credentials. A join receives
borrowed runtime credentials, passes a finite timeout to the transport, and
checks the reported elapsed time after the synchronous vendor call returns.
That records a missed deadline; it cannot preempt a WiFiS3 call. The exact
Arduino facade documents this distinction and copies scan results into
caller-owned fixed storage.

Target compilation and a zero-cost disabled include establish only compile
support. Physical association, socket traffic, controller-firmware
compatibility, retained/transient heap, stack high-water, CPU/latency,
coexistence, and an exact board-feature price require separate evidence before
promotion.
