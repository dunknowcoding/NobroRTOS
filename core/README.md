# Core source layout

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
inventory instead of producing duplicate directories. `tools/check_core_layout.py`
enforces the shape.
