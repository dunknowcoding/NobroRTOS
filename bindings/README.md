# NobroRTOS Bindings

This folder contains language bindings and compatibility facades.

Binding targets:

- `c/` for a stable C ABI over report layouts and status helpers
- `cpp/` for small C++ convenience wrappers over the C ABI
- `python/` for host tooling, report decoding, simulation, and AI/control
  orchestration outside hard realtime paths

Bindings should wrap stable contracts from `core/` and avoid creating a second
source of truth.
