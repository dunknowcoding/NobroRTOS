# Changelog

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
