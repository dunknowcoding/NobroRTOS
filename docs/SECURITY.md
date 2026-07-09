# Security Model

NobroRTOS treats security the way it treats everything else: as **explicit,
machine-checkable contracts**, not vibes. This page states what exists, what each
piece actually guarantees, and where the boundaries are.

## What firmware gets

| Mechanism | What it guarantees | Where |
| --- | --- | --- |
| **Capability grants** | a module only touches the services its manifest declares; admission rejects over-claiming before anything runs | `nobro_kernel` (manifest → admission) |
| **Quota ledger** | RAM/flash/CPU budgets are reserved up front; a module cannot starve the system at runtime | `nobro_kernel::quota` |
| **Peripheral leases** | exclusive, owner-checked access to buses/timers/radios; wrong-owner release is an error, verified by a 30k-op property test | `nobro_hal::lease` |
| **Bounded everything** | fixed-capacity mailboxes, pools, bridges; no heap = no heap exploits, no fragmentation | whole tree (`no_std`, no alloc) |

## The update trust chain (our own crypto, no vendor lock)

```
measure = SHA-256(image)
sig     = HMAC-SHA256(boot_key, measure || version)
verify  → Accept / RejectTampered / RejectSignature / RejectRollback
```

- `nobro_secure::SecureBoot` makes the **decision**: a tampered, forged, or
  version-rolled-back image is refused; `commit()` ratchets the anti-rollback floor.
- `tools/sign_firmware.py` is the authority side; host and device are **pinned to the
  same test vector**, so a divergence breaks the build, not a deployment.
- `tools/ota_preflight_demo.py` chains sign → verify → admission → boot in one gated
  script and must *reject* the bad images to pass CI.
- Supporting pieces: `KeyStore` (slot-addressed keys, no raw key in app code),
  `RollbackGuard`, `TamperSeal` (region integrity), `AuditLog` (hash-chained events).

**Honest boundary:** the *jump-to-image* step of secure boot belongs to a bootloader
we don't ship yet; NobroRTOS provides the verification core and proves it rejects.
HMAC means the verifier holds the (symmetric) key — asymmetric signatures are the
planned upgrade when a bootloader lands.

## Diagnostics without disclosure

`NOBRO_*` reports expose *state*, not secrets: fixed-layout counters, statuses, and
checksums. Keys never appear in reports; the evidence pack redacts machine-specific
paths and environments by construction.

## Sandboxing direction (exploration)

The C-ABI module boundary is being modeled for **Wasm-style isolation** (fixed linear
memory, bounds-checked marshaling, per-poll fuel): see `docs/WASM_MODULE_SLOT.md`.
Today's C/C++ modules are trusted code behind capability grants — isolation claims
wait until a real runtime is embedded and verified.

## Reporting a vulnerability

Open a GitHub issue with the `security` label, or contact the maintainer via the
repository profile for anything sensitive. Include reproduction steps and the commit
hash. There is no bug-bounty program; there *is* a maintainer who treats a failing
security gate as a stop-ship.
