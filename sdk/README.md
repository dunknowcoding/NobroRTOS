# NobroRTOS Standalone SDK

This folder is reserved for the standalone SDK distribution surface.

Planned contents:

- exported headers or generated bindings
- prebuilt metadata for supported board packages
- host-side report decoding helpers
- examples that do not depend on Arduino IDE or PlatformIO
- package build scripts that keep generated artifacts outside the repository

The core implementation remains in `core/`; this folder should contain only the
stable SDK packaging surface.
