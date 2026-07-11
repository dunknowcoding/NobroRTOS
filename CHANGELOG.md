# Changelog

## Unreleased — flexibility and evidence closure

### Added

- One graph declaration now derives manifest, startup order, task metadata, labels, and
  mailbox grants; typed task builders provide reviewable defaults.
- A no-allocation, wake-driven async reactor with fuel bounds, timers, backpressuring
  channels, signals, cancellation, and composition under normal kernel admission.
- `nobro project`: create/import, explain marginal costs and schedulability, compile the
  workload-derived graph, simulate, decode reports, or delegate state-restoring HIL.
- Measured NobroRTOS/Embassy/bare-metal baselines with regression budgets and an honest
  public limitations matrix.
- RP2350 and ESP32-C3 timebase-provider ports, plus a typed support-tier gate.

### Verification

- Added dedicated bounded-async Miri tests and a persistent async service fuzz target.
- Provider firmware now executes the timebase check and folds it into `all_pass`;
  physical provider smoke was run locally without publishing endpoint evidence, and the
  pre-existing ESP32-C3 application was restored afterward.
- The closure gate includes host tests, format/lint, dependency audit, coverage, Miri,
  fuzz, sanitizer, cross-MCU builds, package validation, privacy checks, and deep-HAL HIL.

### Known boundaries

- nRF52840 remains the only deep-HAL platform. Provider ports do not yet have peripheral
  parity or reusable state-restoring HIL integration.
- Cooperative execution, declared/measured timing budgets, shared privileged address
  space, and board-specific boot/key integration remain explicit limitations; see
  [docs/LIMITATIONS.md](docs/LIMITATIONS.md).

## 0.2.0

NobroRTOS grows from a verified single-board control plane into a multi-board, AI-aware,
networked RTOS surface (40 of 50 roadmap milestones).

### Added
- **AI runtime**: `ModelRegistry` multiplexes models by id; `AiRoutePolicy` hybrid routing
  verified on hardware; on-device NN inference timed at 68 us (DWT), inside its 2 ms deadline.
- **Embedded ML** (`nobro-ml`): streaming anomaly detection, EWMA adaptation, confidence
  reject, complementary fusion, and confidence-weighted ensemble (distributed) inference.
- **Networking** (`nobro-net`): distance-vector routing, NTP-style time sync, bounded
  aggregation, gossip dedup, QoS priority queue, partition/reconnect monitor, and a
  heterogeneous unified node schema.
- **Crypto** (`nobro-crypto`): AES-128 (FIPS-197 verified, 131 MB/s host) + seedable
  `CryptoSal` PRNG.
- **Sensors**: INA3221 power-monitor driver; sensor health/calibration/decimation
  (`nobro-sensor`); on-chip flash data logging via NVMC (verified on hardware).
- **Power** (`nobro-power`): sleep-mode selection + duty-cycle budgeting.
- **Kernel**: cross-module memory-budget enforcement, capability-gated admission,
  quota-under-load, and dynamic reallocation tests (169 host tests).
- **Boards**: data-first board profiles in `core/boards/` + validator.
- **Host tooling**: verification orchestrator, board provisioning automation, mesh health
  dashboard, chaos tester, SDK-manifest validator, and a generated API index.

### Verified
- Board demos self-certify via `NOBRO_*` reports over J-Link; host crates via `cargo test`;
  end-to-end by `tools/run_checks.py` (ALL PASS).

## 0.1.0

- Initial verified single-board control plane: kernel manifest/admission/quota/recovery,
  SAL traits, HAL, edge-AI inference, ROS bridge, and the host ABI contract.
