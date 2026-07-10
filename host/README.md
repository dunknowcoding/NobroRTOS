# host/ — the JSON ABI mirror

`nobro-host-contract.json` is the machine-readable mirror of every fixed `NOBRO_*`
report layout, boot-diagnostic code, capability bit, AI/ROS contract, and (since
Wave 15) the per-app `build_budgets` ceilings. Host tools in any language read
THIS file instead of hardcoding offsets; the Rust side and this mirror are kept
in lock-step by the `check-host-contract` gate (part of `check-software-surface`).

It sits at the repo root — not inside `bindings/` — because it is the neutral
contract *between* firmware and every binding, and validators pin this path.
Details: [docs/API.md](../docs/API.md), "Host contract" section.
