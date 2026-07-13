# Host contract

`nobro-host-contract.json` is the machine-readable mirror of public fixed-layout
`NOBRO_*` reports, boot diagnostic codes, capability bits, and AI/ROS contracts.
Host tools read this file instead of hardcoding offsets. Rust constants and the JSON
mirror are kept in sync by the software-surface check.

Machine-specific endpoint configuration and application-specific measurements are
not part of this contract.
