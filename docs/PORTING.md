# Porting

This guide covers two jobs: adapting existing bounded application logic to NobroRTOS
contracts and adding a new MCU/board target. It intentionally avoids framework-specific
translation rules; the public API is the source of truth.

## Reusing application logic

Code is easiest to reuse when it is `no_std`, does not allocate on hot paths, accepts
its dependencies through traits, and has explicit buffer capacities.

| Existing element | NobroRTOS destination |
| --- | --- |
| Pure algorithm, DSP, or control law | Call from a module task or bounded async task |
| Synchronous `embedded-hal` driver | Mount behind an adapter in `core/adapters/<domain>/` |
| Message schema | Fixed-capacity record or generated bounded ROS record |
| Periodic task | Graph task with period, budget, criticality, and dependencies |
| Queue | Bounded mailbox or channel with explicit capacity |
| Shared peripheral | Capability grant plus a HAL lease/provider |
| Board configuration | Data-only `board.json` profile |

Do not carry over a scheduler, allocator, global peripheral singleton, or unbounded
queue. Keep the reusable register/algorithm layer and express ownership, timing, and
capacity through Nobro contracts.

## Add a device adapter

1. Choose the domain under `core/adapters/` (`imu`, `wireless`, `power`, and so on).
2. Implement the domain contract without embedding board policy.
3. Accept a bus/provider through a trait instead of constructing global hardware.
4. Bound every buffer, retry count, and recovery window.
5. Add the adapter path to `core/adapters/catalog.json`.
6. Compose it in an application under `core/apps/<use-case>/`.

Crates hold shared domain contracts. Adapters hold device or library implementations.
Applications select and connect them. Avoid another parallel “ecosystem” hierarchy.

## Add a board profile

Board profiles live under `core/boards/<platform>/<board>/board.json`. A profile is data,
not proof of a working port. Give it a globally unique `board_id` and describe:

- architecture and memory regions;
- boot/application flash origin;
- RAM capacity;
- critical pins and buses;
- supported upload format where applicable.

Run:

```bash
python tools/check_board_profiles.py
python tools/check_core_layout.py
```

## Add an MCU-family port

Ports live in `core/ports/<mcu-family>/` as isolated target workspaces.

1. Add the target and linker memory layout without hardcoding a developer-machine path.
2. Implement `HalClock` first and declare only capabilities that are present.
3. Add deadline, bus, PWM, USB, event, and lease providers independently.
4. Keep board pin selection outside reusable provider types.
5. Return explicit unsupported/capacity errors; do not substitute a software stub for a
   hardware capability.
6. Add a repository-contained implementation directory, a named composition, and only
   implemented claims to `core/boards/platform_tiers.json`.
7. Scope every host/target gate to the exact platform, composition, and capabilities it
   exercises. Bind its runner to a hosted workflow job and receipt driver; the validator
   rejects a required gate that the driver does not invoke.
8. Begin a clean receipt session, execute the matrix argv through
   `tools/check_platform_tiers.py --run-gate`, and assert all runner receipts. Receipts
   bind the current matrix and session to Git HEAD, the tracked diff, and every
   nonignored untracked source path and its content. Ignored `_work` output is excluded.
   This is local freshness bookkeeping, not signed or physical attestation.

The public provider traits are in `nobro_hal::traits`. A provider tier means at least
one real trait implementation exists for the target. A deep tier requires one native
composition to implement every capability in the current provider vocabulary.

## Boot layout rules

- Keep application flash origins in board data and linker scripts synchronized.
- Never erase or rewrite a bootloader as a side effect of a normal application upload.
- Package only application regions in UF2/bin artifacts.
- Treat signing keys and provisioning transports as board integrations, not generic
  kernel behavior.

## Completion checklist

- [ ] Public paths contain no endpoint names, local serial ports, machine paths, or
      private project notes.
- [ ] The adapter/application/board/port lives in the existing categorized tree.
- [ ] Every capacity and unsupported path is explicit.
- [ ] Board and platform matrices describe only implemented public capabilities.
- [ ] `python tools/run_checks.py` passes.
- [ ] `tools/ci_matrix.sh` passes in an MSYS2 or POSIX shell.
