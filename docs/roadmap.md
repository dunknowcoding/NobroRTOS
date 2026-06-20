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
- host boot summary helpers for first-fault decoding
- adapter descriptors and compatibility reports
- nRF52840 board profile and boot layout features
- HAL-level `BoardPackage` validation for boot layout, flash/RAM regions,
  capacity budgets, and critical pins
- host-readable board package reports in the boot diagnostic path
- board profile fixtures for host review without board feature switching
- board package fixtures for host review without board feature switching
- optional `hal-profile` bridge from `BoardPackage` to admission `SystemProfile`
- no-heap `BootAssembly` facade for manifest, startup graph, admission, runtime
  construction, startup reports, and failure snapshots
- `sal_adapter_demo` wired through `BootAssembly` while preserving host reports

## Near-Term Architecture Work

- expand `BootAssembly` app wiring beyond `sal_adapter_demo`
- keep board package fixtures aligned with every supported boot layout
- keep board profile fixtures aligned with every supported board feature
- strengthen adapter manifest examples
- keep host boot summaries aligned with `NOBRO_*` report additions
- keep runtime disable, quota release, mailbox purge, alarm purge, and watchdog
  cleanup behavior covered by tests
- connect adapter preflight reports with `BootAssembly` in remaining demo apps

## Adapter Work

- harden `robo-servo` around actuator timing contracts
- expand `sensor-stub` into a stronger compatibility fixture
- mature `mpu9250-imu` while keeping bus access behind `BusSal`
- add future radio and stream adapters only when their resource ownership model
  is explicit

## Multi-Board Direction

- keep board facts in descriptors and feature gates
- require one selected board feature per firmware build
- derive runtime admission capacity from board packages where HAL is present
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
