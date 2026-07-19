# Changelog

## Unreleased

### Changed

- `nobro-wireless` now separates the bounded byte data plane from
  allocation-free `WifiStack` and `BleStack` lifecycle contracts. Owned
  fallible mounts, runtime-only WiFi credentials, stable instance limits,
  deadline-aware control calls, and caller-sized BLE callback queues are
  available without selecting or claiming a board backend.
- UNO R4 WiFi now has an opt-in, compile-only Arduino WiFiS3 association
  facade and categorized Rust bridge. Its zero-disabled target build is
  gated separately from physical behavior and resource-price promotion.
- Board-feature metadata can represent an exact compile-only binding with an
  explicitly unmeasured price instead of encoding unknown resources as zero.
- ESP32 Arduino users can select either the compact continuous-ADC transport
  or a fixed-capacity persistent ESP-IDF transport with no per-frame heap
  allocation; unused transports remain link-dead.
- Physical C3/P4 campaigns now price transient heap, stack high-water, active
  cycles, p99/max latency, and ADC/LEDC/RMT coexistence for both ADC paths.
- Board-feature pricing now separates fixed mount ownership from transient
  heap, stack high-water, CPU, and latency evidence bound to an exact provider
  configuration and admitted operation rate.
- ESP32 continuous-ADC, LEDC, RMT, and ES8311 adapters reject incomplete,
  workload-mismatched, or impossible zero-ownership prices.
- Board-feature registry schema v2 requires independent fixed/runtime values
  and provenance. Workload fingerprints are recomputed from explicit
  configuration words, and source-derived ownership is distinct from measured
  values and declared zeroes.
- Configuration-specific persistent ADC-DMA bindings for ESP32-C3, ESP32-P4,
  and ESP32-S3
  carry complete fixed/runtime prices, upstream source pins, coexistence
  ownership, and exact target-build gates.
- The S3 ADC binding covers one 20 ksample/s factory-calibrated workload,
  repeated recovery, zero per-frame heap, and physical coexistence with the
  exact ES8311 audio binding. Absolute voltage accuracy remains unclaimed
  without a known electrical reference.
- The ESP32-S3 Arduino ES8311 composition now has one exact 16 kHz mono
  signed-16 full-duplex binding with zero-disabled proof, isolated
  flash/static cost, retained heap, caller-stack/CPU/latency measurements,
  explicit TX/RX DMA and interrupt ownership, repeated recovery, and physical
  playback/capture evidence.

## 0.3.2 - 2026-07-18

### Added

- Adapter catalog v2 separates domain relationships, component deployment,
  maturity, evidence, target scope, limitations, and immutable provenance.
- `nobro adapter new` creates a bounded multi-backend adapter, registers its
  stable component ID, and adds it to the core workspace.
- Allocation-free `nobro-audio` and `nobro-servo` contracts establish the
  portable domain boundary before hardware-specific backends are promoted.
- Expandable protocol domains may use one family level, such as
  `wireless/wifi/<implementation>`, without adding a duplicate ecosystem tree.

### Changed

- Canonical adapter categories are now `sensors` and `servo`; `environment`
  and `actuator` remain metadata-only migration aliases.
- PlatformIO release archives are deterministic and verified byte-for-byte
  against the tracked package source before publication.

## 0.3.1 - 2026-07-18

### Changed

- The Python distribution is now named `nobro-rtos`, so the normalized
  `pip install nobro_rtos` command installs the `nobro_rtos` package directly.
- The default Python install remains dependency-free; live serial monitoring is
  in the `serial` extra and the large TensorFlow importer is in `tflite`.
- Arduino and PlatformIO package documentation now explains installation,
  configuration, source ownership, and the boundary to native NobroRTOS
  firmware. PlatformIO ships the same checked C++ facade as Arduino.

## 0.3.0 - 2026-07-18

### Added

- One graph declaration now derives the manifest, startup order, task metadata,
  labels, mailbox grants, and schedulability explanation.
- A no-allocation, wake-driven async reactor with fuel bounds, timers,
  backpressuring channels, signals, cancellation, and task groups.
- `nobro project` can create, explain, build, simulate, and decode a graph-declared
  application.
- Profile-aware resource accounting and an explicit public limitations matrix.
- RP2350, ESP32-C3, and ESP32-S3 provider ports with typed support tiers.
- Categorized adapter and application trees plus a concise adapter catalog.
- One versioned task/wire authoring contract now aligns Rust, C, C++11,
  Arduino, Python, JSON, and the block editor; older `sensor`, `channel`, and
  `connect` names remain compatibility aliases.
- One versioned error registry now gives Rust, C/C++11, Arduino, Python, and
  CLI/JSON failures stable `NOBRO-E0xx` identities, generated recovery docs,
  and checked compile-fail diagnostics.
- Arduino, PlatformIO, and Python package surfaces now carry checked license
  copies; PlatformIO archives vendor the canonical C headers instead of
  escaping into the source repository, and clean-artifact smoke tests reject
  private, cache, and compiler-output leakage.

### Known boundaries

- nRF52840 remains the only deep-HAL platform. Other ports implement selected
  providers or the portable core and do not yet have peripheral parity.
- Cooperative execution, declared timing budgets, a shared privileged address
  space, and board-specific boot/key integration remain explicit limitations.

## 0.2.0

NobroRTOS expands from a single-board control plane into a multi-board,
AI-aware, networked RTOS surface.

### Added

- **AI runtime**: bounded local, remote, edge-sidecar, and hybrid routing.
- **Embedded ML**: streaming anomaly detection, adaptation, confidence rejection,
  sensor fusion, and fixed-capacity ensemble inference.
- **Networking**: bounded routing, time synchronization, aggregation, deduplication,
  QoS queues, reconnect monitoring, and a unified node schema.
- **Crypto**: AES-128 with known-answer vectors and a seedable PRNG.
- **Sensors and storage**: INA3221 support, health/calibration/decimation utilities,
  and on-chip flash logging.
- **Power**: sleep-mode selection and duty-cycle budgeting.
- **Kernel**: memory-budget enforcement, capability-gated admission, quotas, and
  bounded resource reallocation.
- **Boards**: data-first board profiles in `core/boards/`.
- **Host utilities**: project generation, contract decoding, SDK validation, and a
  generated API index.

## 0.1.0

- Initial single-board control plane: manifest, admission, quota, recovery, SAL
  traits, HAL, bounded inference, ROS bridge, and the host ABI contract.
