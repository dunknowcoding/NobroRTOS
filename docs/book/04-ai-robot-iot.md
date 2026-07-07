# AI, Robot, And IoT Modules

AI, robotics, and networking code should enter NobroRTOS as bounded modules.
That means every model, bridge, and transport declares its memory, buffers,
timeout, and fallback behavior before it participates in runtime work.

Use these rules:

- Local TinyML models use caller-owned input, output, and scratch buffers.
- Remote or sidecar inference stays outside hard-realtime loops.
- ROS-style topics and services map to bounded queues and fixed records.
- Mesh and OTA features use deterministic planners before packets are sent.
- Proximity modules such as RFID readers expose bounded polling through the same IoT transport contract.
- Robot control loops keep deadlines explicit and recover through module state.

This lets one app combine an IMU, actuator, edge model, and network bridge
without letting any single piece own the whole system.
