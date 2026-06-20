# NobroRTOS Bindings

This folder is reserved for language bindings and compatibility facades.

Planned binding targets:

- `c/` for a stable C ABI over reports, manifests, and selected runtime helpers
- `cpp/` for small C++ convenience wrappers
- `python/` for host tooling, report decoding, simulation, and AI/control
  orchestration outside hard realtime paths

Bindings should wrap stable contracts from `core/` and avoid creating a second
source of truth.
