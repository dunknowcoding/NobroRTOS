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
- AI module and capability bits for local or external inference contracts
- bounded `AiInferenceSal` request/result contract using caller-owned buffers
- nRF52840 board profile and boot layout features
- HAL hardware capability metadata through `HalCompatibility`
- HAL-level `BoardPackage` validation for boot layout, flash/RAM regions,
  capacity budgets, and critical pins
- host-readable board package reports in the boot diagnostic path
- board profile fixtures for host review without board feature switching
- board package fixtures for host review without board feature switching
- optional `hal-profile` bridge from `BoardPackage` to admission `SystemProfile`
- no-heap `BootAssembly` facade for manifest, startup graph, admission, runtime
  construction, startup reports, and failure snapshots
- unified `BootAssemblyReports` helper for success and failure startup exports
- `sal_adapter_demo` wired through `BootAssembly` while preserving host reports
- `sensor-stub` fixture modes for nominal, silent, error, and bad-data samples

## Near-Term Architecture Work

- expand `BootAssembly` app wiring beyond `sal_adapter_demo`
- keep board package fixtures aligned with every supported boot layout
- keep board profile fixtures aligned with every supported board feature
- strengthen adapter manifest examples
- keep host boot summaries aligned with `NOBRO_*` report additions
- keep runtime disable, quota release, mailbox purge, alarm purge, and watchdog
  cleanup behavior covered by tests
- connect adapter preflight reports with `BootAssembly` in remaining demo apps
- add AI model descriptor reports for backend kind, model ID, arena bytes,
  input/output bounds, timeout, and stale-result policy
- add ROS/micro-ROS bridge descriptors for bounded topics, services, actions,
  parameters, and custom transports

## Adapter Work

- harden `robo-servo` around actuator timing contracts
- use `sensor-stub` fault modes in recovery and compatibility scenarios
- mature `mpu9250-imu` while keeping bus access behind `BusSal`
- add future radio and stream adapters only when their resource ownership model
  is explicit
- add AI adapters for local TinyML, generated C++ model libraries, sidecar
  inference, and remote API sessions
- add robotics bridge adapters that keep ROS-style communication bounded and
  diagnosable

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
- ROS/micro-ROS bridge compatibility without making DDS or XRCE-DDS kernel
  dependencies

Formal release tags and versioned releases will wait until the complete edition
has a stable public API, documented board support, and a repeatable validation
story.
