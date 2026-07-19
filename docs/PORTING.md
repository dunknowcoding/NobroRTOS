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
| Synchronous `embedded-hal` driver | Mount behind an adapter in `core/adapters/<domain>/`; use one stack-family level only when a protocol domain needs it |
| Message schema | Fixed-capacity record or generated bounded ROS record |
| Periodic task | Graph task with period, budget, criticality, and dependencies |
| Queue | Bounded mailbox or channel with explicit capacity |
| Shared peripheral | Capability grant plus a HAL lease/provider |
| Board configuration | Data-only `board.json` profile |

Do not carry over a scheduler, allocator, global peripheral singleton, or unbounded
queue. Keep the reusable register/algorithm layer and express ownership, timing, and
capacity through Nobro contracts.

## Add a device adapter

Start with one command:

```bash
python sdk/cli/nobro.py adapter new sensors my-part
```

The command refuses unknown domains and duplicate names. It creates the crate,
registers its stable component ID in catalog v2, adds the workspace member, and
generates mutually exclusive `native`, `embedded-hal`, `c-module`, and
`arduino-shim` backend slots. Use repeated `--backend` options when only a subset
applies.

Then:

1. Implement the domain contract without embedding board policy.
2. Accept a bus/provider through a trait instead of constructing global hardware.
3. Bound every buffer, retry count, and recovery window.
4. Advance `maturity`, `evidence`, and `supported_targets` independently in
   `core/adapters/catalog.json`; a compile is not physical evidence.
5. Pin external source, revision, version, and license once in `provenance`, then
   reference that ID from the member component.
6. Compose the adapter in an application under `core/apps/<use-case>/`.

Crates hold shared domain contracts. Adapters hold device or library implementations.
Applications select and connect them. Ecosystem names are catalog relationships,
never another source hierarchy. `environment` and `actuator` remain migration aliases
for the canonical `sensors` and `servo` domains; aliases never duplicate source.

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

## Add a board-varying feature stack

Use `core/boards/feature_providers.json` for facilities whose implementation
changes with the board or vendor stack, such as I2S, WiFi, BLE, ADC-DMA, CAN,
or Ethernet. Do not put credentials, endpoints, or local tool paths in it.

1. Reuse or add one portable domain contract.
2. Add the capability kind and stack family as data.
3. Scaffold the backend without editing a validator:

   ```bash
   python sdk/cli/nobro.py adapter new audio my-codec \
     --capability-kind audio_i2s
   ```

4. Add a board binding only after its backend exists. Record every price
   dimension, coexistence/lease ownership, limitations, report wiring, and
   scoped evidence gate. Store the workload namespace and configuration words;
   the validator recomputes their fingerprint instead of trusting an opaque
   digest. Classify prices as measured, source-derived, or declared zero.
   Fixed stack means provider-created worker stacks, while caller-task
   high-water is runtime evidence. Ordinary heap-backed DMA pools belong to
   retained heap rather than vendor-reserved RAM.
5. Prove the disabled configuration against the same-board baseline with zero
   Nobro/vendor flash and RAM delta plus forbidden-symbol checks.
6. Add the capability claim to `platform_tiers.json`; its validator loads the
   feature vocabulary and exact binding from the registry, so a new kind needs
   no Python source edit.

Run `python tools/check_board_features.py`,
`python tools/check_platform_tiers.py --selftest`, and the exact target gate.
`compile-only` is not physical support, and an unbound capability kind is not
a platform claim.

The in-tree `audio/esp32s3-es8311` bridge is the reference split for a vendor
audio stack: keep frame/lifecycle/admission policy in `nobro-audio`, keep the
board core's I2S/DMA ownership in its transport, and pin an external Arduino
library as a catalog member rather than copying it into the adapter. Use
`NobroNiusAudio.h` as the C++ facade pattern. The reference binding shows the
required closure shape: exact format/frame/rate identity, same-target
enabled/disabled pricing, pinned source ownership, measured runtime cost,
repeated recovery, and physical playback/capture. Do not reuse that price for
another codec or fill unknown runtime heap or DMA reservations with zero.

The same split applies to ADC-DMA, LEDC, and RMT. Put a new implementation
under the existing `sensors` or `servo` adapter category, keep the portable
shape in `nobro-sensor` or `nobro-servo`, and expose a small optional facade.
Register each stack separately (`adc_dma`, `pwm_ledc`, or `pulse_rmt`) because
one board may combine or replace them independently. A target build establishes
API compatibility only; add an exact binding after resource measurement,
disabled-state zero-cost proof, report wiring, and physical evidence.
Two implementations of one capability are separate backends, not new
ecosystems: for example, the compact Arduino continuous-ADC path and the
fixed-capacity ESP-IDF persistent-buffer path share `adc_dma` and the portable
sensor contract, while retaining independent prices and evidence.

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
