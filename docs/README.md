# NobroRTOS Documentation

Seven documents, one purpose each. Start at the top; go deeper as needed.

| Document | Read it when you want to… |
| --- | --- |
| [GETTING_STARTED.md](GETTING_STARTED.md) | go from zero to a verified PASS — hardware, zero-code, or laptop-only |
| [USER_GUIDE.md](USER_GUIDE.md) | work with the SDK day-to-day: app generator, prebuilt firmware loop, C modules, repo hygiene |
| [API.md](API.md) | look up the public surface: Rust crates, C ABI, Python package, host contract |
| [ARCHITECTURE.md](ARCHITECTURE.md) | understand the layers, design rules, mountable backends, and the Universal Driver Interface |
| [PORTING.md](PORTING.md) | migrate from FreeRTOS / Embassy / Zephyr, or port NobroRTOS to new silicon |
| [ENGINEERING.md](ENGINEERING.md) | audit the internals: security, interrupts, measured latencies, unsafe inventory, Wasm slot |
| [api-index.md](api-index.md) | scan the generated per-crate symbol index (appendix to API.md) |

Learning by doing instead? The step-by-step ladder lives in
[`tutorials/`](../tutorials/README.md) — from a zero-code first light to Rust
deep dives.

Maintainer-only planning and lab notes are intentionally outside this public
documentation set.
