# NobroRTOS Roadmap

NobroRTOS is moving toward a small, approachable, multi-board RTOS for robotics
firmware. The roadmap favors contracts, diagnostics, and adapter portability
over kernel size growth.

## Foundation

Completed or substantially present:

- Rust workspace split into kernel, HAL, SAL, host, adapters, and apps
- static manifests and module criticality
- capability requirements and ownership
- startup planning
- quota ledger and degraded-mode planning
- fixed mailbox IPC
- fixed alarm queue
- fixed key-value store
- health monitor and recovery coordinator
- event log and host-readable reports
- adapter descriptors and compatibility reports
- nRF52840 board profile and boot layout features
- HAL-level `BoardPackage` validation for boot layout, flash/RAM regions,
  capacity budgets, and critical pins
- host-readable board package reports in the boot diagnostic path

## Near-Term Architecture Work

- add generated board profile fixtures for host review
- add generated board package fixtures for host review
- strengthen adapter manifest examples
- expose richer host decoding examples for `NOBRO_*` reports
- keep runtime disable, quota release, mailbox purge, alarm purge, and watchdog
  cleanup behavior covered by tests
- make app assembly easier without hiding manifest and capability contracts

## Adapter Work

- harden `robo-servo` around actuator timing contracts
- expand `sensor-stub` into a stronger compatibility fixture
- mature `mpu9250-imu` while keeping bus access behind `BusSal`
- add future radio and stream adapters only when their resource ownership model
  is explicit

## Multi-Board Direction

- keep board facts in descriptors and feature gates
- require one selected board feature per firmware build
- add capacity checks before runtime admission
- map `HalEventCapture` to each platform's native trigger fabric
- keep HAL snapshots available for register and layout review

## Long-Term Direction

- static async executor integration
- richer host inspection tools
- optional filesystem or flash persistence behind kernel-owned APIs
- multi-node time synchronization experiments
- optional ML or VM workloads outside hard-realtime paths

Formal release tags and versioned releases will wait until the complete edition
has a stable public API, documented board support, and a repeatable validation
story.
