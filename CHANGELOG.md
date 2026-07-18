# Changelog

## Unreleased

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
