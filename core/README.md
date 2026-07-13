# Core source layout

The directory tree expresses ownership; the ecosystem matrix expresses relationships.
They are deliberately different:

- `crates/<nobro_domain>` contains reusable contracts and runtime capabilities.
- `adapters/<domain>/<implementation>` contains device or external-library bridges.
- `apps/<use-case>/<composition>` contains complete firmware compositions.
- `boards/<platform>/<board>` contains data-only real-board profiles.
- `ports/<mcu-family>` contains portable provider implementations shared by boards.
- `ecosystem/integration_matrix.json` links domains, adapters, libraries, boards, and
  evidence without duplicating their source trees.

Only one category level is allowed. Package names remain stable if a source directory
moves. A library that supports many modules, such as NiusIMU, is one library member
with an upstream inventory; its module aliases do not become dozens of duplicate
NobroRTOS directories. `tools/check_core_layout.py` enforces these rules.
