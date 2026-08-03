# Core source layout

Kernel capacity aliases live in `nobro_kernel::presets` and are also re-exported
at the crate root. Static startup graphs and post-start transactional dependency
graphs are separate modules, as are the admitted reactor and its fixed-capacity
synchronization primitives. These source boundaries do not add allocation,
background executors, or target dependencies.

The directory tree is the ownership model:

- `crates/<nobro_domain>` contains reusable contracts and runtime capabilities.
- `adapters/<domain>/<implementation>` contains device or external-library bridges.
  A large protocol domain may add one stack-family level, for example
  `adapters/wireless/wifi/<implementation>`.
- `apps/<use-case>/<composition>` contains complete firmware compositions.
- `boards/<platform>/<board>` contains data-only board profiles.
- `ports/<mcu-family>` contains target provider implementations.

Cross-domain membership is summarized in `adapters/catalog.json`; it does not create a
second source hierarchy. Only one category level is allowed under adapters, apps, and
boards. A library that supports many modules remains one library member with a concise
inventory instead of producing duplicate directories. `tools/checks/core/check_core_layout.py`
enforces the shape.

Workspace crates declare Rust 1.85 as their minimum supported Rust version and
inherit it from the workspace manifest. Hosted release gates use the exact Rust
1.97 toolchain and separately compile the maintained portable core on 1.85.
Standalone ports state their own floor when their architecture toolchain is
newer (currently the ESP32-P4 port).
