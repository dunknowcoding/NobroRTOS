# NobroRTOS Milestone Roadmap (M33–M82)

Continuation of the verified milestone series (M1–M32 complete: kernel control plane,
edge AI, ROS bridge, SPI/embedded-hal, nRF RADIO + resource management, autonomous
multi-board/mesh collector, trained int8 MLP on-device). Each milestone is verified on
real hardware (board1 J-Link, COM boards) or via the endorsed simulation path, then
committed. ✅ = done/verified.

## AI / ML on embedded (M33–M42)
- M33 3-class motion NN (idle / walk / shake) on board1
- [x] M34 Anomaly detection (reconstruction error) on board1
- M35 AiRoutePolicy hybrid routing verified on hardware
- M36 Multi-model registry + AiInferenceSal multiplexing
- [x] M37 int8-vs-float quantization accuracy report
- M38 ESP32-S3-CAM vision recognition over COM25
- M39 Multi-board AI fusion (board1 NN + ESP32 vision) in the collector
- [x] M40 On-device online adaptation (threshold/feature update)
- M41 AI inference latency benchmark + deadline compliance
- [x] M42 Confidence calibration + reject option

## Sensors (M43–M50)
- M43 BMP280 pressure/temp adapter (board5 GY-91)
- M44 ICM45686 interop (Nano) / native adapter
- [x] M45 INA3221 native Rust SAL adapter
- [x] M46 Multi-sensor fusion node (IMU + power + pressure)
- [x] M47 Sensor health / fault detection
- [x] M48 Sensor calibration routine
- [x] M49 Sample-rate management + downsampling
- M50 Sensor data logging to flash (NVMC)

## Multi-board / mesh (M51–M60)
- M51 Real over-the-air radio board-to-board
- [x] M52 Mesh routing protocol (multi-hop, real + sim)
- [x] M53 Time synchronization across nodes
- [x] M54 On-device mesh aggregation + rollup
- [x] M55 Collector ingests UNO R4 + ESP32-S3 + Pico2 W
- M56 Heterogeneous board fusion + unified schema
- M57 Network partition / reconnect handling
- [x] M58 Broadcast / gossip protocol
- [x] M59 Mesh QoS / priority queues
- M60 Distributed inference across the mesh

## Resource management (M61–M68)
- [x] M61 CryptoSal adapter (software AES) + Capability::Crypto
- [x] M62 Power / sleep management + low-power modes
- M63 More managed resources (PWM / EGU / PPI leases)
- M64 Resource contention + arbitration test
- [x] M65 Memory budget enforcement across modules
- M66 Deadline / quota enforcement under load
- M67 Capability-gated multi-module admission
- M68 Dynamic resource reallocation

## Robustness / recovery (M69–M76)
- M69 Fault injection across the mesh
- M70 Watchdog + recovery on real hardware
- M71 Graceful degradation under resource pressure
- M72 Brown-out / reset recovery
- M73 Bus-fault recovery (stuck I2C/SPI)
- M74 Self-DFU autonomy across all UF2 boards
- M75 Host health-monitoring dashboard
- M76 Chaos testing (random faults)

## Tooling / SDK / docs (M77–M82)
- M77 Host SDK packaging completion
- M78 Autonomous multi-board test orchestrator
- M79 Board provisioning automation
- M80 Performance benchmarking suite
- M81 Documentation generation
- M82 Release 0.2.0
